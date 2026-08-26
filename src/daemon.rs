use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::frame::EthernetFrame;
use crate::io::TapPort;
use crate::switch::{Forward, PortId, Switch, Vlan};

/// Parses `<tap-name>:<vlan-id>` pairs, e.g. `tap0:10 tap1:10 tap2:20`.
/// Real config (TOML, live reconfig) is phase 5 — this is just enough to
/// stand the daemon up and prove phase 3's `ping`-across-namespaces case.
///
/// # Errors
///
/// Returns an error if any argument isn't `<tap-name>:<vlan-id>`, or if a
/// VLAN id doesn't parse as a `u16`.
pub fn parse_port_specs(args: impl Iterator<Item = String>) -> io::Result<Vec<(String, Vlan)>> {
    args.map(|arg| {
        let (name, vlan) = arg.split_once(':').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("expected <tap-name>:<vlan-id>, got {arg:?}"),
            )
        })?;
        let vlan: Vlan = vlan.parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad VLAN id in {arg:?}"),
            )
        })?;
        Ok((name.to_owned(), vlan))
    })
    .collect()
}

/// Opens a TAP device per `(name, vlan)` pair and runs the switch until a
/// port's TAP device errors out. Each port gets a reader task and a writer
/// task; a single forwarding task owns the `Switch` core and needs no
/// locking despite every port's reader feeding it concurrently.
///
/// # Errors
///
/// Returns an error if a TAP device can't be opened (most commonly a
/// missing `CAP_NET_ADMIN`) or if there are too many ports to fit a `u32`
/// port index.
pub async fn run(specs: Vec<(String, Vlan)>) -> io::Result<()> {
    let mut switch = Switch::new();
    let mut writers: HashMap<PortId, mpsc::UnboundedSender<Vec<u8>>> = HashMap::new();
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<(PortId, Vec<u8>)>();

    for (index, (name, vlan)) in specs.into_iter().enumerate() {
        let port_index = u32::try_from(index)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many ports"))?;
        let port = PortId(port_index);
        let tap = Arc::new(TapPort::open(&name)?);
        switch.add_port(port, vlan);
        eprintln!("{port:?}: {name} (vlan {vlan})");

        // Writer task: this port's outbound queue, drained onto the TAP device.
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        writers.insert(port, tx);
        let writer_tap = Arc::clone(&tap);
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                if let Err(e) = writer_tap.send(&bytes).await {
                    eprintln!("{port:?}: send error: {e}");
                }
            }
        });

        // Reader task: every frame this port sees goes into the shared queue
        // the forwarding loop below drains.
        let reader_tap = Arc::clone(&tap);
        let frame_tx = frame_tx.clone();
        tokio::spawn(async move {
            loop {
                match reader_tap.recv().await {
                    Ok(bytes) => {
                        if frame_tx.send((port, bytes)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("{port:?}: recv error: {e}");
                        break;
                    }
                }
            }
        });
    }
    drop(frame_tx);

    // The forwarding loop: the only place that touches `switch`, so it needs
    // no locking even though every port's reader task feeds it concurrently.
    while let Some((ingress, bytes)) = frame_rx.recv().await {
        let decision = match EthernetFrame::parse(&bytes) {
            Ok(frame) => switch.forward(ingress, &frame),
            Err(e) => {
                eprintln!("{ingress:?}: dropping malformed frame: {e}");
                continue;
            }
        };
        match decision {
            Ok(Forward::Unicast(egress)) => {
                if let Some(tx) = writers.get(&egress) {
                    let _ = tx.send(bytes);
                }
            }
            Ok(Forward::Flood(targets)) => {
                for target in targets {
                    if let Some(tx) = writers.get(&target) {
                        let _ = tx.send(bytes.clone());
                    }
                }
            }
            Ok(Forward::Drop) => {}
            Err(e) => eprintln!("{ingress:?}: {e}"),
        }
    }

    Ok(())
}

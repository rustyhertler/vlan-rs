use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::frame::EthernetFrame;
use crate::io::TapPort;
use crate::switch::{Forward, PortId, Switch, Vlan};

/// How many not-yet-written frames a port's outbound queue can hold before
/// new ones are dropped. Bounded on purpose: an `unbounded_channel` here
/// would let one stalled peer grow memory without limit; a real switch's
/// answer to a full egress queue is tail-drop, not unbounded buffering.
const OUTBOUND_QUEUE_DEPTH: usize = 256;

/// Parses `<tap-name>:<vlan-id>` pairs, e.g. `tap0:10 tap1:10 tap2:20`.
/// Real config (TOML, live reconfig) is phase 5 — this is just enough to
/// stand the daemon up and prove phase 3's `ping`-across-namespaces case.
///
/// # Errors
///
/// Returns an error if any argument isn't `<tap-name>:<vlan-id>`, if a
/// VLAN id doesn't parse as a `u16`, or if a TAP name is repeated (each
/// port needs its own device — two ports sharing one would let the switch
/// flood a frame back out the interface it just arrived on).
pub fn parse_port_specs(args: impl Iterator<Item = String>) -> io::Result<Vec<(String, Vlan)>> {
    let specs: Vec<(String, Vlan)> = args
        .map(|arg| {
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
        .collect::<io::Result<_>>()?;

    let mut seen = HashSet::new();
    for (name, _) in &specs {
        if !seen.insert(name.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate TAP device name: {name:?}"),
            ));
        }
    }

    Ok(specs)
}

/// One thing that happened on a port, fed into the central forwarding loop.
enum PortEvent {
    /// A frame arrived.
    Frame(Arc<[u8]>),
    /// The port's TAP device closed or errored — it's gone for good.
    Down,
}

/// Sends `bytes` to `port`'s outbound queue if it still has one. Silently
/// drops (with a log line) if the queue is full or the port is gone —
/// there's no delivery guarantee to build on top of here, only best-effort.
fn deliver(writers: &HashMap<PortId, mpsc::Sender<Arc<[u8]>>>, port: PortId, bytes: &Arc<[u8]>) {
    let Some(tx) = writers.get(&port) else {
        return;
    };
    if tx.try_send(Arc::clone(bytes)).is_err() {
        eprintln!("{port:?}: outbound queue full, dropping frame");
    }
}

/// Opens a TAP device per `(name, vlan)` pair and runs the switch's
/// forwarding loop. Each port gets a reader task and a writer task; a
/// single forwarding task owns the `Switch` core and needs no locking
/// despite every port's reader feeding it concurrently. If a port's TAP
/// device errors out or closes, that port is deregistered from the switch
/// and its tasks stop; the other ports keep running. Returns once every
/// port has gone down.
///
/// # Errors
///
/// Returns an error if a TAP device can't be opened (most commonly a
/// missing `CAP_NET_ADMIN`) or if there are too many ports to fit a `u32`
/// port index.
pub async fn run(specs: Vec<(String, Vlan)>) -> io::Result<()> {
    let mut switch = Switch::new();
    let mut writers: HashMap<PortId, mpsc::Sender<Arc<[u8]>>> = HashMap::new();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(PortId, PortEvent)>();

    for (index, (name, vlan)) in specs.into_iter().enumerate() {
        let port_index = u32::try_from(index)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many ports"))?;
        let port = PortId(port_index);
        let tap = Arc::new(TapPort::open(&name)?);
        switch.add_port(port, vlan);
        eprintln!("{port:?}: {} (vlan {vlan})", tap.name()?);

        // Writer task: this port's outbound queue, drained onto the TAP device.
        let (tx, mut rx) = mpsc::channel::<Arc<[u8]>>(OUTBOUND_QUEUE_DEPTH);
        writers.insert(port, tx);
        let writer_tap = Arc::clone(&tap);
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                match writer_tap.send(&bytes).await {
                    Ok(n) if n == bytes.len() => {}
                    Ok(n) => eprintln!("{port:?}: short write: sent {n} of {} bytes", bytes.len()),
                    Err(e) => eprintln!("{port:?}: send error: {e}"),
                }
            }
        });

        // Reader task: every frame this port sees goes into the shared queue
        // the forwarding loop below drains; a closed or errored device sends
        // one `Down` event so the forwarding loop can excise the port.
        let reader_tap = Arc::clone(&tap);
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                match reader_tap.recv().await {
                    Ok(bytes) if bytes.is_empty() => {
                        eprintln!("{port:?}: device closed");
                        let _ = event_tx.send((port, PortEvent::Down));
                        break;
                    }
                    Ok(bytes) => {
                        if event_tx
                            .send((port, PortEvent::Frame(Arc::from(bytes))))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("{port:?}: recv error: {e}");
                        let _ = event_tx.send((port, PortEvent::Down));
                        break;
                    }
                }
            }
        });
    }
    drop(event_tx);

    // The forwarding loop: the only place that touches `switch`, so it needs
    // no locking even though every port's reader task feeds it concurrently.
    while let Some((ingress, event)) = event_rx.recv().await {
        let bytes = match event {
            PortEvent::Down => {
                eprintln!("{ingress:?}: removing dead port");
                switch.remove_port(ingress);
                writers.remove(&ingress); // drops the sender, ending the writer task
                continue;
            }
            PortEvent::Frame(bytes) => bytes,
        };

        let decision = match EthernetFrame::parse(&bytes) {
            Ok(frame) => switch.forward(ingress, &frame),
            Err(e) => {
                eprintln!("{ingress:?}: dropping malformed frame: {e}");
                continue;
            }
        };
        match decision {
            Ok(Forward::Unicast(egress)) => deliver(&writers, egress, &bytes),
            Ok(Forward::Flood(targets)) => {
                for target in targets {
                    deliver(&writers, target, &bytes);
                }
            }
            Ok(Forward::Drop) => {}
            Err(e) => eprintln!("{ingress:?}: {e}"),
        }
    }

    Ok(())
}

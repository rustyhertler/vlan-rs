use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::frame::EthernetFrame;
use crate::io::TapPort;
use crate::switch::{Delivery, PortId, PortMode, Switch, Vlan};

/// How many not-yet-written frames a port's outbound queue can hold before
/// new ones are dropped. Bounded on purpose: an `unbounded_channel` here
/// would let one stalled peer grow memory without limit; a real switch's
/// answer to a full egress queue is tail-drop, not unbounded buffering.
const OUTBOUND_QUEUE_DEPTH: usize = 256;

fn bad_spec(arg: &str, msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{msg} in {arg:?}"))
}

/// Parses one `<tap-name>:<vlan-id>` (access) or
/// `<tap-name>:trunk:<native-or-->:<allowed-csv>` (trunk) argument.
/// `-` means no native VLAN; the allowed list may be empty only if a
/// native VLAN is set (an untagged-only trunk).
fn parse_spec(arg: &str) -> io::Result<(String, PortMode)> {
    let (name, rest) = arg.split_once(':').ok_or_else(|| {
        bad_spec(
            arg,
            "expected <tap-name>:<vlan-id> or <tap-name>:trunk:<native-or-->:<allowed-csv>",
        )
    })?;

    let mode = if let Some(trunk_spec) = rest.strip_prefix("trunk:") {
        let (native_str, allowed_str) = trunk_spec
            .split_once(':')
            .ok_or_else(|| bad_spec(arg, "expected trunk:<native-or-->:<allowed-csv>"))?;

        let native = if native_str == "-" {
            None
        } else {
            Some(
                native_str
                    .parse::<Vlan>()
                    .map_err(|_| bad_spec(arg, "bad native VLAN id"))?,
            )
        };

        // An empty allowed_str means "no allowed VLANs" (fine if native is
        // set); an empty *field* within it (a stray or trailing comma) is
        // a likely typo, not the same thing — reject it rather than
        // silently dropping it, since dropping it here would look like
        // "the VLAN just isn't on this trunk" at delivery time instead of
        // a config error at startup time.
        let allowed = if allowed_str.is_empty() {
            Vec::new()
        } else {
            allowed_str
                .split(',')
                .map(|s| {
                    s.parse::<Vlan>()
                        .map_err(|_| bad_spec(arg, "bad VLAN id in allowed list"))
                })
                .collect::<io::Result<Vec<Vlan>>>()?
        };

        // PortMode::trunk itself enforces native+allowed can't both be
        // empty — see its doc comment for why that's checked there and
        // not just here.
        PortMode::trunk(native, allowed).map_err(|e| bad_spec(arg, &e.to_string()))?
    } else {
        let vlan: Vlan = rest.parse().map_err(|_| bad_spec(arg, "bad VLAN id"))?;
        PortMode::access(vlan).map_err(|e| bad_spec(arg, &e.to_string()))?
    };

    Ok((name.to_owned(), mode))
}

/// Parses one port spec per argument — see [`parse_spec`] for the grammar.
/// Real config (TOML, live reconfig) is phase 5 — this is just enough to
/// stand the daemon up and prove phases 3 and 4's acceptance tests.
///
/// # Errors
///
/// Returns an error if any argument doesn't match the grammar, a VLAN id
/// is out of range, or a TAP name is repeated (each port needs its own
/// device — two ports sharing one would let the switch flood a frame back
/// out the interface it just arrived on).
pub fn parse_port_specs(args: impl Iterator<Item = String>) -> io::Result<Vec<(String, PortMode)>> {
    let specs: Vec<(String, PortMode)> = args
        .map(|arg| parse_spec(&arg))
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

fn describe(mode: &PortMode) -> String {
    match mode {
        PortMode::Access { vlan } => format!("access, vlan {vlan}"),
        PortMode::Trunk { native, allowed } => {
            let mut allowed: Vec<_> = allowed.iter().collect();
            allowed.sort_unstable();
            format!("trunk, native {native:?}, allowed {allowed:?}")
        }
    }
}

/// One thing that happened on a port, fed into the central forwarding loop.
enum PortEvent {
    /// A frame arrived.
    Frame(Vec<u8>),
    /// The port's TAP device closed or errored — it's gone for good.
    Down,
}

/// Sends `bytes` to `port`'s outbound queue if it still has one. Silently
/// drops (with a log line) if the queue is full or the port is gone —
/// there's no delivery guarantee to build on top of here, only best-effort.
fn deliver(writers: &HashMap<PortId, mpsc::Sender<Vec<u8>>>, port: PortId, bytes: Vec<u8>) {
    let Some(tx) = writers.get(&port) else {
        return;
    };
    if tx.try_send(bytes).is_err() {
        eprintln!("{port:?}: outbound queue full, dropping frame");
    }
}

/// Opens a TAP device per `(name, mode)` pair and runs the switch's
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
pub async fn run(specs: Vec<(String, PortMode)>) -> io::Result<()> {
    let mut switch = Switch::new();
    let mut writers: HashMap<PortId, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(PortId, PortEvent)>();

    for (index, (name, mode)) in specs.into_iter().enumerate() {
        let port_index = u32::try_from(index)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many ports"))?;
        let port = PortId(port_index);
        let mode_desc = describe(&mode);
        let tap = Arc::new(TapPort::open(&name)?);
        switch.add_port(port, mode);
        eprintln!("{port:?}: {} ({mode_desc})", tap.name()?);

        // Writer task: this port's outbound queue, drained onto the TAP device.
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_QUEUE_DEPTH);
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
                        if event_tx.send((port, PortEvent::Frame(bytes))).is_err() {
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
            Ok(deliveries) => {
                for Delivery { port, bytes } in deliveries {
                    deliver(&writers, port, bytes);
                }
            }
            Err(e) => eprintln!("{ingress:?}: {e}"),
        }
    }

    Ok(())
}

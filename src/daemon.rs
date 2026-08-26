use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::frame::EthernetFrame;
use crate::io::TapPort;
use crate::switch::{Delivery, PortId, PortMode, Switch, Vlan};

/// How many not-yet-written frames a port's outbound queue can hold before
/// new ones are dropped. Bounded on purpose: an `unbounded_channel` here
/// would let one stalled peer grow memory without limit; a real switch's
/// answer to a full egress queue is tail-drop, not unbounded buffering.
const OUTBOUND_QUEUE_DEPTH: usize = 256;

/// How long a learned MAC entry survives without being relearned — matches
/// the ~300s default most real switches use.
const MAC_MAX_AGE: Duration = Duration::from_secs(300);
/// How often the aging sweep runs. Shorter than `MAC_MAX_AGE` so a stale
/// entry doesn't linger much past its actual age-out point; long enough
/// that the sweep itself is negligible overhead.
const MAC_AGE_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// How often the loop guard broadcasts its probe out every port. Much
/// shorter than the MAC-aging sweep — containing a broadcast storm is
/// urgent in a way aging out a stale route isn't.
const LOOP_PROBE_INTERVAL: Duration = Duration::from_secs(5);

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
/// The inline-args alternative to [`run_from_config`]'s TOML file; both
/// end up as the same `Vec<(String, PortMode)>` [`run`] takes.
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
    reject_duplicate_names(&specs)?;
    Ok(specs)
}

fn reject_duplicate_names(specs: &[(String, PortMode)]) -> io::Result<()> {
    let mut seen = HashSet::new();
    for (name, _) in specs {
        if !seen.insert(name.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate TAP device name: {name:?}"),
            ));
        }
    }
    Ok(())
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

/// A running port: its reader/writer tasks (each holds its own
/// `Arc<TapPort>` clone — that's what actually keeps the device open, not
/// this struct). Dropping this without aborting the tasks first leaks
/// them: the device stays open and the tasks keep running until
/// explicitly stopped, not merely until this struct is dropped. See
/// [`teardown_port`].
struct PortHandle {
    writer_tx: mpsc::Sender<Vec<u8>>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

/// Opens `name` as `mode`, registers it with `switch`, and spawns its
/// reader and writer tasks.
fn spawn_port(
    port: PortId,
    name: &str,
    mode: PortMode,
    switch: &mut Switch,
    event_tx: mpsc::UnboundedSender<(PortId, PortEvent)>,
) -> io::Result<PortHandle> {
    let mode_desc = describe(&mode);
    let tap = Arc::new(TapPort::open(name)?);
    switch.add_port(port, mode);
    eprintln!("{port:?}: {} ({mode_desc})", tap.name()?);

    // Writer task: this port's outbound queue, drained onto the TAP device.
    let (writer_tx, mut rx) = mpsc::channel::<Vec<u8>>(OUTBOUND_QUEUE_DEPTH);
    let writer_tap = Arc::clone(&tap);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            match writer_tap.send(&bytes).await {
                Ok(n) if n == bytes.len() => {}
                Ok(n) => eprintln!("{port:?}: short write: sent {n} of {} bytes", bytes.len()),
                Err(e) => eprintln!("{port:?}: send error: {e}"),
            }
        }
    });

    // Reader task: every frame this port sees goes into the shared queue
    // the forwarding loop drains; a closed or errored device sends one
    // `Down` event so the forwarding loop can excise the port.
    let reader_tap = Arc::clone(&tap);
    let reader = tokio::spawn(async move {
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

    Ok(PortHandle {
        writer_tx,
        reader,
        writer,
    })
}

/// Aborts `handle`'s reader and writer tasks and waits for them to
/// actually finish before returning, so by the time this resolves the
/// underlying TAP device is guaranteed closed — not just requested to
/// close. Reused during live reconfig, where a still-open old device
/// under the same name would make the replacement's `TapPort::open` race
/// against it; a port that's dying on its own (an errored/closed device)
/// doesn't need this — its reader is already exiting unprompted.
async fn teardown_port(handle: PortHandle) {
    handle.reader.abort();
    handle.writer.abort();
    let _ = handle.reader.await;
    let _ = handle.writer.await;
}

/// Sends `bytes` to `port`'s outbound queue if it's still running.
/// Silently drops (with a log line) if the queue is full or the port is
/// gone — there's no delivery guarantee to build on top of here, only
/// best-effort.
fn deliver(handles: &HashMap<PortId, PortHandle>, port: PortId, bytes: Vec<u8>) {
    let Some(handle) = handles.get(&port) else {
        return;
    };
    if handle.writer_tx.try_send(bytes).is_err() {
        eprintln!("{port:?}: outbound queue full, dropping frame");
    }
}

/// Loads `path` as TOML and runs [`run`] with it, keeping `path` around so
/// `SIGHUP` can reload from the same place later.
///
/// # Errors
///
/// Returns an error if `path` can't be read or doesn't parse as a valid
/// topology, or anything [`run`] can return.
pub async fn run_from_config(path: PathBuf) -> io::Result<()> {
    let specs = Config::load(&path)?.into_specs()?;
    run(specs, Some(path)).await
}

/// Opens a TAP device per `(name, mode)` pair and runs the switch's
/// forwarding loop indefinitely — until the process is killed, not merely
/// until every port has gone down, since `SIGHUP` (with `reload_path` set)
/// can always bring new ones up. Each port gets a reader task and a writer
/// task; the forwarding loop, `SIGHUP`, and `SIGUSR1` are all handled by
/// one task via `select!`, so `switch` and the port table need no locking
/// despite every port's reader feeding them concurrently.
///
/// `SIGHUP` reloads `reload_path` (a no-op with a log line if `None`) and
/// does a full teardown-and-rebuild of every port — simpler than diffing
/// against the running config, at the cost of briefly interrupting
/// unchanged ports too. `SIGUSR1` dumps per-port and per-VLAN counters to
/// stderr.
///
/// # Errors
///
/// Returns an error if a TAP device can't be opened (most commonly a
/// missing `CAP_NET_ADMIN`) or if there are too many ports to fit a `u32`
/// port index.
pub async fn run(specs: Vec<(String, PortMode)>, reload_path: Option<PathBuf>) -> io::Result<()> {
    let mut switch = Switch::new();
    let mut handles: HashMap<PortId, PortHandle> = HashMap::new();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(PortId, PortEvent)>();
    let mut port_ids = PortIdAllocator::default();

    for (name, mode) in specs {
        let port = port_ids.next()?;
        let handle = spawn_port(port, &name, mode, &mut switch, event_tx.clone())?;
        handles.insert(port, handle);
    }

    let mut sighup = signal(SignalKind::hangup())?;
    let mut sigusr1 = signal(SignalKind::user_defined1())?;
    let mut age_sweep = tokio::time::interval(MAC_AGE_SWEEP_INTERVAL);
    age_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut loop_probe = tokio::time::interval(LOOP_PROBE_INTERVAL);
    loop_probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some((ingress, event)) = event else {
                    // Every sender (one per reader task) has dropped — can't
                    // actually happen since senders live as long as their
                    // task, and tasks only end after sending Down, but a
                    // stalled loop is worse than a redundant check.
                    continue;
                };
                handle_frame_event(&mut switch, &mut handles, ingress, event);
            }
            _ = sighup.recv() => {
                reload(&mut switch, &mut handles, reload_path.as_ref(), &event_tx, &mut port_ids).await;
            }
            _ = sigusr1.recv() => {
                dump_counters(&switch);
            }
            _ = age_sweep.tick() => {
                let evicted = switch.age_out(MAC_MAX_AGE, Instant::now());
                if evicted > 0 {
                    eprintln!("aged out {evicted} stale MAC table entr{}", if evicted == 1 { "y" } else { "ies" });
                }
            }
            _ = loop_probe.tick() => {
                // Sent raw to every port's writer, not through Switch::forward
                // — a probe bypasses VLAN/tag processing entirely (see
                // forward's doc comment), so it needs no per-port encoding.
                let probe = switch.build_loop_probe();
                for (port, handle) in &handles {
                    // A full writer queue is exactly the condition a storm
                    // produces — silently dropping the probe here would
                    // discard the one frame that could end it, with no
                    // trace of why the loop guard never fired. Logged, not
                    // just best-effort, for that reason.
                    if handle.writer_tx.try_send(probe.clone()).is_err() {
                        eprintln!("{port:?}: outbound queue full, dropped loop-guard probe");
                    }
                }
            }
        }
    }
}

/// Hands out `PortId`s that are never reused for the life of the process,
/// even across a `SIGHUP` reload. `reload`'s `abort()` on a port's old
/// tasks takes effect at their next yield point, not synchronously — one
/// more event (worst case, a stale `Down`) can already be past the
/// now-synchronous `UnboundedSender::send` and sitting in the channel by
/// the time the task actually stops. Restarting numbering from 0 on every
/// reload would let that stale event land on whatever new port reused its
/// old id; never reusing an id means a stale event's id simply isn't in
/// `handles` (or `switch`'s ports) any more, so it's ignored exactly like
/// any other reference to a port that's gone — never misattributed to a
/// different, currently-live one.
#[derive(Default)]
struct PortIdAllocator(u32);

impl PortIdAllocator {
    fn next(&mut self) -> io::Result<PortId> {
        let id = self.0;
        self.0 = self
            .0
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "too many ports"))?;
        Ok(PortId(id))
    }
}

fn handle_frame_event(
    switch: &mut Switch,
    handles: &mut HashMap<PortId, PortHandle>,
    ingress: PortId,
    event: PortEvent,
) {
    let bytes = match event {
        PortEvent::Down => {
            eprintln!("{ingress:?}: removing dead port");
            switch.remove_port(ingress);
            // The reader that sent this is already exiting on its own;
            // just drop our references. Dropping writer_tx ends the
            // writer task once its queue drains.
            handles.remove(&ingress);
            return;
        }
        PortEvent::Frame(bytes) => bytes,
    };

    // Checked around the forward() call, not exposed as part of its return
    // type: blocking is a side effect of a loop-guard probe being handled
    // successfully (see Switch::forward's doc comment), not an error or a
    // delivery — this is the simplest way to still log the transition.
    let was_blocked = switch.is_blocked(ingress);
    let decision = match EthernetFrame::parse(&bytes) {
        Ok(frame) => switch.forward(ingress, &frame, Instant::now()),
        Err(e) => {
            eprintln!("{ingress:?}: dropping malformed frame: {e}");
            return;
        }
    };
    if !was_blocked && switch.is_blocked(ingress) {
        eprintln!("{ingress:?}: loop detected — port blocked");
    }
    match decision {
        Ok(deliveries) => {
            for Delivery { port, bytes } in deliveries {
                deliver(handles, port, bytes);
            }
        }
        Err(e) => eprintln!("{ingress:?}: {e}"),
    }
}

async fn reload(
    switch: &mut Switch,
    handles: &mut HashMap<PortId, PortHandle>,
    reload_path: Option<&PathBuf>,
    event_tx: &mpsc::UnboundedSender<(PortId, PortEvent)>,
    port_ids: &mut PortIdAllocator,
) {
    let Some(path) = reload_path else {
        eprintln!("SIGHUP: no --config file this daemon was started from, ignoring");
        return;
    };

    // This task also owns frame forwarding for every port (see run's
    // doc comment) — spawn_blocking keeps a slow read (a large file, a
    // network filesystem) from stalling that instead of just this reload.
    let load_path = path.clone();
    let load_result =
        tokio::task::spawn_blocking(move || Config::load(&load_path).and_then(Config::into_specs))
            .await;
    let new_specs = match load_result {
        Ok(Ok(specs)) => specs,
        Ok(Err(e)) => {
            eprintln!("reload failed, keeping current config: {e}");
            return;
        }
        Err(join_err) => {
            eprintln!("reload failed ({join_err}), keeping current config");
            return;
        }
    };

    eprintln!(
        "reloading {} port(s) from {} — counters and learned MAC state for every port reset, \
         not just the ones that changed",
        new_specs.len(),
        path.display()
    );
    for (_, handle) in handles.drain() {
        teardown_port(handle).await;
    }
    *switch = Switch::new();

    for (name, mode) in new_specs {
        let port = match port_ids.next() {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "reload: {e}, stopping with {} port(s) active",
                    handles.len()
                );
                break;
            }
        };
        match spawn_port(port, &name, mode, switch, event_tx.clone()) {
            Ok(handle) => {
                handles.insert(port, handle);
            }
            Err(e) => eprintln!("reload: failed to open {name}: {e}"),
        }
    }
    eprintln!("reload complete: {} port(s) active", handles.len());
}

fn dump_counters(switch: &Switch) {
    eprintln!("=== vlan-rs counters ===");
    let mut ports: Vec<_> = switch.all_port_counters().collect();
    ports.sort_by_key(|(p, _)| p.0);
    for (port, c) in ports {
        eprintln!(
            "{port:?}: in={} ({}B) out={} ({}B) drops={}",
            c.frames_in, c.bytes_in, c.frames_out, c.bytes_out, c.drops
        );
    }
    let mut vlans: Vec<_> = switch.all_vlan_counters().collect();
    vlans.sort_by_key(|(v, _)| *v);
    for (vlan, c) in vlans {
        eprintln!(
            "vlan {vlan}: in={} ({}B) out={} ({}B)",
            c.frames_in, c.bytes_in, c.frames_out, c.bytes_out
        );
    }
}

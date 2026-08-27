//! A tiny, hand-rolled HTTP server exposing the same per-port/per-VLAN
//! counters `SIGUSR1` already dumps to stderr (`daemon::dump_counters`),
//! plus each port's live [`PortMode`] — as JSON, and as a small
//! auto-refreshing HTML page. Opt-in via `--dashboard <bind-addr>`; see
//! `daemon::run`'s doc comment for how it's wired into the forwarding
//! loop without adding any locking around [`Switch`].
//!
//! No web framework: no keep-alive, no chunked encoding, no header
//! parsing beyond the request line. A dashboard polled every couple of
//! seconds by a handful of browser tabs doesn't need HTTP/1.1's full
//! feature set, and skipping it is what keeps this dependency-free — the
//! only new dependency surface is two extra `tokio` features (`net`,
//! `io-util`).
//!
//! Trust model: no auth, matching `SIGUSR1`'s own trust model (anyone who
//! can signal the process can already dump these same counters). Binding
//! anything beyond `127.0.0.1` is the operator's explicit, informed
//! choice — documented in `--dashboard`'s help text, not gated in code.

use std::fmt::Write as _;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::switch::{Counters, PortMode, Switch};

/// How long a connection gets to send its request line before it's
/// abandoned. Bounds a stalled/slow client to one lightweight task
/// instead of an indefinite hang — there's no keep-alive here for a
/// client to legitimately hold a connection open across.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

const INDEX_HTML: &str = include_str!("dashboard/index.html");

/// Accepts connections on `listener` forever, one spawned task per
/// connection. `counters_tx` is how a connection reaches the switch:
/// `daemon::run`'s `select!` loop is the only thing allowed to touch
/// `Switch` (see its doc comment), so a request for `/api/counters` hands
/// over a reply channel instead of reading anything itself.
///
/// # Errors
///
/// Returns an error only if `listener.accept()` itself fails (e.g. the
/// process is out of file descriptors) — a single connection's own I/O
/// errors are logged and don't end the accept loop.
pub async fn serve(
    listener: TcpListener,
    counters_tx: mpsc::UnboundedSender<oneshot::Sender<String>>,
) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let counters_tx = counters_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, counters_tx).await {
                eprintln!("dashboard: connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    counters_tx: mpsc::UnboundedSender<oneshot::Sender<String>>,
) -> io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = tokio::time::timeout(REQUEST_READ_TIMEOUT, stream.read(&mut buf))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request timed out"))??;

    // Only the request line matters — headers and any body are ignored
    // outright. GET requests from a browser or curl fit in one read.
    let line = buf[..n].split(|&b| b == b'\n').next().unwrap_or(&[]);
    let line = String::from_utf8_lossy(line);
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    let (status, content_type, body) = match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", INDEX_HTML.to_string()),
        ("GET", "/api/counters") => match request_counters_json(&counters_tx).await {
            Some(json) => ("200 OK", "application/json", json),
            None => server_error(),
        },
        ("GET", _) => ("404 Not Found", "text/plain", "not found".to_string()),
        _ => (
            "405 Method Not Allowed",
            "text/plain",
            "GET only".to_string(),
        ),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

async fn request_counters_json(
    counters_tx: &mpsc::UnboundedSender<oneshot::Sender<String>>,
) -> Option<String> {
    let (reply_tx, reply_rx) = oneshot::channel();
    counters_tx.send(reply_tx).ok()?;
    reply_rx.await.ok()
}

fn server_error() -> (&'static str, &'static str, String) {
    (
        "500 Internal Server Error",
        "text/plain",
        "daemon unavailable".to_string(),
    )
}

/// Renders `switch`'s current per-port and per-VLAN counters (plus each
/// port's mode and loop-guard block state) as JSON — the same data
/// `SIGUSR1`'s `dump_counters` prints to stderr, shaped for a browser
/// instead of a log. Every field is numeric, boolean, or one of a small
/// fixed set of string literals this function itself chooses (`"access"`,
/// `"trunk"`), and no free-text ever flows in here — hand-rolled
/// formatting needs no escaping to stay safe.
///
/// Lists every *registered* port (`Switch::port_ids`), not just ones
/// `all_port_counters` already has an entry for — otherwise a
/// freshly-added or blocked-but-idle port would simply be missing from
/// the dashboard instead of showing up as zero traffic.
#[must_use]
pub fn render_counters_json(switch: &Switch) -> String {
    let mut ports: Vec<_> = switch
        .port_ids()
        .map(|p| {
            (
                p,
                switch.port_counters(p),
                switch.is_blocked(p),
                switch.port_mode(p),
            )
        })
        .collect();
    ports.sort_by_key(|(p, ..)| p.0);

    let mut vlans: Vec<_> = switch.all_vlan_counters().collect();
    vlans.sort_by_key(|(v, _)| *v);

    let mut out = String::from(r#"{"ports":["#);
    for (i, (port, c, blocked, mode)) in ports.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_port(&mut out, port.0, *blocked, mode.as_ref(), c);
    }
    out.push_str(r#"],"vlans":["#);
    for (i, (vlan, c)) in vlans.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_vlan(&mut out, *vlan, c);
    }
    out.push_str("]}");
    out
}

fn write_port(out: &mut String, port: u32, blocked: bool, mode: Option<&PortMode>, c: &Counters) {
    // write! on a String only fails on a formatting bug (never allocation
    // in practice here), so this mirrors loop_guard::build_probe's
    // `let _ =` honesty rather than pretending it can't fail.
    let _ = write!(out, r#"{{"port":{port},"blocked":{blocked},"mode":"#);
    write_mode(out, mode);
    let _ = write!(
        out,
        r#","frames_in":{},"bytes_in":{},"frames_out":{},"bytes_out":{},"drops":{}}}"#,
        c.frames_in, c.bytes_in, c.frames_out, c.bytes_out, c.drops
    );
}

fn write_mode(out: &mut String, mode: Option<&PortMode>) {
    match mode {
        // Every caller derives `port` from the switch's own port set, so
        // `None` (an unregistered port) can't actually happen — but a
        // valid-JSON `null` is a cheap, honest fallback over unwrapping.
        None => out.push_str("null"),
        Some(PortMode::Access { vlan }) => {
            let _ = write!(out, r#"{{"kind":"access","vlan":{vlan}}}"#);
        }
        Some(PortMode::Trunk { native, allowed }) => {
            let mut allowed: Vec<_> = allowed.iter().collect();
            allowed.sort_unstable();
            let _ = write!(out, r#"{{"kind":"trunk","native":"#);
            match native {
                Some(v) => {
                    let _ = write!(out, "{v}");
                }
                None => out.push_str("null"),
            }
            out.push_str(r#","allowed":["#);
            for (i, vlan) in allowed.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{vlan}");
            }
            out.push_str("]}");
        }
    }
}

fn write_vlan(out: &mut String, vlan: u16, c: &Counters) {
    let _ = write!(
        out,
        r#"{{"vlan":{vlan},"frames_in":{},"bytes_in":{},"frames_out":{},"bytes_out":{}}}"#,
        c.frames_in, c.bytes_in, c.frames_out, c.bytes_out
    );
}

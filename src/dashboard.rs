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

/// How long a connection gets to send its full request line before it's
/// abandoned. Bounds a stalled/slow client to one lightweight task
/// instead of an indefinite hang — there's no keep-alive here for a
/// client to legitimately hold a connection open across.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on how much of a request this server will buffer looking
/// for the request line's terminating `\n`. TCP delivers a write as
/// however many reads it delivers as — a single `read()` isn't
/// guaranteed to contain a whole line — so this accumulates across reads
/// rather than assuming the first one has it all; a client that never
/// sends a `\n` (or sends an absurdly long line) gets cut off here
/// rather than growing this buffer without limit.
const MAX_REQUEST_LINE_LEN: usize = 8192;

/// After responding, how long (and how much) this server will keep
/// reading and discarding whatever the client sends, before finally
/// closing the connection. Draining first — rather than closing straight
/// away — is what keeps an unread request body from producing a TCP RST
/// on the client's end instead of a clean close.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const DRAIN_MAX_BYTES: usize = 64 * 1024;

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
    let line = tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request_line(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request timed out"))??;

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
    stream.shutdown().await?;
    // Only after: closing the socket while the kernel still has bytes
    // buffered for it (an unread request body, most commonly) commonly
    // produces a TCP RST instead of a clean FIN — harmless to us since
    // the response already went out, but it can surface on the client's
    // end as a spurious "connection reset" for something as ordinary as
    // a POST with a body this server never reads.
    drain_request_body(&mut stream).await;
    Ok(())
}

/// Reads from `stream` until a full line (through `\n`) is buffered, or
/// [`MAX_REQUEST_LINE_LEN`] is reached. Only the request line is ever
/// used — headers and any body are ignored outright, which is why this
/// only needs to find one `\n`, not parse `Content-Length` or frame a
/// body at all.
async fn read_request_line(stream: &mut TcpStream) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    while !buf.contains(&b'\n') && buf.len() < MAX_REQUEST_LINE_LEN {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break; // client closed before sending a full line
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let line = buf.split(|&b| b == b'\n').next().unwrap_or(&[]);
    Ok(String::from_utf8_lossy(line).into_owned())
}

/// Best-effort: reads and discards whatever the client sends after the
/// response has already gone out, up to [`DRAIN_TIMEOUT`] /
/// [`DRAIN_MAX_BYTES`] — see the call site for why.
async fn drain_request_body(stream: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    let mut drained = 0usize;
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    while drained < DRAIN_MAX_BYTES {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => drained += n,
            _ => break, // EOF, a read error, or the drain timeout — stop either way
        }
    }
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
/// instead of a log. Every field is a bare number, a bare boolean, or one
/// of a small fixed set of string literals this function itself chooses
/// (`"access"`, `"trunk"`) — except the `u64` counters themselves
/// (`frames_in`/`bytes_in`/`frames_out`/`bytes_out`/`drops`), which are
/// quoted as strings: JS numbers are `f64`, so a bare JSON integer past
/// `2^53` (~9 petabytes of traffic — reachable on a long-running switch)
/// would silently lose precision the moment a browser calls
/// `JSON.parse`; `index.html` reads them back with `BigInt`. No free-text
/// ever flows into any of this — hand-rolled formatting needs no
/// escaping to stay safe.
///
/// Lists every *registered* port (`Switch::port_ids`), not just ones
/// `all_port_counters` already has an entry for — otherwise a
/// freshly-added or blocked-but-idle port would simply be missing from
/// the dashboard instead of showing up as zero traffic.
#[must_use]
pub fn render_counters_json(switch: &Switch) -> String {
    // port_snapshot combines what would otherwise be three separate
    // lookups (port_counters, is_blocked, port_mode) per port into one.
    let mut ports: Vec<_> = switch
        .port_ids()
        .filter_map(|p| {
            switch
                .port_snapshot(p)
                .map(|(c, blocked, mode)| (p, c, blocked, mode))
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
        write_port(&mut out, port.0, *blocked, mode, c);
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

fn write_port(out: &mut String, port: u32, blocked: bool, mode: &PortMode, c: &Counters) {
    // write! on a String only fails on a formatting bug (never allocation
    // in practice here), so this mirrors loop_guard::build_probe's
    // `let _ =` honesty rather than pretending it can't fail.
    let _ = write!(out, r#"{{"port":{port},"blocked":{blocked},"mode":"#);
    write_mode(out, mode);
    let _ = write!(
        out,
        r#","frames_in":"{}","bytes_in":"{}","frames_out":"{}","bytes_out":"{}","drops":"{}"}}"#,
        c.frames_in, c.bytes_in, c.frames_out, c.bytes_out, c.drops
    );
}

fn write_mode(out: &mut String, mode: &PortMode) {
    match mode {
        PortMode::Access { vlan } => {
            let _ = write!(out, r#"{{"kind":"access","vlan":{vlan}}}"#);
        }
        PortMode::Trunk { native, allowed } => {
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
        r#"{{"vlan":{vlan},"frames_in":"{}","bytes_in":"{}","frames_out":"{}","bytes_out":"{}"}}"#,
        c.frames_in, c.bytes_in, c.frames_out, c.bytes_out
    );
}

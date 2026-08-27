use std::net::SocketAddr;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use vlan_rs::dashboard::{render_counters_json, serve};
use vlan_rs::frame::EthernetFrame;
use vlan_rs::switch::{BROADCAST, PortId, PortMode, Switch};

const PORT1: PortId = PortId(1);
const PORT2: PortId = PortId(2);
const HOST_A: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01];

fn frame(dst: [u8; 6], src: [u8; 6]) -> EthernetFrame<'static> {
    EthernetFrame {
        dst,
        src,
        tag: None,
        ethertype: 0x0800,
        payload: &[0xAB; 46],
    }
}

// --- render_counters_json: pure, no networking ---

#[test]
fn renders_an_empty_switch_as_empty_arrays() {
    let switch = Switch::new();
    assert_eq!(render_counters_json(&switch), r#"{"ports":[],"vlans":[]}"#);
}

#[test]
fn renders_an_access_ports_mode_and_counters() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    switch.add_port(PORT2, PortMode::access(10).unwrap());
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), Instant::now())
        .unwrap();

    let json = render_counters_json(&switch);
    assert!(json.contains(
        r#"{"port":1,"blocked":false,"mode":{"kind":"access","vlan":10},"frames_in":"1","#
    ));
    assert!(json.contains(r#""vlans":[{"vlan":10,"frames_in":"1","#));
}

#[test]
fn counter_values_are_quoted_json_strings_not_bare_numbers() {
    // A bare JSON integer above 2^53 silently loses precision once a
    // browser's JSON.parse hands it to an f64 — quoting u64 counters as
    // strings (index.html reads them back with BigInt) avoids that. This
    // pins the wire format down so it can't regress back to bare numbers.
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    switch
        .forward(PORT1, &frame(BROADCAST, HOST_A), Instant::now())
        .unwrap();

    let json = render_counters_json(&switch);
    assert!(json.contains(r#""frames_in":"1""#));
    assert!(!json.contains(r#""frames_in":1"#));
    assert!(json.contains(r#""drops":"0""#));
}

#[test]
fn renders_a_trunk_ports_mode_with_sorted_allowed_vlans() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::trunk(Some(10), [30, 20]).unwrap());

    let json = render_counters_json(&switch);
    assert!(json.contains(r#""mode":{"kind":"trunk","native":10,"allowed":[20,30]}"#));
}

#[test]
fn renders_an_untagged_only_trunks_null_native() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::trunk(None, [20]).unwrap());

    let json = render_counters_json(&switch);
    assert!(json.contains(r#""mode":{"kind":"trunk","native":null,"allowed":[20]}"#));
}

#[test]
fn renders_a_blocked_ports_status() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    switch.block_port(PORT1);

    assert!(render_counters_json(&switch).contains(r#""port":1,"blocked":true"#));
}

#[test]
fn ports_and_vlans_are_sorted_by_id() {
    let mut switch = Switch::new();
    switch.add_port(PortId(3), PortMode::access(30).unwrap());
    switch.add_port(PortId(1), PortMode::access(10).unwrap());

    let json = render_counters_json(&switch);
    let port1 = json.find(r#""port":1"#).unwrap();
    let port3 = json.find(r#""port":3"#).unwrap();
    assert!(port1 < port3, "port 1 should be listed before port 3");
    let vlan10 = json.find(r#""vlan":10"#).unwrap();
    let vlan30 = json.find(r#""vlan":30"#).unwrap();
    assert!(vlan10 < vlan30, "vlan 10 should be listed before vlan 30");
}

// --- serve(): a real TcpListener on an ephemeral port, real HTTP requests ---

/// Binds `switch` behind a live `dashboard::serve` on `127.0.0.1:0` and
/// returns the address it's actually listening on. No daemon, no TAP
/// devices — a plain in-memory task answers each `/api/counters` request
/// against the given `Switch`, exactly like `daemon::run`'s `select!` arm
/// does for real.
async fn spawn_dashboard(switch: Switch) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (counters_tx, mut counters_rx) = mpsc::unbounded_channel::<oneshot::Sender<String>>();
    tokio::spawn(serve(listener, counters_tx));
    tokio::spawn(async move {
        while let Some(reply_tx) = counters_rx.recv().await {
            let _ = reply_tx.send(render_counters_json(&switch));
        }
    });
    addr
}

/// Sends `request` verbatim and reads the response until the server
/// closes the connection — every response here is `Connection: close`,
/// so end-of-stream is always the right read boundary.
async fn request(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    String::from_utf8(buf).unwrap()
}

#[tokio::test]
async fn serves_the_index_page() {
    let addr = spawn_dashboard(Switch::new()).await;
    let resp = request(addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").await;

    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp.contains("Content-Type: text/html"));
    assert!(resp.contains("vlan-rs dashboard"));
}

#[tokio::test]
async fn serves_counters_as_json_over_the_wire() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    let addr = spawn_dashboard(switch).await;

    let resp = request(addr, "GET /api/counters HTTP/1.1\r\nHost: x\r\n\r\n").await;

    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp.contains("Content-Type: application/json"));
    assert!(resp.contains(r#""port":1,"blocked":false,"mode":{"kind":"access","vlan":10}"#));
}

#[tokio::test]
async fn unknown_path_is_404() {
    let addr = spawn_dashboard(Switch::new()).await;
    let resp = request(addr, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.starts_with("HTTP/1.1 404 Not Found\r\n"));
}

#[tokio::test]
async fn non_get_method_is_405() {
    let addr = spawn_dashboard(Switch::new()).await;
    let resp = request(addr, "POST /api/counters HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert!(resp.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
}

#[tokio::test]
async fn a_request_line_split_across_multiple_writes_is_still_parsed() {
    // TCP makes no promise that one write() on the client arrives as one
    // read() on the server — sending the request line in two separate
    // writes, with a yield in between so they can't coalesce into a
    // single read, exercises that the server accumulates across reads
    // instead of assuming the first one has the whole line.
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    let addr = spawn_dashboard(switch).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET /api/count").await.unwrap();
    tokio::task::yield_now().await;
    stream
        .write_all(b"ers HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8(buf).unwrap();

    assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp.contains(r#""port":1"#));
}

#[tokio::test]
async fn a_request_with_an_unread_body_still_gets_a_complete_response() {
    // The server only ever reads the request line — it never parses
    // Content-Length or reads a body. Sending one anyway is exactly the
    // scenario that used to risk a TCP RST cutting the response short
    // (draining fixed that); this checks the client-visible behavior
    // stays a normal, complete response.
    let addr = spawn_dashboard(Switch::new()).await;
    let body = "x".repeat(4096);
    let resp = request(
        addr,
        &format!(
            "POST /api/counters HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;

    assert!(resp.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
    assert!(resp.ends_with("GET only"));
}

#[tokio::test]
async fn multiple_concurrent_requests_all_get_answered() {
    let mut switch = Switch::new();
    switch.add_port(PORT1, PortMode::access(10).unwrap());
    let addr = spawn_dashboard(switch).await;

    let requests = (0..8).map(|_| request(addr, "GET /api/counters HTTP/1.1\r\nHost: x\r\n\r\n"));
    let responses = futures_join_all(requests).await;

    for resp in responses {
        assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(resp.contains(r#""port":1"#));
    }
}

/// A minimal stand-in for `futures::future::join_all` — pulling in the
/// `futures` crate for one test helper isn't worth a new dev-dependency;
/// spawning each request as its own task and awaiting the handles gets
/// the same "run these concurrently" property.
async fn futures_join_all(
    requests: impl Iterator<Item = impl std::future::Future<Output = String> + Send + 'static>,
) -> Vec<String> {
    let handles: Vec<_> = requests.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for handle in handles {
        out.push(handle.await.unwrap());
    }
    out
}

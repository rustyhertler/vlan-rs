// See src/lib.rs for why: a panic here takes down the whole daemon, not
// just one bad request.
#![warn(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

use vlan_rs::daemon;

#[tokio::main]
async fn main() {
    if let Err(e) = try_main().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("usage: vlan-rs [--dashboard <bind-addr>] <port-spec> [<port-spec> ...]");
    eprintln!("       vlan-rs [--dashboard <bind-addr>] --config <path.toml>");
    eprintln!("  <port-spec> is one of:");
    eprintln!("    <tap-name>:<vlan-id>                        (access port)");
    eprintln!(
        "    <tap-name>:trunk:<native-or-->:<allowed-csv> (trunk port; '-' = no native VLAN)"
    );
    eprintln!("SIGHUP reloads --config; SIGUSR1 dumps port/VLAN counters to stderr.");
    eprintln!(
        "--dashboard binds a read-only HTTP counters page, e.g. --dashboard 127.0.0.1:8080 \
         (no auth — same trust model as SIGUSR1, so avoid binding beyond 127.0.0.1 on an \
         untrusted network)"
    );
}

fn bad_arg(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.to_string())
}

/// Pulls `--dashboard <bind-addr>` out of `args` if present, wherever it
/// appears — before or after `--config`, or among the port specs — so the
/// rest of argument parsing can proceed unaware it was ever there.
fn take_dashboard_addr(args: &mut Vec<String>) -> io::Result<Option<SocketAddr>> {
    let Some(i) = args.iter().position(|a| a == "--dashboard") else {
        return Ok(None);
    };
    if i + 1 >= args.len() {
        return Err(bad_arg(
            "--dashboard needs a bind address, e.g. 127.0.0.1:8080",
        ));
    }
    let addr_str = args.remove(i + 1);
    args.remove(i);
    addr_str
        .parse::<SocketAddr>()
        .map(Some)
        .map_err(|_| bad_arg(&format!("invalid --dashboard address {addr_str:?}")))
}

async fn try_main() -> io::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dashboard_addr = take_dashboard_addr(&mut args)?;

    if args.first().is_some_and(|a| a == "--config") {
        let [_, path] = args.as_slice() else {
            eprintln!("usage: vlan-rs [--dashboard <bind-addr>] --config <path.toml>");
            std::process::exit(2);
        };
        return daemon::run_from_config(PathBuf::from(path), dashboard_addr).await;
    }

    let specs = daemon::parse_port_specs(args.into_iter())?;
    if specs.is_empty() {
        print_usage();
        std::process::exit(2);
    }
    daemon::run(specs, None, dashboard_addr).await
}

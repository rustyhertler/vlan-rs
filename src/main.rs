// See src/lib.rs for why: a panic here takes down the whole daemon, not
// just one bad request.
#![warn(clippy::unwrap_used, clippy::expect_used)]

use std::io;
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
    eprintln!("usage: vlan-rs <port-spec> [<port-spec> ...]");
    eprintln!("       vlan-rs --config <path.toml>");
    eprintln!("  <port-spec> is one of:");
    eprintln!("    <tap-name>:<vlan-id>                        (access port)");
    eprintln!(
        "    <tap-name>:trunk:<native-or-->:<allowed-csv> (trunk port; '-' = no native VLAN)"
    );
    eprintln!("SIGHUP reloads --config; SIGUSR1 dumps port/VLAN counters to stderr.");
}

async fn try_main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().is_some_and(|a| a == "--config") {
        let [_, path] = args.as_slice() else {
            eprintln!("usage: vlan-rs --config <path.toml>");
            std::process::exit(2);
        };
        return daemon::run_from_config(PathBuf::from(path)).await;
    }

    let specs = daemon::parse_port_specs(args.into_iter())?;
    if specs.is_empty() {
        print_usage();
        std::process::exit(2);
    }
    daemon::run(specs, None).await
}

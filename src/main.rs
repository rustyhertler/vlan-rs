use std::io;

use vlan_rs::daemon;

#[tokio::main]
async fn main() {
    if let Err(e) = try_main().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn try_main() -> io::Result<()> {
    let specs = daemon::parse_port_specs(std::env::args().skip(1))?;
    if specs.is_empty() {
        eprintln!("usage: vlan-rs <tap-name>:<vlan-id> [<tap-name>:<vlan-id> ...]");
        std::process::exit(2);
    }
    daemon::run(specs).await
}

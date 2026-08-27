# vlan-rs

A real 802.1Q software switch in Rust — parses and builds actual tagged Ethernet frames and enforces VLAN isolation between real Linux interfaces (TAP devices in separate network namespaces), not an in-memory simulator.

## What's here

Real 802.1Q tagging/untagging, per-VLAN MAC learning, access and trunk
ports, live TOML config with `SIGHUP` reload, per-port/VLAN counters
(`SIGUSR1` or the `--dashboard` HTTP endpoint), MAC aging, and a
self-loop guard — all running over real TAP devices, not simulated.
QinQ is the one thing knowingly not built.

Full design write-up, including diagrams: [`docs/design.md`](docs/design.md).

## Try it

```sh
cargo build
sudo setcap cap_net_admin+ep target/debug/vlan-rs   # one-time; lets it open TAP devices without sudo
./scripts/netns-smoke-test.sh                        # ping across two namespaces
./scripts/trunk-smoke-test.sh                        # ping across two switches linked by a trunk
./scripts/config-reload-smoke-test.sh                # TOML config + live SIGHUP reload + SIGUSR1 counters
```
All three scripts still need `sudo` themselves, for netns/bridge admin — see each script's header.

Two ways to specify ports:

- Inline args: `<tap-name>:<vlan-id>` for an access port, or
  `<tap-name>:trunk:<native-vlan-or-->:<allowed-vlan-csv>` for a trunk (`-` = no native VLAN)
- `--config <path.toml>` — see `scripts/config-reload-smoke-test.sh` for an example file. While running:
  `kill -HUP <pid>` reloads the file (tears down and rebuilds every port to match it);
  `kill -USR1 <pid>` dumps per-port/VLAN counters to stderr.

Add `--dashboard <bind-addr>` (before the port specs or `--config`) for a live, auto-refreshing HTML view of the
same counters plus each port's mode, e.g. `vlan-rs --dashboard 127.0.0.1:8080 tap0:10`, then open
`http://127.0.0.1:8080/` or `curl http://127.0.0.1:8080/api/counters`. No auth — same trust model as `SIGUSR1`
(anyone who can already signal the process can already dump these counters), so don't bind beyond `127.0.0.1` on
an untrusted network.

Fuzzing the frame parser (needs nightly + `cargo install cargo-fuzz`):

```sh
cargo +nightly fuzz run parse_frame
```

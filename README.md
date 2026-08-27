# vlan-rs

A real 802.1Q software switch in Rust — parses and builds actual tagged Ethernet frames and enforces VLAN isolation between real Linux interfaces (TAP devices in separate network namespaces), not an in-memory simulator.

## Status

**All 5 planned phases done.** Phase 5: TOML config (`--config <path.toml>`), live reload on `SIGHUP` (real TAP ports torn down/rebuilt to match an edited config, no restart), and a `SIGUSR1` counters dump — `scripts/config-reload-smoke-test.sh` passed. Now picking up stretch goals — see `docs/stretch-goals.md`.

Roadmap:

0. Spec & frame primer (no code)
1. Frame parser/builder ✅
2. Switch core, in-process (channels as ports, prove VLAN isolation before touching the kernel) ✅
3. Real I/O via TAP + netns (`ping` across namespaces is the acceptance test) ✅
4. Trunk ports (tag/untag, allowed-VLAN lists, native VLAN, two switches over a trunk) ✅
5. Config & CLI (TOML topology, live reconfig, counters) ✅

Stretch: scripted netns test harness in CI ✅, `cargo-fuzz` on the parser ✅, MAC aging ✅, loop guard ✅ (self-loops only — see `docs/stretch-goals.md`), small web dashboard ✅ (`--dashboard <bind-addr>`). Unscheduled: QinQ.

Full design and rationale: [`docs/plan.md`](docs/plan.md), [`docs/stretch-goals.md`](docs/stretch-goals.md).

## Try it

```sh
cargo build
sudo setcap cap_net_admin+ep target/debug/vlan-rs   # one-time; lets it open TAP devices without sudo
./scripts/netns-smoke-test.sh                        # phase 3: ping across two namespaces
./scripts/trunk-smoke-test.sh                        # phase 4: ping across two switches linked by a trunk
./scripts/config-reload-smoke-test.sh                # phase 5: TOML config + live SIGHUP reload + SIGUSR1 counters
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

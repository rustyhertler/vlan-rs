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

Stretch: scripted netns test harness in CI ✅ and `cargo-fuzz` on the parser ✅. Unscheduled: MAC aging, QinQ, loop guard, small web dashboard.

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

Fuzzing the frame parser (needs nightly + `cargo install cargo-fuzz`):

```sh
cargo +nightly fuzz run parse_frame
```

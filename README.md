# vlan-rs

A real 802.1Q software switch in Rust — parses and builds actual tagged Ethernet frames and enforces VLAN isolation between real Linux interfaces (TAP devices in separate network namespaces), not an in-memory simulator.

## Status

**Phase 4 done** — `scripts/trunk-smoke-test.sh` passes: `ping` between two hosts through two separate `vlan-rs` switch instances linked by a real 802.1Q trunk. Access *and* trunk ports (tag on egress, strip/validate on ingress, allowed-VLAN lists, native VLAN) are in and unit-tested (36 tests). Phase 5 (config & CLI) is next.

Roadmap:

0. Spec & frame primer (no code)
1. Frame parser/builder ✅
2. Switch core, in-process (channels as ports, prove VLAN isolation before touching the kernel) ✅
3. Real I/O via TAP + netns (`ping` across namespaces is the acceptance test) ✅
4. Trunk ports (tag/untag, allowed-VLAN lists, native VLAN, two switches over a trunk) ✅
5. Config & CLI (TOML topology, live reconfig, counters) ← current

Stretch, unscheduled: MAC aging, QinQ, loop guard, scripted netns test harness, web dashboard, `cargo-fuzz` on the parser.

Full design and rationale: [`docs/plan.md`](docs/plan.md).

## Try it

```sh
cargo build
sudo setcap cap_net_admin+ep target/debug/vlan-rs   # one-time; lets it open TAP devices without sudo
./scripts/netns-smoke-test.sh                        # phase 3: ping across two namespaces
./scripts/trunk-smoke-test.sh                        # phase 4: ping across two switches linked by a trunk
```
Both scripts still need `sudo` themselves, for netns/bridge admin — see each script's header.

CLI port syntax: `<tap-name>:<vlan-id>` for an access port, or
`<tap-name>:trunk:<native-vlan-or-->:<allowed-vlan-csv>` for a trunk (`-` = no native VLAN).

# vlan-rs

A real 802.1Q software switch in Rust — parses and builds actual tagged Ethernet frames and enforces VLAN isolation between real Linux interfaces (TAP devices in separate network namespaces), not an in-memory simulator.

## Status

**Phase 3 done** — `scripts/netns-smoke-test.sh` passes: `ping` between two real network namespaces, connected only through a real `vlan-rs` switch over two TAP devices. Phase 4 (trunk ports) is next.

Roadmap:

0. Spec & frame primer (no code)
1. Frame parser/builder ✅
2. Switch core, in-process (channels as ports, prove VLAN isolation before touching the kernel) ✅
3. Real I/O via TAP + netns (`ping` across namespaces is the acceptance test) ✅
4. Trunk ports (tag/untag, allowed-VLAN lists, native VLAN, two switches over a trunk) ← current
5. Config & CLI (TOML topology, live reconfig, counters)

Stretch, unscheduled: MAC aging, QinQ, loop guard, scripted netns test harness, web dashboard, `cargo-fuzz` on the parser.

Full design and rationale: [`docs/plan.md`](docs/plan.md).

## Try it

```sh
cargo build
sudo setcap cap_net_admin+ep target/debug/vlan-rs   # one-time; lets it open TAP devices without sudo
./scripts/netns-smoke-test.sh                        # needs sudo too, for netns admin — see the script's header
```

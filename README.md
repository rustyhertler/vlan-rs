# vlan-rs

A real 802.1Q software switch in Rust — parses and builds actual tagged Ethernet frames and enforces VLAN isolation between real Linux interfaces (TAP devices in separate network namespaces), not an in-memory simulator.

## Status

**Phase 1 — frame parser/builder.** Hand-rolled `EthernetFrame` / `Dot1qTag` types, round-trip unit tests against captured/hand-built frames.

Roadmap:

0. Spec & frame primer (no code)
1. Frame parser/builder ← current
2. Switch core, in-process (channels as ports, prove VLAN isolation before touching the kernel)
3. Real I/O via TAP + netns (`ping` across namespaces is the acceptance test)
4. Trunk ports (tag/untag, allowed-VLAN lists, native VLAN, two switches over a trunk)
5. Config & CLI (TOML topology, live reconfig, counters)

Stretch, unscheduled: MAC aging, QinQ, loop guard, scripted netns test harness, web dashboard, `cargo-fuzz` on the parser.

Full design and rationale: [`docs/plan.md`](docs/plan.md).
# vlan-rs

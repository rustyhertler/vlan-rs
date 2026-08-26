# vlan-rs — project plan

Status: Phase 3 in progress (as of 2026-08-25). This is the committed, durable version of the plan. A richer, interactively-reviewable copy is drafted locally via the `blueprint` tool during active planning rounds — this file is what gets updated once a round is finished, so it stays readable to anyone without that tool installed.

## Scope (assumption, ~65% confidence)

Real 802.1Q software switch: parses/builds actual tagged Ethernet frames, enforces VLAN isolation between real Linux interfaces (TAP + network namespaces), not an in-memory-only simulator.

Two alternatives were considered and set aside:
- a pure in-memory simulator (lower effort, no kernel I/O — you never find out if your length fields are wrong)
- managing kernel VLAN interfaces via netlink instead of writing switch logic ourselves (answers "can Linux do VLANs," not "can *I* build a switch")

## The 802.1Q frame

An Ethernet II frame with a 4-byte tag inserted between Src MAC and EtherType:

```
Dst MAC (6) | Src MAC (6) | TPID 0x8100 (2) | PCP(3) DEI(1) VID(12) (2) | EtherType (2) | Payload
```

| Field  | Size    | Notes |
|--------|---------|-------|
| TPID   | 2 bytes | Fixed `0x8100` for 802.1Q; `0x88a8` for a QinQ outer tag (stretch goal) |
| PCP    | 3 bits  | Priority code point — round-tripped only, not enforced |
| DEI    | 1 bit   | Drop eligible indicator — round-tripped only |
| VID    | 12 bits | VLAN ID, 1–4094 valid; 0 and 4095 reserved |

## Architecture

Four layers, each depending only on the one below it:

1. **I/O** — TAP device per port, one netns per simulated host, veth pairs
2. **Frame** — Ethernet + 802.1Q parse/build (hand-rolled first, checked against `etherparse`)
3. **Switch core** — per-VLAN MAC learning table, port state (access/trunk/PVID/allowed-VLANs), forward/flood/learn logic; zero I/O, pure logic, unit-testable
4. **Control** — TOML config at startup; later a CLI/JSON API for live reconfig

The switch core has no I/O dependencies — ports are just an abstraction it forwards frames to. That's what makes phase 2 unit-testable without a kernel in the loop.

## Roadmap

0. **Spec & frame primer** — no code; nail down 802.1Q vocabulary.
1. **Frame parser/builder** ✅ *done*. Hand-rolled `EthernetFrame` / `Dot1qTag`, round-trip unit tests against captured/hand-built frames. Highest-value phase — this is where the format-level bugs live.
2. **Switch core, in-process** ✅ *done*. Channels stand in for ports; prove VLAN isolation with unit tests before touching the kernel.
3. **Real I/O via TAP + netns** ← *current*. Tokio event loop over TAP fds; `ping` across two namespaces is the acceptance test.
4. **Trunk ports.** Tag/untag on trunk egress/ingress, allowed-VLAN lists, native VLAN, two switches linked by a trunk.
5. **Config & CLI.** TOML topology file, live reconfig, per-port/VLAN counters.

Stretch, unscheduled: MAC aging, QinQ, loop guard, scripted netns test harness, small web dashboard, `cargo-fuzz` on the parser.

## Phase 1 in detail — frame parser/builder

Hand-rolled deliberately. `etherparse` is a check against the hand-rolled version, not a shortcut past it.

### Proposed types

```rust
// src/frame/dot1q.rs
pub struct Dot1qTag {
    pub pcp: u8,   // 3 bits, stored 0..=7
    pub dei: bool, // 1 bit
    pub vid: u16,  // 12 bits, stored 0..=4094 (1..=4094 valid on the wire)
}

// src/frame/ethernet.rs
pub struct EthernetFrame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub tag: Option<Dot1qTag>,
    pub ethertype: u16,
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ParseError> { /* ... */ }
    pub fn write_into(&self, out: &mut Vec<u8>) { /* ... */ }
}
```

### File-by-file plan

| File | Change | ~Lines |
|------|--------|--------|
| `Cargo.toml` | Add `etherparse` as a `[dev-dependencies]` crate (reference-check only) | 2 |
| `src/frame/mod.rs` | New module, re-exports `EthernetFrame`, `Dot1qTag`, `ParseError` | 10 |
| `src/frame/ethernet.rs` | New: `EthernetFrame` struct, `parse()`, `write_into()` | 90 |
| `src/frame/dot1q.rs` | New: `Dot1qTag` struct, bit-packing helpers for PCP/DEI/VID into the 2-byte TCI | 60 |
| `src/frame/error.rs` | New: `ParseError` enum (too short, bad TPID, truncated payload, etc.) | 25 |
| `tests/frame_roundtrip.rs` | New: build → bytes → parse → assert-equal, tagged and untagged; hand-built byte arrays from real captures | 80 |
| `src/main.rs` | `mod frame;`, drop the placeholder `println!` | 2 |

## Crates

| Crate | Role | Notes |
|-------|------|-------|
| `etherparse` 0.20.x | Reference parser, VLAN header support | Actively maintained; cross-check only |
| `pnet` / `pnet_packet` 0.35.x | Raw sockets, packet types | Useful if targeting a real NIC later |
| `tun-rs` or `tunio` | TAP device creation (phase 3) | Prefer over the dormant `tun-tap` 0.1.x; re-check activity before committing |
| `tokio` | Async I/O over TAP fds (phase 3+) | |
| `smoltcp` | Optional real IP stack for test hosts | Not committed to yet |

## Testing approach

- **Pure-logic unit tests** (phase 1 & 2) — frame round-trip tests, later switch-core forwarding/learning tests over channels. Fast, no privileges needed.
- **Netns + veth integration tests** (phase 3+) — scripted, not manual. Catches bugs a mocked-port unit test can't: wrong length fields, a tag that should've been stripped on egress but wasn't. Becomes the project's real acceptance suite once TAP is in the loop.
- **Hardware-in-the-loop (optional, once frames are really tagged)** — once phase 3/4 produce real tagged traffic, run it through a physical managed switch with a couple of Linux boxes on the other end. Netns + veth is still same-kernel, same-driver on both sides; a real switch and separate hardware catch things that can't: a vendor's own 802.1Q quirks, real NIC/driver behavior, actual link timing. Not a phase gate — an escalation once the virtual suite is solid, most useful around phase 4 (trunk ports) where interop with a real switch is the whole point.

## Known risks

- **TAP creation needs `CAP_NET_ADMIN`.** Resolved: `sudo setcap cap_net_admin+ep target/debug/vlan-rs` once, so the switch binary itself never needs `sudo`. Namespace administration (creating netns, moving an interface into one) is a separate kernel privilege boundary that setcap on one binary can't cover — `scripts/netns-smoke-test.sh` still shells out to `sudo ip netns ...` for just that part.
- **Scope creep toward a full switch OS** (STP, LACP, SNMP) is a real temptation once the core loop works. Deliberately out of scope.
- **Guardrail:** hand-roll phase 1 before reaching for `etherparse`. The crate checks the hand-rolled parser; it doesn't replace the phase where the learning happens.

## Verification

| Phase | How we know it works |
|-------|----------------------|
| 1 — frame parser | `cargo test --test frame_roundtrip` passes against hand-built and real-captured byte arrays |
| 2 — switch core | Unit tests over in-process channel "ports" assert frames tagged for VLAN 10 never reach a port only in VLAN 20 |
| 3 — TAP + netns | `scripts/netns-smoke-test.sh` — `ping` between two network namespaces succeeds only through the switch's TAP ports |
| 4 — trunk ports | Two switch instances linked by a trunk correctly tag on egress / strip on ingress; cross-switch VLAN isolation holds. Optionally repeated against a real managed switch and physical Linux boxes (hardware-in-the-loop, above) |
| 5 — config & CLI | A TOML topology file reproduces a given port/VLAN layout on startup; live reconfig doesn't drop in-flight traffic |

## Open questions

- **Is the QinQ stretch goal worth scheduling explicitly**, or should it stay unscheduled until phases 1–5 are done?

### Resolved

- **Crate choice for TAP creation (phase 3):** re-checked at phase-3 start as planned. `tunio` (the plan's other candidate) is itself now dormant — last release 2022-06-26, ~96 downloads/90 days — while `tun-rs` shipped 2.8.8 on 2026-07-21 with 210k recent downloads. Went with **`tun-rs`**.
- **`CAP_NET_ADMIN` handling:** a capability grant on the built binary (`setcap cap_net_admin+ep`) — see Known risks, above.

# vlan-rs — stretch goals

Status: 4 of 6 done (as of 2026-08-26). This is the committed, durable version of the plan drafted interactively via the `blueprint` tool — see `docs/plan.md`'s own header for why that split exists.

All five planned phases (`docs/plan.md`) are done. `docs/plan.md`'s Stretch line names six unscheduled items: MAC aging, QinQ, loop guard, a scripted netns test harness, a small web dashboard, and `cargo-fuzz` on the parser. This doc sketches scope and effort for each and records what's actually been done.

## The six goals

### 1. Scripted test harness in CI ✅ done

Added a `smoke-tests` CI job (`.github/workflows/rust.yml`) that runs `scripts/netns-smoke-test.sh`, `scripts/trunk-smoke-test.sh`, and `scripts/config-reload-smoke-test.sh` for real — GitHub-hosted `ubuntu-latest` runners have passwordless `sudo` and netns/bridge support, so the phase 3–5 acceptance tests that previously only ever ran when a human ran them locally now run on every push/PR.

### 2. `cargo-fuzz` on the parser ✅ done

`fuzz/fuzz_targets/parse_frame.rs` feeds `EthernetFrame::parse` arbitrary bytes and, when parsing succeeds, round-trips the result through `write_into` too — both are the hand-rolled, bit-twiddling code most likely to have an edge case example tests miss. A `fuzz` CI job runs it for 60 bounded seconds per push/PR (not full-time fuzzing — a smoke check, not a fuzzing farm). Needs the nightly toolchain for libFuzzer instrumentation; `rust-toolchain.toml`'s stable pin for the main crate is untouched, since `fuzz/` is a separate, unlinked crate.

Manually verified locally: 46M+ executions in 60 seconds, zero crashes, coverage saturated quickly (75 edges).

### 3. MAC aging ✅ done

`MacTable` entries now carry `last_seen: Instant`, stamped by `learn` — supplied by the caller (`Switch::forward` takes a `now: Instant` parameter) rather than read internally, so aging stays unit-testable without real time passing. `Switch::age_out(max_age, now)` evicts anything not relearned within `max_age`; `daemon.rs` runs it from a `tokio::time::interval` every 30s (`MAC_AGE_SWEEP_INTERVAL`), evicting entries older than 300s (`MAC_MAX_AGE`) — matching real switches' default. Aging is based on *source* activity only, same as real hardware — a lookup as a destination never refreshes an entry's clock, tested explicitly (`lookups_never_refresh_an_entrys_age`).

### 4. Loop guard ✅ done

vlan-rs has no STP; a real loop in the topology causes an unbounded broadcast storm. Full STP is out of scope by design — this is a lighter self-loop-detection mechanism instead: `daemon.rs` broadcasts a probe (`Switch::build_loop_probe`) out every port every `LOOP_PROBE_INTERVAL` (5s), carrying a per-`Switch` random `probe_id` (`Switch::with_probe_id`/`Switch::new`, keyed via `RandomState`, no `rand` dependency). Probes are recognized by a reserved `EtherType` (`0x88B7`, `src/switch/loop_guard.rs`) and handled before any VLAN/tag processing — like real switches special-case BPDUs — so a probe looping back through a trunk with no native VLAN isn't incorrectly rejected by that trunk's own ingress validation. If a port ever receives *this switch's own* `probe_id` back, `Switch::block_port` shuts that port down (no forwarding in or out) until the topology changes; `daemon.rs` logs the transition. "Blocked" landed as switch-level state layered on top of `PortMode` (a `PortEntry { mode, blocked }`), not a new `PortMode` variant — a port's VLAN configuration and its loop-guard status are orthogonal, and folding them together would have meant every `PortMode` match arm handling a blocked case that has nothing to do with VLANs. Recovery is via config reload (SIGHUP): `Switch::add_port` clears any prior block, so a topology fix plus a reload un-sticks a blocked port; there's no separate operator "unblock" command.

### 5. Small web dashboard — not started

`SIGUSR1` already dumps per-port/VLAN counters to stderr (phase 5); a dashboard would add an HTTP listener serving the same data as JSON plus a small auto-refreshing page. Genuinely optional — the operator-facing job is already done. Open question: hand-rolled TCP+HTTP (matching the project's general dependency-light stance) or a real crate like `axum`.

### 6. QinQ — not started

Double-tagged frames (an outer S-VLAN tag, TPID `0x88a8`, around the existing single 802.1Q tag). Comparable in size to phase 4 (trunk ports) — `EthernetFrame` would need a second outer-tag field, `parse`/`write_into` would need to handle nesting, and trunk ports would need a "provider" mode. Explicitly out of scope since the first planning round; no concrete driving use case for it yet.

## Verification

| Goal | How we know it works |
|------|----------------------|
| CI harness | `smoke-tests` job passes on a real PR |
| `cargo-fuzz` | `fuzz` job passes (60s bounded run, zero crashes) on a real PR |
| MAC aging | `ages_out_stale_entries_but_keeps_fresh_ones`, `lookups_never_refresh_an_entrys_age` — a fake clock advanced past the threshold, no real time passing |
| Loop guard | `scripts/loop-guard-smoke-test.sh` (CI `smoke-tests` job): bridge a switch's two access ports directly to each other and confirm the log shows the loop detected and blocked, plus unit tests (`a_switchs_own_probe_looping_back_blocks_the_receiving_port`, `a_different_switchs_probe_does_not_block_anything`, `a_blocked_port_rejects_forward_calls`, `a_blocked_port_is_excluded_from_flooding`, `unblock_port_restores_normal_forwarding`, `re_adding_a_port_clears_a_block`, `a_probe_touches_no_counters`) |
| Web dashboard | Manual: curl the JSON endpoint and load the HTML page after generating traffic via a smoke test |
| QinQ | Round-trip tests for double-tagged frames, mirroring phase 1's single-tag approach |

## Open questions

- Web dashboard, QinQ are still unscheduled — pick up if/when there's a concrete reason to want one.
- Web dashboard: hand-rolled or a real HTTP crate — the one place a new dependency would be a bigger philosophy shift than `thiserror`/`serde` were.

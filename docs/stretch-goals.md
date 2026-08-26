# vlan-rs — stretch goals

Status: 2 of 6 done (as of 2026-08-26). This is the committed, durable version of the plan drafted interactively via the `blueprint` tool — see `docs/plan.md`'s own header for why that split exists.

All five planned phases (`docs/plan.md`) are done. `docs/plan.md`'s Stretch line names six unscheduled items: MAC aging, QinQ, loop guard, a scripted netns test harness, a small web dashboard, and `cargo-fuzz` on the parser. This doc sketches scope and effort for each and records what's actually been done.

## The six goals

### 1. Scripted test harness in CI ✅ done

Added a `smoke-tests` CI job (`.github/workflows/rust.yml`) that runs `scripts/netns-smoke-test.sh`, `scripts/trunk-smoke-test.sh`, and `scripts/config-reload-smoke-test.sh` for real — GitHub-hosted `ubuntu-latest` runners have passwordless `sudo` and netns/bridge support, so the phase 3–5 acceptance tests that previously only ever ran when a human ran them locally now run on every push/PR.

### 2. `cargo-fuzz` on the parser ✅ done

`fuzz/fuzz_targets/parse_frame.rs` feeds `EthernetFrame::parse` arbitrary bytes and, when parsing succeeds, round-trips the result through `write_into` too — both are the hand-rolled, bit-twiddling code most likely to have an edge case example tests miss. A `fuzz` CI job runs it for 60 bounded seconds per push/PR (not full-time fuzzing — a smoke check, not a fuzzing farm). Needs the nightly toolchain for libFuzzer instrumentation; `rust-toolchain.toml`'s stable pin for the main crate is untouched, since `fuzz/` is a separate, unlinked crate.

Manually verified locally: 46M+ executions in 60 seconds, zero crashes, coverage saturated quickly (75 edges).

### 3. MAC aging — not started

`MacTable` entries never expire — once learned, a `(Vlan, MAC)` → `PortId` mapping is permanent until that port is reassigned or removed. Real switches age out entries after ~300s of silence. Sketch: an `Entry { port, last_seen: Instant }`, `learn` stamping it, an `evict_older_than(max_age, now)` method taking an explicit clock (not reading it internally) so it stays unit-testable, and one more `tokio::select!` arm in `daemon.rs` — a `tokio::time::interval` ticking every ~30s.

### 4. Loop guard — not started

vlan-rs has no STP; a real loop in the topology causes an unbounded broadcast storm. Full STP is out of scope by design — this would be a lighter self-loop-detection mechanism instead: a per-`Switch` nonce sent periodically out every port, and if a port ever receives its own switch's nonce back, that port gets blocked. Needs a new port state beyond today's `PortMode::{Access, Trunk}` and a decision about where "blocked" lives (in `PortMode` itself, or as switch-level state layered on top) — worth a dedicated planning round if picked up.

### 5. Small web dashboard — not started

`SIGUSR1` already dumps per-port/VLAN counters to stderr (phase 5); a dashboard would add an HTTP listener serving the same data as JSON plus a small auto-refreshing page. Genuinely optional — the operator-facing job is already done. Open question: hand-rolled TCP+HTTP (matching the project's general dependency-light stance) or a real crate like `axum`.

### 6. QinQ — not started

Double-tagged frames (an outer S-VLAN tag, TPID `0x88a8`, around the existing single 802.1Q tag). Comparable in size to phase 4 (trunk ports) — `EthernetFrame` would need a second outer-tag field, `parse`/`write_into` would need to handle nesting, and trunk ports would need a "provider" mode. Explicitly out of scope since the first planning round; no concrete driving use case for it yet.

## Verification

| Goal | How we know it works |
|------|----------------------|
| CI harness | `smoke-tests` job passes on a real PR |
| `cargo-fuzz` | `fuzz` job passes (60s bounded run, zero crashes) on a real PR |
| MAC aging | Unit test: learn a MAC, advance a fake clock past the threshold, assert the lookup now misses |
| Loop guard | Bridge a switch's two trunk ports to each other and confirm no broadcast storm / the loop gets blocked |
| Web dashboard | Manual: curl the JSON endpoint and load the HTML page after generating traffic via a smoke test |
| QinQ | Round-trip tests for double-tagged frames, mirroring phase 1's single-tag approach |

## Open questions

- MAC aging, loop guard, web dashboard, QinQ are all unscheduled — pick up if/when there's a concrete reason to want one.
- Loop guard's "blocked port" state design isn't resolved here — needs its own planning round if picked up.
- Web dashboard: hand-rolled or a real HTTP crate — the one place a new dependency would be a bigger philosophy shift than `thiserror`/`serde` were.

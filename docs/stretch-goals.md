# vlan-rs — stretch goals

Status: 5 of 6 done (as of 2026-08-27). This is the committed, durable version of the plan drafted interactively via the `blueprint` tool — see `docs/plan.md`'s own header for why that split exists.

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

vlan-rs has no STP; a real loop in the topology causes an unbounded broadcast storm. Full STP is out of scope by design — this is a lighter self-loop-detection mechanism instead: `daemon.rs` broadcasts a probe (`Switch::build_loop_probe`) out every port every `LOOP_PROBE_INTERVAL` (5s), carrying a per-`Switch` random `probe_id` (`Switch::with_probe_id`/`Switch::new`, keyed via `RandomState`, no `rand` dependency). Probes are recognized by a reserved `EtherType` (`0x88B7`, `src/switch/loop_guard.rs`) and handled before any VLAN/tag processing — like real switches special-case BPDUs — so a probe looping back through a trunk with no native VLAN isn't incorrectly rejected by that trunk's own ingress validation. If a port ever receives *this switch's own* `probe_id` back, `Switch::block_port` shuts that port down (no forwarding in or out) until the topology changes; `daemon.rs` logs the transition.

**Scope limitation:** this only catches a *self*-loop — a cable or hub/bridge that connects two ports of the *same* switch instance back to each other, the way the smoke test below constructs one. A probe crossing to a neighboring vlan-rs switch is recognized as "not mine" and silently absorbed (`Ok(vec![])`) rather than flooded onward, so it never makes it back to its originator — meaning the most common storm-causing topology in practice, a loop formed by two switches and two links between them, is **not** detected by this mechanism. Closing that gap would need probes to be flooded rather than dropped when they're not recognized as this switch's own, which risks the probe outliving the loop that produced it; out of scope for this lightweight guard.

"Blocked" landed as switch-level state layered on top of `PortMode` (a `PortEntry { mode, blocked }`), not a new `PortMode` variant — a port's VLAN configuration and its loop-guard status are orthogonal, and folding them together would have meant every `PortMode` match arm handling a blocked case that has nothing to do with VLANs. Recovery is via config reload (SIGHUP): `Switch::add_port` clears any prior block, so a topology fix plus a reload un-sticks a blocked port; there's no separate operator "unblock" command.

### 5. Small web dashboard ✅ done

Opt-in via `--dashboard <bind-addr>` (e.g. `vlan-rs --dashboard 127.0.0.1:8080 tap0:10`). Serves the same per-port/per-VLAN counters `SIGUSR1` already dumps to stderr, plus each port's live `mode` (access/trunk, VLAN membership — `Switch::port_mode`, which `SIGUSR1` doesn't show) as JSON at `GET /api/counters`, and a small auto-refreshing HTML page at `GET /` (`src/dashboard/index.html` — vanilla JS, polls every 2s, no build step). Went with hand-rolled TCP+HTTP over a crate like `axum`: no keep-alive, no chunked encoding, no header parsing beyond the request line, `Connection: close` on every response. The only new dependency surface is two extra `tokio` features (`net`, `io-util`).

`Switch` is owned by a single task and never shared behind a lock (see `daemon::run`'s doc comment), so the dashboard's per-connection tasks (`src/dashboard.rs`) can't read it directly. A request for `/api/counters` instead sends a `oneshot` reply channel through an `mpsc` queue into `daemon::run`'s own `select!` loop — the same pattern `SIGUSR1` already uses conceptually (an external trigger asks the owning task to read its own state), just replacing "print to stderr" with "hand back a JSON string over a channel". A `SIGHUP` reload is transparent to the dashboard for the same reason: the listener task never holds a reference to `switch`, so the next request after a reload just sees the post-reload state.

`render_counters_json` lists every *registered* port (`Switch::port_ids`, added alongside `Switch::port_mode`), not just ports `all_port_counters` already has an entry for — otherwise a freshly-added or blocked-but-idle port would silently vanish from the dashboard instead of showing up as zero traffic (caught by a failing test while implementing this, not by inspection).

No new CI job needed: unlike every other stretch/phase acceptance test, `tests/dashboard.rs` needs neither `sudo` nor a real TAP device — `dashboard::serve` only ever talks to a channel, never a kernel interface, so it's tested with a real `TcpListener`/`TcpStream` on `127.0.0.1:0` under the existing plain `cargo test` step.

### 6. QinQ — not started

Double-tagged frames (an outer S-VLAN tag, TPID `0x88a8`, around the existing single 802.1Q tag). Comparable in size to phase 4 (trunk ports) — `EthernetFrame` would need a second outer-tag field, `parse`/`write_into` would need to handle nesting, and trunk ports would need a "provider" mode. Explicitly out of scope since the first planning round; no concrete driving use case for it yet.

## Verification

| Goal | How we know it works |
|------|----------------------|
| CI harness | `smoke-tests` job passes on a real PR |
| `cargo-fuzz` | `fuzz` job passes (60s bounded run, zero crashes) on a real PR |
| MAC aging | `ages_out_stale_entries_but_keeps_fresh_ones`, `lookups_never_refresh_an_entrys_age` — a fake clock advanced past the threshold, no real time passing |
| Loop guard | `scripts/loop-guard-smoke-test.sh` (CI `smoke-tests` job): bridge a switch's two access ports directly to each other (a *self*-loop — see the scope limitation above) and confirm the log shows the loop detected and blocked, plus unit tests (`a_switchs_own_probe_looping_back_blocks_the_receiving_port`, `a_different_switchs_probe_does_not_block_anything`, `a_zero_padded_probe_is_still_recognized`, `two_switches_get_different_probe_ids`, `a_blocked_port_rejects_forward_calls`, `a_blocked_port_is_excluded_from_flooding`, `unicast_to_a_mac_learned_on_a_since_blocked_port_does_not_egress_it`, `unblock_port_restores_normal_forwarding`, `re_adding_a_port_clears_a_block`, `a_probe_touches_no_counters`) |
| Web dashboard | `tests/dashboard.rs` (regular `cargo test`, no `sudo`/TAP needed): JSON rendering against a real `Switch` (`renders_an_access_ports_mode_and_counters`, `renders_a_trunk_ports_mode_with_sorted_allowed_vlans`, `renders_an_untagged_only_trunks_null_native`, `renders_a_blocked_ports_status`, `ports_and_vlans_are_sorted_by_id`), plus live HTTP over a real `TcpListener`/`TcpStream` (`serves_the_index_page`, `serves_counters_as_json_over_the_wire`, `unknown_path_is_404`, `non_get_method_is_405`, `multiple_concurrent_requests_all_get_answered`) |
| QinQ | Round-trip tests for double-tagged frames, mirroring phase 1's single-tag approach |

## Open questions

- Web dashboard, QinQ are still unscheduled — pick up if/when there's a concrete reason to want one.
- Web dashboard: hand-rolled or a real HTTP crate — the one place a new dependency would be a bigger philosophy shift than `thiserror`/`serde` were.

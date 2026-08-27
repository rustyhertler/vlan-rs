#!/usr/bin/env bash
# Stretch goal 4's acceptance test: a single vlan-rs switch with two access
# ports, bridged directly to each other (the kernel bridge stands in for a
# cable plugged from one port back into another on the same switch) — a
# real physical loop. Each port's own periodic probe (LOOP_PROBE_INTERVAL,
# 5s) crosses the bridge and arrives back at the switch on the *other*
# port, so both ports should end up blocked without ever forming a
# sustained broadcast storm.
#
# Same privilege setup as scripts/netns-smoke-test.sh — see that script's
# header for the sudo/setcap nuances, which apply here unchanged.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# `sudo ./script.sh` resets PATH and often $HOME, which can drop a
# rustup-installed cargo entirely. Fall back to the invoking user's cargo.
if ! command -v cargo >/dev/null 2>&1; then
  real_home="${SUDO_USER:+$(getent passwd "$SUDO_USER" | cut -d: -f6)}"
  real_home="${real_home:-$HOME}"
  [ -d "$real_home/.cargo/bin" ] && PATH="$real_home/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || {
  echo "error: cargo not found on PATH" >&2
  exit 1
}

BIN="target/debug/vlan-rs"
TAP_1="vlanrs-lp1"
TAP_2="vlanrs-lp2"
BRIDGE="vlanrs-loop"

LOG_DIR="$(mktemp -d /tmp/vlanrs-loop-guard-test.XXXXXX)"
LOG_FILE="$LOG_DIR/switch.log"
echo "switch log: $LOG_FILE"

SWITCH_PID=""

cleanup() {
  echo "--- cleanup ---"
  [ -n "$SWITCH_PID" ] && kill "$SWITCH_PID" 2>/dev/null || true
  # setup used `sudo ip link add`/`set` (the setcap path runs as a non-root
  # user), so a bare `ip link delete` here silently fails behind `|| true`
  # under the same privilege model, leaving the bridge/TAPs behind for the
  # next run to collide with ("File exists"). Match setup's privilege.
  sudo ip link delete "$BRIDGE" >/dev/null 2>&1 || true
  sudo ip link delete "$TAP_1" >/dev/null 2>&1 || true
  sudo ip link delete "$TAP_2" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "--- building ---"
cargo build --quiet

if [ "$(id -u)" -ne 0 ] && ! getcap "$BIN" 2>/dev/null | grep -q cap_net_admin; then
  echo "error: $BIN needs cap_net_admin. Run once:" >&2
  echo "  sudo setcap cap_net_admin+ep $BIN" >&2
  exit 1
fi

echo "--- starting switch ($TAP_1:10 $TAP_2:10) ---"
"$BIN" "$TAP_1:10" "$TAP_2:10" >"$LOG_FILE" 2>&1 &
SWITCH_PID=$!

echo "--- waiting for both TAP devices to appear ---"
for _ in $(seq 1 50); do
  kill -0 "$SWITCH_PID" 2>/dev/null || {
    echo "error: switch exited before both TAP devices appeared" >&2
    echo "--- switch log ---" >&2
    cat "$LOG_FILE" >&2
    exit 1
  }
  ip link show "$TAP_1" >/dev/null 2>&1 && ip link show "$TAP_2" >/dev/null 2>&1 && break
  sleep 0.1
done
for dev in "$TAP_1" "$TAP_2"; do
  ip link show "$dev" >/dev/null 2>&1 || { echo "error: $dev never appeared" >&2; exit 1; }
done

echo "--- bridging the two ports directly together (the loop) ---"
sudo ip link add name "$BRIDGE" type bridge
sudo sysctl -qw "net.ipv6.conf.$BRIDGE.disable_ipv6=1" 2>/dev/null || true
for dev in "$TAP_1" "$TAP_2"; do
  sudo sysctl -qw "net.ipv6.conf.$dev.disable_ipv6=1" 2>/dev/null || true
done
sudo ip link set "$TAP_1" master "$BRIDGE"
sudo ip link set "$TAP_2" master "$BRIDGE"
sudo ip link set "$TAP_1" up
sudo ip link set "$TAP_2" up
sudo ip link set "$BRIDGE" up

# Probes fire every LOOP_PROBE_INTERVAL (5s); give it a few rounds to be
# sure this isn't a race against the very first tick.
echo "--- waiting up to 20s for the loop guard to detect and block the loop ---"
detected=0
for _ in $(seq 1 40); do
  kill -0 "$SWITCH_PID" 2>/dev/null || {
    echo "error: switch exited before the loop was detected" >&2
    echo "--- switch log ---" >&2
    cat "$LOG_FILE" >&2
    exit 1
  }
  if grep -q "loop detected" "$LOG_FILE"; then
    detected=1
    break
  fi
  sleep 0.5
done

if [ "$detected" -ne 1 ]; then
  echo "FAIL: no loop detected within 20s" >&2
  echo "--- switch log ---" >&2
  cat "$LOG_FILE" >&2
  exit 1
fi

# grep -c counts log lines, which today happens to equal the number of
# distinct ports blocked (one "loop detected" line per port) — but that's
# incidental, not guaranteed by the log format, so derive the port count
# from the actual port identifiers logged rather than the line count.
blocked_ports="$(grep -o '^PortId([0-9]*): loop detected' "$LOG_FILE" | sort -u | wc -l)"

# The acceptance criterion (docs/design.md) is "no broadcast storm",
# not just "a log line appeared" — confirm traffic has actually plateaued,
# not merely that detection fired once. Port counters can't tell us this:
# a loop-guard probe never touches them even when it's *not* recognized
# and detection has failed (see `a_probe_touches_no_counters`), so on
# this bridge — which only ever carries probes, no real user traffic —
# they'd read 0 whether the loop guard is working or completely broken.
# Log growth is the real signal here: past the one-time "loop detected"
# transition per port, steady state produces no further output, so if
# detection had failed and the probes kept ricocheting/re-triggering,
# the log would still be growing.
log_lines() { wc -l <"$LOG_FILE"; }

before="$(log_lines)"
sleep 2
after="$(log_lines)"

if [ "$before" != "$after" ]; then
  echo "FAIL: switch log still growing after blocking ($before -> $after lines) — doesn't look like a settled, blocked loop" >&2
  echo "--- switch log ---" >&2
  cat "$LOG_FILE" >&2
  exit 1
fi

echo "PASS: loop guard blocked $blocked_ports port(s) after bridging $TAP_1 <-> $TAP_2 into a loop, log settled at $after lines"

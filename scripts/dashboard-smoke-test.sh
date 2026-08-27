#!/usr/bin/env bash
# Stretch goal 5's acceptance test: two Linux network namespaces connected
# through a real vlan-rs switch (same topology as netns-smoke-test.sh),
# with --dashboard also on — proving the HTTP layer sees real traffic
# through a real TAP device, not just the channel-only path
# tests/dashboard.rs already covers. ping between the namespaces, then
# assert the dashboard's own /api/counters shows nonzero frames in *and*
# out on both ports, plus that / and the 404/405 paths respond correctly.
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
NS1="vlanrs-dns1"
NS2="vlanrs-dns2"
TAP1="vlanrs-dtap1"
TAP2="vlanrs-dtap2"
IP1="10.10.1.1"
IP2="10.10.1.2"
DASHBOARD_ADDR="127.0.0.1:18080"

LOG_DIR="$(mktemp -d /tmp/vlanrs-dashboard-test.XXXXXX)"
LOG_FILE="$LOG_DIR/switch.log"
echo "switch log: $LOG_FILE"

SWITCH_PID=""

cleanup() {
  echo "--- cleanup ---"
  [ -n "$SWITCH_PID" ] && kill "$SWITCH_PID" 2>/dev/null || true
  sudo ip netns delete "$NS1" >/dev/null 2>&1 || true
  sudo ip netns delete "$NS2" >/dev/null 2>&1 || true
  sudo ip link delete "$TAP1" >/dev/null 2>&1 || true
  sudo ip link delete "$TAP2" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "--- building ---"
cargo build --quiet

if [ "$(id -u)" -ne 0 ] && ! getcap "$BIN" 2>/dev/null | grep -q cap_net_admin; then
  echo "error: $BIN needs cap_net_admin. Run once:" >&2
  echo "  sudo setcap cap_net_admin+ep $BIN" >&2
  exit 1
fi

echo "--- starting switch ($TAP1:10 $TAP2:10, dashboard on $DASHBOARD_ADDR) ---"
"$BIN" --dashboard "$DASHBOARD_ADDR" "$TAP1:10" "$TAP2:10" >"$LOG_FILE" 2>&1 &
SWITCH_PID=$!

echo "--- waiting for both TAP devices and the dashboard to come up ---"
for _ in $(seq 1 50); do
  kill -0 "$SWITCH_PID" 2>/dev/null || {
    echo "error: switch exited before startup finished" >&2
    cat "$LOG_FILE" >&2
    exit 1
  }
  ip link show "$TAP1" >/dev/null 2>&1 && ip link show "$TAP2" >/dev/null 2>&1 \
    && curl -s -o /dev/null "http://$DASHBOARD_ADDR/" && break
  sleep 0.1
done
ip link show "$TAP1" >/dev/null 2>&1 || { echo "error: $TAP1 never appeared" >&2; exit 1; }
ip link show "$TAP2" >/dev/null 2>&1 || { echo "error: $TAP2 never appeared" >&2; exit 1; }

echo "--- namespaces ---"
sudo ip netns add "$NS1"
sudo ip netns add "$NS2"

echo "--- moving TAP devices into namespaces ---"
sudo ip link set "$TAP1" netns "$NS1"
sudo ip link set "$TAP2" netns "$NS2"

echo "--- addressing + bringing interfaces up ---"
sudo ip netns exec "$NS1" ip addr add "$IP1/24" dev "$TAP1"
sudo ip netns exec "$NS1" ip link set "$TAP1" up
sudo ip netns exec "$NS1" ip link set lo up

sudo ip netns exec "$NS2" ip addr add "$IP2/24" dev "$TAP2"
sudo ip netns exec "$NS2" ip link set "$TAP2" up
sudo ip netns exec "$NS2" ip link set lo up

echo "--- checking / and error paths before any traffic ---"
index_status="$(curl -s -o /dev/null -w '%{http_code}' "http://$DASHBOARD_ADDR/")"
[ "$index_status" = "200" ] || { echo "error: GET / returned $index_status, want 200" >&2; exit 1; }
curl -s "http://$DASHBOARD_ADDR/" | grep -q "vlan-rs dashboard" \
  || { echo "error: GET / body missing the expected title" >&2; exit 1; }

not_found_status="$(curl -s -o /dev/null -w '%{http_code}' "http://$DASHBOARD_ADDR/nope")"
[ "$not_found_status" = "404" ] || { echo "error: GET /nope returned $not_found_status, want 404" >&2; exit 1; }

method_status="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://$DASHBOARD_ADDR/api/counters")"
[ "$method_status" = "405" ] || { echo "error: POST /api/counters returned $method_status, want 405" >&2; exit 1; }

echo "--- ping: $NS1 -> $NS2, across the switch ---"
sudo ip netns exec "$NS1" ping -c 3 -W 2 "$IP2"

echo "--- asserting the dashboard actually saw the traffic ---"
counters="$(curl -s "http://$DASHBOARD_ADDR/api/counters")"
echo "$counters"

# No jq dependency (matches this repo's scripts staying dependency-light).
# The "vlans" object has its own "frames_out" key too, so isolate the
# "ports" array first — otherwise the VLAN-level total would slip into
# this count as if it were a third port's value.
ports_json="$(sed -n 's/.*"ports":\[\(.*\)\],"vlans".*/\1/p' <<<"$counters")"
# Every per-port "frames_out":N with N > 0 is what we're checking for, on
# *both* ports: proof that a frame arrived on one real TAP device and was
# actually relayed out the other, not just recognized as a probe or
# dropped, and that the dashboard's oneshot/mpsc path to the live Switch
# is reporting real, current state rather than a fixed/stale snapshot.
frames_out_values="$(grep -o '"frames_out":[0-9]*' <<<"$ports_json" | cut -d: -f2)"
nonzero_count="$(grep -c -v '^0$' <<<"$frames_out_values" || true)"
if [ "$nonzero_count" -lt 2 ]; then
  echo "FAIL: expected both ports to show frames_out > 0, got: $frames_out_values" >&2
  exit 1
fi

echo "PASS: dashboard served real counters (frames_out: $frames_out_values) across a real TAP-backed ping"

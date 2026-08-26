#!/usr/bin/env bash
# Phase 4's acceptance test: two separate vlan-rs switch instances, each
# with an access-port host and a trunk port; the two trunk ports are
# bridged together (the kernel bridge stands in for "the wire" between two
# real switches). ping between the two hosts proves the trunk actually
# tags on egress and strips on ingress end to end, not just in unit tests.
# The trunk carries VLAN 10 with no native VLAN, so the host traffic is
# genuinely 802.1Q-tagged crossing the link — not accidentally untagged.
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
NS1="vlanrs-h1"
NS2="vlanrs-h2"
TAP_A_ACCESS="vlanrs-a-acc"
TAP_A_TRUNK="vlanrs-a-trk"
TAP_B_ACCESS="vlanrs-b-acc"
TAP_B_TRUNK="vlanrs-b-trk"
BRIDGE="vlanrs-link"
IP1="10.10.0.1"
IP2="10.10.0.2"

SWITCH_A_PID=""
SWITCH_B_PID=""

cleanup() {
  echo "--- cleanup ---"
  [ -n "$SWITCH_A_PID" ] && kill "$SWITCH_A_PID" 2>/dev/null || true
  [ -n "$SWITCH_B_PID" ] && kill "$SWITCH_B_PID" 2>/dev/null || true
  sudo ip netns delete "$NS1" >/dev/null 2>&1 || true
  sudo ip netns delete "$NS2" >/dev/null 2>&1 || true
  ip link delete "$BRIDGE" >/dev/null 2>&1 || true
  ip link delete "$TAP_A_ACCESS" >/dev/null 2>&1 || true
  ip link delete "$TAP_A_TRUNK" >/dev/null 2>&1 || true
  ip link delete "$TAP_B_ACCESS" >/dev/null 2>&1 || true
  ip link delete "$TAP_B_TRUNK" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "--- building ---"
cargo build --quiet

# Running this whole script as root already has whatever setcap would
# grant, so only require the capability bit when not already root.
if [ "$(id -u)" -ne 0 ] && ! getcap "$BIN" 2>/dev/null | grep -q cap_net_admin; then
  echo "error: $BIN needs cap_net_admin. Run once:" >&2
  echo "  sudo setcap cap_net_admin+ep $BIN" >&2
  exit 1
fi

echo "--- starting switch A ($TAP_A_ACCESS:10 $TAP_A_TRUNK:trunk:-:10,20) ---"
"$BIN" "$TAP_A_ACCESS:10" "$TAP_A_TRUNK:trunk:-:10,20" &
SWITCH_A_PID=$!

echo "--- starting switch B ($TAP_B_ACCESS:10 $TAP_B_TRUNK:trunk:-:10,20) ---"
"$BIN" "$TAP_B_ACCESS:10" "$TAP_B_TRUNK:trunk:-:10,20" &
SWITCH_B_PID=$!

echo "--- waiting for all four TAP devices to appear ---"
for _ in $(seq 1 50); do
  ip link show "$TAP_A_ACCESS" >/dev/null 2>&1 \
    && ip link show "$TAP_A_TRUNK" >/dev/null 2>&1 \
    && ip link show "$TAP_B_ACCESS" >/dev/null 2>&1 \
    && ip link show "$TAP_B_TRUNK" >/dev/null 2>&1 \
    && break
  sleep 0.1
done
for dev in "$TAP_A_ACCESS" "$TAP_A_TRUNK" "$TAP_B_ACCESS" "$TAP_B_TRUNK"; do
  ip link show "$dev" >/dev/null 2>&1 || { echo "error: $dev never appeared" >&2; exit 1; }
done

echo "--- bridging the two trunk ports together (the 'wire' between the switches) ---"
sudo ip link add name "$BRIDGE" type bridge
sudo ip link set "$TAP_A_TRUNK" master "$BRIDGE"
sudo ip link set "$TAP_B_TRUNK" master "$BRIDGE"
sudo ip link set "$TAP_A_TRUNK" up
sudo ip link set "$TAP_B_TRUNK" up
sudo ip link set "$BRIDGE" up

echo "--- namespaces ---"
sudo ip netns add "$NS1"
sudo ip netns add "$NS2"

echo "--- moving the access TAP devices into namespaces ---"
sudo ip link set "$TAP_A_ACCESS" netns "$NS1"
sudo ip link set "$TAP_B_ACCESS" netns "$NS2"

echo "--- addressing + bringing interfaces up ---"
sudo ip netns exec "$NS1" ip addr add "$IP1/24" dev "$TAP_A_ACCESS"
sudo ip netns exec "$NS1" ip link set "$TAP_A_ACCESS" up
sudo ip netns exec "$NS1" ip link set lo up

sudo ip netns exec "$NS2" ip addr add "$IP2/24" dev "$TAP_B_ACCESS"
sudo ip netns exec "$NS2" ip link set "$TAP_B_ACCESS" up
sudo ip netns exec "$NS2" ip link set lo up

echo "--- ping: $NS1 -> $NS2, across switch A's trunk, the bridge, and switch B's trunk ---"
sudo ip netns exec "$NS1" ping -c 3 -W 2 "$IP2"

echo "PASS: ping succeeded across two vlan-rs switches linked by a trunk"

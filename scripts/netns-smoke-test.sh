#!/usr/bin/env bash
# Phase 3's acceptance test: two Linux network namespaces, connected only
# through a real vlan-rs switch over two TAP devices, both on VLAN 10.
# `ping` between them proves the whole stack — frame parsing, the switch
# core, and real kernel I/O — actually works, not just compiles.
#
# One-time setup (needs root once, not on every run):
#   cargo build
#   sudo setcap cap_net_admin+ep target/debug/vlan-rs
#
# Namespace/interface admin (netns create, moving an interface into one)
# is a separate privilege boundary from what setcap grants a single binary
# — the kernel requires it regardless of how vlan-rs itself is invoked, so
# this script still needs `sudo` for those specific commands. vlan-rs itself
# is launched with no sudo at all, which is the point of the setcap step.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# `sudo ./script.sh` (as opposed to plain `./script.sh`, which only needs
# sudo for the individual ip/netns commands below) resets PATH and often
# $HOME, which can drop a rustup-installed cargo entirely. Fall back to the
# invoking user's cargo env in that case.
if ! command -v cargo >/dev/null 2>&1; then
  real_home="${SUDO_USER:+$(getent passwd "$SUDO_USER" | cut -d: -f6)}"
  real_home="${real_home:-$HOME}"
  # Prepend directly rather than sourcing ~/.cargo/env — that file builds
  # its PATH addition from $HOME, which under sudo is often root's, not
  # the invoking user's, defeating the whole point of resolving real_home.
  [ -d "$real_home/.cargo/bin" ] && PATH="$real_home/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || {
  echo "error: cargo not found on PATH" >&2
  exit 1
}

BIN="target/debug/vlan-rs"
NS1="vlanrs-ns1"
NS2="vlanrs-ns2"
TAP1="vlanrs-tap1"
TAP2="vlanrs-tap2"
IP1="10.10.0.1"
IP2="10.10.0.2"

SWITCH_PID=""

cleanup() {
  echo "--- cleanup ---"
  [ -n "$SWITCH_PID" ] && kill "$SWITCH_PID" 2>/dev/null || true
  sudo ip netns delete "$NS1" >/dev/null 2>&1 || true
  sudo ip netns delete "$NS2" >/dev/null 2>&1 || true
  ip link delete "$TAP1" >/dev/null 2>&1 || true
  ip link delete "$TAP2" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "--- building ---"
cargo build --quiet

# Running this whole script as root (rather than the recommended plain
# `./script.sh`, which relies on setcap and only needs sudo for the
# individual ip/netns commands below) already has whatever setcap would
# grant, so only require the capability bit when not already root.
if [ "$(id -u)" -ne 0 ] && ! getcap "$BIN" 2>/dev/null | grep -q cap_net_admin; then
  echo "error: $BIN needs cap_net_admin. Run once:" >&2
  echo "  sudo setcap cap_net_admin+ep $BIN" >&2
  exit 1
fi

echo "--- starting switch ($TAP1:10 $TAP2:10) ---"
"$BIN" "$TAP1:10" "$TAP2:10" &
SWITCH_PID=$!

echo "--- waiting for both TAP devices to appear ---"
for _ in $(seq 1 50); do
  ip link show "$TAP1" >/dev/null 2>&1 && ip link show "$TAP2" >/dev/null 2>&1 && break
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

echo "--- ping: $NS1 -> $NS2, across the switch ---"
sudo ip netns exec "$NS1" ping -c 3 -W 2 "$IP2"

echo "PASS: ping succeeded across the vlan-rs switch"

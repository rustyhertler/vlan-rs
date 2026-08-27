#!/usr/bin/env bash
# Bridges a real physical NIC to a vlan-rs TAP port for bench testing on
# real hardware — the "hardware-in-the-loop" escalation docs/design.md's
# Testing approach section describes, made concrete.
#
# Unlike scripts/*-smoke-test.sh, this isn't a self-contained CI test: it
# needs an actual second box (or a real client device) on the other end
# of the cable, so it's meant to be run by hand, once per physical port
# you want to hand to vlan-rs, on each box in your bench topology.
#
# Run this AFTER vlan-rs is already running and has opened the TAP device
# you're pointing it at (it polls briefly, but won't create the TAP
# itself), and BEFORE plugging in the physical link you actually want to
# test.
#
# Why a bridge, not a raw socket straight into vlan-rs: vlan-rs only
# knows how to open a TAP device (src/io/tap.rs), not a physical NIC
# directly. A default Linux bridge doesn't interpret 802.1Q tags — it
# just relays raw frames by MAC — so this works equally for a vlan-rs
# access port and a vlan-rs trunk port; vlan-rs is still the only thing
# in this setup that actually understands VLANs.
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 <physical-iface> <tap-name> [bridge-name]" >&2
  echo "  e.g.: $0 eth1 tap0                  # bridge name defaults to vlanrs-br-tap0" >&2
  echo "  e.g.: $0 eth1 tap0 vlanrs-br0" >&2
  exit 1
fi

PHYS="$1"
TAP="$2"
BRIDGE="${3:-vlanrs-br-$TAP}"

if [ "$(id -u)" -ne 0 ]; then
  echo "error: needs root (bridge/interface admin) — run with sudo" >&2
  exit 1
fi

echo "--- waiting up to 10s for $TAP to exist (start vlan-rs first) ---"
for _ in $(seq 1 20); do
  ip link show "$TAP" >/dev/null 2>&1 && break
  sleep 0.5
done
ip link show "$TAP" >/dev/null 2>&1 || {
  echo "error: $TAP still doesn't exist — is vlan-rs running with a port named $TAP?" >&2
  exit 1
}
ip link show "$PHYS" >/dev/null 2>&1 || {
  echo "error: no such interface $PHYS" >&2
  exit 1
}

echo "--- disabling NetworkManager on $PHYS, if present (best-effort) ---"
# NetworkManager (or systemd-networkd) will otherwise fight the bridge
# for DHCP/link-state on the physical interface once it's enslaved.
command -v nmcli >/dev/null 2>&1 && nmcli device set "$PHYS" managed no 2>/dev/null || true
ip addr flush dev "$PHYS" 2>/dev/null || true

echo "--- bridging $PHYS <-> $TAP via $BRIDGE ---"
ip link add name "$BRIDGE" type bridge 2>/dev/null || echo "  ($BRIDGE already exists, reusing)"
ip link set "$PHYS" master "$BRIDGE"
ip link set "$TAP" master "$BRIDGE"
ip link set "$PHYS" up
ip link set "$TAP" up
ip link set "$BRIDGE" up

echo "PASS: $PHYS is now vlan-rs's real-hardware backing for $TAP (via $BRIDGE)."
echo "      Plug the cable in now. Tear down with: ip link delete $BRIDGE"

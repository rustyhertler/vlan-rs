#!/usr/bin/env bash
# Phase 5's acceptance test: TOML config loading, SIGHUP live reload (a
# real TAP port torn down, a different real TAP port brought up, without
# restarting the process), and a SIGUSR1 counters dump.
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
CONFIG="$(mktemp /tmp/vlanrs-config-test.XXXXXX.toml)"
TAP1="vlanrs-cfg1"
TAP2="vlanrs-cfg2"
TAP3="vlanrs-cfg3"

SWITCH_PID=""

cleanup() {
  echo "--- cleanup ---"
  [ -n "$SWITCH_PID" ] && kill "$SWITCH_PID" 2>/dev/null || true
  rm -f "$CONFIG"
  for dev in "$TAP1" "$TAP2" "$TAP3"; do
    ip link delete "$dev" >/dev/null 2>&1 || true
  done
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

cat >"$CONFIG" <<EOF
[[port]]
name = "$TAP1"
mode = "access"
vlan = 10

[[port]]
name = "$TAP2"
mode = "access"
vlan = 10
EOF

echo "--- starting switch (--config $CONFIG) ---"
"$BIN" --config "$CONFIG" &
SWITCH_PID=$!

echo "--- waiting for $TAP1 and $TAP2 to appear ---"
for _ in $(seq 1 50); do
  ip link show "$TAP1" >/dev/null 2>&1 && ip link show "$TAP2" >/dev/null 2>&1 && break
  sleep 0.1
done
ip link show "$TAP1" >/dev/null 2>&1 || { echo "error: $TAP1 never appeared" >&2; exit 1; }
ip link show "$TAP2" >/dev/null 2>&1 || { echo "error: $TAP2 never appeared" >&2; exit 1; }
echo "PASS: initial config loaded, both ports up"

echo "--- rewriting config to drop $TAP2 and add $TAP3 (a trunk), sending SIGHUP ---"
cat >"$CONFIG" <<EOF
[[port]]
name = "$TAP1"
mode = "access"
vlan = 10

[[port]]
name = "$TAP3"
mode = "trunk"
native = 10
allowed = [10, 20]
EOF
kill -HUP "$SWITCH_PID"

echo "--- waiting for $TAP3 to appear and $TAP2 to disappear ---"
for _ in $(seq 1 50); do
  if ip link show "$TAP3" >/dev/null 2>&1 && ! ip link show "$TAP2" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
ip link show "$TAP3" >/dev/null 2>&1 || { echo "error: $TAP3 never appeared after reload" >&2; exit 1; }
if ip link show "$TAP2" >/dev/null 2>&1; then
  echo "error: $TAP2 still exists after reload should have dropped it" >&2
  exit 1
fi
echo "PASS: live reload tore down $TAP2 and brought up $TAP3, without restarting the process"

echo "--- SIGUSR1: requesting a counters dump (see the daemon's own stderr for the output) ---"
kill -USR1 "$SWITCH_PID"
sleep 0.2

echo "PASS: config loading, live reload, and counters dump all worked"

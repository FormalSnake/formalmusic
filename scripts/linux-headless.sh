#!/usr/bin/env bash
# Runs the Linux build inside its own headless sway, so a screenshot or a CPU
# reading never touches the desktop session. DISPLAY is dropped because GPUI
# falls back to X11 (the desktop's Xwayland) when WAYLAND_DISPLAY is empty.
#
#   scripts/linux-headless.sh <binary> <out.png> [seconds]
#
# Prints the socket, pid, RSS in KB and one CPU% sample per second, then
# writes the screenshot. Extra env (FORMALMUSIC_DEMO=1) passes through.
set -euo pipefail

bin=$1
out=$2
seconds=${3:-10}
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
unset DISPLAY

dir=$(mktemp -d)
printf 'output HEADLESS-1 resolution 1400x900\ndefault_border none\n' > "$dir/sway.conf"
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WAYLAND_DISPLAY= \
  nix shell nixpkgs#sway -c sway -d -c "$dir/sway.conf" > "$dir/sway.log" 2>&1 &
sway_pid=$!
trap 'kill "${app_pid:-}" "$sway_pid" 2>/dev/null || true; rm -rf "$dir"' EXIT

socket=
for _ in $(seq 1 60); do
  socket=$(grep -o "wayland display '[^']*'" "$dir/sway.log" | tail -1 | cut -d"'" -f2 || true)
  [ -n "$socket" ] && break
  sleep 0.5
done
[ -n "$socket" ] || { echo "sway did not start" >&2; grep -iE "error|fail|unable" "$dir/sway.log" | tail -8 >&2; exit 1; }
echo "socket=$socket"

WAYLAND_DISPLAY=$socket "$bin" > "$dir/app.log" 2>&1 &
app_pid=$!
sleep 4
kill -0 "$app_pid" 2>/dev/null || { echo "app exited" >&2; tail -20 "$dir/app.log" >&2; exit 1; }
echo "pid=$app_pid rss_kb=$(ps -o rss= -p "$app_pid")"
top -b -n "$((seconds + 1))" -d 1 -p "$app_pid" | awk -v pid="$app_pid" '$1 == pid { print $9 }' | tail -n "$seconds" | tr '\n' ' '
echo
WAYLAND_DISPLAY=$socket nix shell nixpkgs#grim -c grim "$out"
echo "shot=$out"
tail -5 "$dir/app.log"

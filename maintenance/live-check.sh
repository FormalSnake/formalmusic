#!/usr/bin/env bash
# Checks against the real YouTube Music, for the weekly maintenance run: an
# anonymous kopuzd of the pinned rev, then crates/core/tests/live.rs against
# it, which walks every screen's data path and plays an album and a mix
# muted. The checks that need an account skip themselves.
#
#   maintenance/live-check.sh
#
# kopuzd runs on its own socket, home, runtime, data and cache dirs under
# dbus-run-session, so it never meets the user's daemon, session, MPRIS name
# or scrobblers. Linux only. Exits non-zero when any check fails;
# FORMALMUSIC_MAINTENANCE_FORCE_FAIL=1 fails it on purpose after the real
# checks, for dry runs of the maintenance flow.
set -uo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
if [ -z "${IN_NIX_SHELL:-}" ]; then
  exec nix develop "$root" -c "$0" "$@"
fi
[ "$(uname)" = Linux ] || { echo "live-check: Linux only" >&2; exit 2; }

failed=0
summary=()
pass() { summary+=("ok    $1"); }
fail() { summary+=("FAIL  $1"); failed=1; }

dir=$(mktemp -d -t formalmusic-live-check.XXXXXX)
mkdir -p "$dir"/{runtime,home,config,data,cache}
chmod 700 "$dir/runtime"
sock=$dir/runtime/kopuzd.sock

cleanup() {
  [ -n "${daemon_pid:-}" ] && kill "$daemon_pid" 2>/dev/null && wait "$daemon_pid" 2>/dev/null
  rm -rf "$dir"
}
trap cleanup EXIT

# The dev shell puts the kopuzd of the pinned rev on PATH.
if ! kopuzd=$(command -v kopuzd); then
  fail "no kopuzd on PATH"
else
  env -u DISPLAY -u WAYLAND_DISPLAY \
    HOME="$dir/home" XDG_RUNTIME_DIR="$dir/runtime" XDG_CONFIG_HOME="$dir/config" \
    XDG_DATA_HOME="$dir/data" XDG_CACHE_HOME="$dir/cache" \
    dbus-run-session -- "$kopuzd" --socket "$sock" --db-path "$dir/home/kopuz.db" \
    > "$dir/daemon.log" 2>&1 &
  daemon_pid=$!
  for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.2; done
  if [ -S "$sock" ]; then
    pass "kopuzd on a private socket"
  else
    fail "kopuzd did not open its socket"
  fi
fi

if [ -S "$sock" ]; then
  if (cd "$root" && FORMALMUSIC_SOCKET="$sock" \
    cargo test -q -p formalmusic-core --test live -- --ignored --test-threads 1 --nocapture) \
    > "$dir/live.log" 2>&1; then
    pass "live tests: $(rg -o '[0-9]+ passed' "$dir/live.log" | awk '{s += $1} END {print s}') passed"
  else
    fail "live tests"
    echo "--- cargo test -p formalmusic-core --test live -- --ignored (failures)"
    rg -A 12 '^failures:|panicked at' "$dir/live.log" | head -60
  fi
  rg '^skipped' "$dir/live.log" | sed 's/^/      /'
fi
if [ "$failed" = 1 ] && [ -f "$dir/daemon.log" ]; then
  echo "--- kopuzd log (last 30 lines)"
  tail -30 "$dir/daemon.log"
fi

if [ "${FORMALMUSIC_MAINTENANCE_FORCE_FAIL:-}" = 1 ]; then
  fail "forced failure (FORMALMUSIC_MAINTENANCE_FORCE_FAIL=1)"
fi

echo "--- live-check"
printf '%s\n' "${summary[@]}"
exit "$failed"

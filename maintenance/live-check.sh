#!/usr/bin/env bash
# Checks against the real YouTube Music, for the weekly maintenance run:
# a daemon of this checkout plays 10 s of a fixed track into the null sink
# and fetches Home, Search and an album over its socket, then the innertube
# live tests run, renderer keys against the fixtures included.
#
#   maintenance/live-check.sh
#
# The daemon runs anonymous on its own socket, runtime, state, config and
# cache dirs under dbus-run-session, so it never meets the user's daemon,
# session, MPRIS name or scrobblers. Linux only. Exits non-zero when any
# check fails; FORMALMUSIC_MAINTENANCE_FORCE_FAIL=1 fails it on purpose after
# the real checks, for dry runs of the maintenance flow.
set -uo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
if [ -z "${IN_NIX_SHELL:-}" ]; then
  exec nix develop "$root" -c "$0" "$@"
fi
[ "$(uname)" = Linux ] || { echo "live-check: Linux only" >&2; exit 2; }

track=IluRBvnYMoY
album=MPREb_K8qWMWVqXGi
failed=0
summary=()
pass() { summary+=("ok    $1"); }
fail() { summary+=("FAIL  $1"); failed=1; }

dir=$(mktemp -d -t formalmusic-live-check.XXXXXX)
mkdir -p "$dir"/{runtime,state,config/formalmusic,cache}
chmod 700 "$dir/runtime"
# History reports are off even though the daemon is anonymous, so nothing
# this run plays could land in an account.
printf '{ "reportHistory": false }\n' > "$dir/config/formalmusic/daemon.json"
sock=$dir/runtime/formalmusicd.sock

cleanup() {
  [ -n "${SOCK_PID:-}" ] && kill "$SOCK_PID" 2>/dev/null
  [ -n "${daemon_pid:-}" ] && kill "$daemon_pid" 2>/dev/null && wait "$daemon_pid" 2>/dev/null
  rm -rf "$dir"
}
trap cleanup EXIT

if ! (cd "$root" && cargo build -q -p formalmusicd); then
  fail "build formalmusicd"
else
  env -u DISPLAY -u WAYLAND_DISPLAY \
    XDG_RUNTIME_DIR="$dir/runtime" FORMALMUSIC_SOCKET="$sock" \
    XDG_STATE_HOME="$dir/state" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" \
    FORMALMUSIC_AUDIO=null \
    dbus-run-session -- "$root/target/debug/formalmusicd" > "$dir/daemon.log" 2>&1 &
  daemon_pid=$!
  for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.2; done
fi

next_id=0
# Sends one command on the open connection and prints the response's `ok`
# value, or returns 1 on an error reply or after `wait` seconds.
call() {
  local wait=${2:-30} line
  next_id=$((next_id + 1))
  jq -cn --argjson id "$next_id" --argjson cmd "$1" '{id: $id} + $cmd' >&"${SOCK[1]}"
  while IFS= read -r -t "$wait" line <&"${SOCK[0]}"; do
    if jq -e --argjson id "$next_id" '.type == "response" and .id == $id' <<<"$line" >/dev/null; then
      jq -ce '.ok // error(.err | tostring)' <<<"$line" 2>&1 || { echo "$line" >&2; return 1; }
      return 0
    fi
  done
  echo "no answer within ${wait}s" >&2
  return 1
}

if [ -S "$sock" ]; then
  coproc SOCK { socat - "UNIX-CONNECT:$sock"; }
  if hello=$(call '{"cmd":"hello","args":{"protocol":2}}' 10); then
    pass "daemon $(jq -r .data.version <<<"$hello") on a private socket"

    if r=$(call '{"cmd":"browse","args":{"target":{"kind":"home"}}}') &&
      n=$(jq '.data.sections | length' <<<"$r") && [ "$n" -ge 2 ]; then
      pass "home: $n shelves"
    else
      fail "home: ${r:-no reply}"
    fi

    if r=$(call '{"cmd":"search","args":{"query":"daft punk","filter":null}}') &&
      n=$(jq '[.data.sections[].items[]] | length' <<<"$r") && [ "$n" -ge 10 ]; then
      pass "search: $n results"
    else
      fail "search: ${r:-no reply}"
    fi

    if r=$(call "{\"cmd\":\"browse\",\"args\":{\"target\":{\"kind\":\"album\",\"id\":\"$album\"}}}") &&
      n=$(jq '[.data.sections[0].items[] | select(.kind == "track")] | length' <<<"$r") && [ "$n" -ge 10 ]; then
      pass "album: $n tracks"
    else
      fail "album: ${r:-no reply}"
    fi

    play="{\"cmd\":\"play\",\"args\":{\"source\":{\"kind\":\"radio\",\"video_id\":\"$track\"},\"start_index\":0,\"shuffle\":false,\"radio\":false}}"
    if call "$play" 60 >/dev/null; then
      state=
      for _ in $(seq 1 120); do
        state=$(call '{"cmd":"player_state"}' 10) || break
        [ "$(jq '.data.position_ms' <<<"$state")" -ge 10000 ] && break
        sleep 1
      done
      if jq -e '.data.status == "playing" and .data.position_ms >= 10000' <<<"$state" >/dev/null 2>&1; then
        pass "playback: $(jq -r '"\(.data.playing_id) \(.data.stream // "?") at \(.data.position_ms / 1000 | floor) s"' <<<"$state")"
      else
        fail "playback: did not reach 10 s, last state $(jq -c '.data | {status, position_ms, playing_id}' <<<"$state" 2>/dev/null)"
      fi
    else
      fail "playback: play was refused"
    fi
  else
    fail "daemon did not answer hello"
  fi
else
  [ -n "${daemon_pid:-}" ] && fail "daemon did not open its socket"
fi
if [ "$failed" = 1 ] && [ -f "$dir/daemon.log" ]; then
  echo "--- daemon log (last 30 lines)"
  tail -30 "$dir/daemon.log"
fi

if (cd "$root" && cargo test -q -p formalmusic-innertube -- --ignored --nocapture) > "$dir/innertube.log" 2>&1; then
  pass "innertube live tests: $(rg -o '[0-9]+ passed' "$dir/innertube.log" | awk '{s += $1} END {print s}') passed"
else
  fail "innertube live tests"
  echo "--- cargo test -p formalmusic-innertube -- --ignored (failures)"
  rg -A 12 '^failures:|panicked at' "$dir/innertube.log" | head -60
fi
rg '^(home|search_all|album|player): new' "$dir/innertube.log" | sed 's/^/      renderer keys /'

if [ "${FORMALMUSIC_MAINTENANCE_FORCE_FAIL:-}" = 1 ]; then
  fail "forced failure (FORMALMUSIC_MAINTENANCE_FORCE_FAIL=1)"
fi

echo "--- live-check"
printf '%s\n' "${summary[@]}"
exit "$failed"

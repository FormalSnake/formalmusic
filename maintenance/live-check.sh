#!/usr/bin/env bash
# The pinned kopuzd against live YouTube Music, isolated from the owner's: its own
# database, settings, caches and socket under a temp dir, an anonymous YouTube Music source.
# Asks over the socket for Home, a search, an album and ten seconds of playback.
#
#   maintenance/live-check.sh              # kopuzd from PATH (nix develop has it)
#   KOPUZD=/path/to/kopuzd maintenance/live-check.sh
#
# FORMALMUSIC_MAINTENANCE_FORCE_FAIL=1 fails the playback check on purpose, for the
# weekly run's dry run.
set -uo pipefail

kopuzd=${KOPUZD:-kopuzd}
command -v grpcurl >/dev/null || { echo "grpcurl is not on PATH"; exit 2; }
work=$(mktemp -d)
sock=$work/kopuzd.sock
log=$work/kopuzd.log
pid=

cleanup() {
  [ -n "$pid" ] && kill "$pid" 2>/dev/null
  wait 2>/dev/null
  rm -rf -- "$work"
}
trap cleanup EXIT

# Kopuz keeps settings, credentials and caches under HOME and the XDG dirs besides the database.
mkdir -p "$work/home"
HOME=$work/home XDG_DATA_HOME=$work/home/data XDG_CONFIG_HOME=$work/home/config \
  XDG_CACHE_HOME=$work/home/cache XDG_STATE_HOME=$work/home/state \
  "$kopuzd" --socket "$sock" --db-path "$work/library.db" >"$log" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  [ -S "$sock" ] && break
  sleep 0.2
done
[ -S "$sock" ] || { echo "kopuzd did not open its socket"; tail -n 40 "$log"; exit 1; }

rpc() { grpcurl -max-time 60 -unix -plaintext -d "$2" "$sock" "kopuz.v1.Kopuz/$1"; }

failed=0
check() {
  local name=$1 ok=$2
  if [ "$ok" = 1 ]; then
    summary+="ok    $name"$'\n'
  else
    summary+="FAIL  $name"$'\n'
    failed=1
  fi
}
summary=

source_id=$(rpc UpsertSource '{"name":"YouTube Music","service":"ytmusic","values":[{"key":"auth_method","value":"anonymous"}]}' | jq -r .id)
rpc SelectSource "{\"id\":\"$source_id\"}" >/dev/null
check "anonymous source $source_id" "$([ -n "$source_id" ] && [ "$source_id" != null ] && echo 1)"

home=$(rpc GetCatalogDetail '{"kind":"CATALOG_ITEM_PAGE","id":"FEmusic_home"}')
shelves=$(jq '.shelves | length' <<<"$home" 2>/dev/null || echo 0)
check "home: $shelves shelves" "$([ "${shelves:-0}" -gt 0 ] && echo 1)"

search=$(rpc Search '{"query":"daft punk get lucky","filter":"songs"}')
key=$(jq -r '[(.tracks[]?, .shelves[]?.items[]?.track) | .key // empty][0] // empty' <<<"$search")
check "search: first song $key" "$([ -n "$key" ] && echo 1)"

album=$(jq -r '[(.tracks[]?, .shelves[]?.items[]?.track) | .albumId // empty][0] // empty' <<<"$search")
if [ -n "$album" ]; then
  tracks=$(rpc GetCatalogDetail "{\"kind\":\"CATALOG_ITEM_ALBUM\",\"id\":\"$album\"}" | jq '.tracks | length')
fi
check "album $album: ${tracks:-0} tracks" "$([ "${tracks:-0}" -gt 0 ] && echo 1)"

rpc SetQueue "{\"mode\":\"QUEUE_MODE_REPLACE\",\"context\":{\"tracks\":{\"keys\":[\"$key\"]}},\"startIndex\":0}" >/dev/null
sleep 12
state=$(rpc GetPlayerState '{}')
position=$(jq -r '((.position.ms // 0 | tonumber) + (if .position.playing then (.nowMs | tonumber) - (.position.atMs | tonumber) else 0 end))' <<<"$state")
[ "${FORMALMUSIC_MAINTENANCE_FORCE_FAIL:-0}" = 1 ] && position=0
check "playback: $(jq -r .phase <<<"$state") at ${position} ms" "$([ "$position" -gt 5000 ] && echo 1)"

if [ "$failed" = 1 ]; then
  echo "--- kopuzd log (tail)"
  tail -n 40 "$log"
fi
echo "--- live check"
printf '%s' "$summary"
exit "$failed"

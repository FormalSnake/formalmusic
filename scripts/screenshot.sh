#!/usr/bin/env bash
# One PNG of the demo data with animations jumped to their end.
#
#   scripts/screenshot.sh [out.png] [scene]     default docs/images/formalmusic.png
#
# Scenes: home (default), album, artist, playlist, explore, library, search,
# suggest, queue, lyrics, lyrics-duet, related, signin, menu, collapsed.
#
# macOS renders the frame offscreen with Metal (the `screenshot` feature);
# Linux runs the release build inside scripts/linux-headless.sh's own sway.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/docs/images/formalmusic.png}
scene=${2:-home}
# The sign-in screen shows itself when nobody is signed in.
[ "$scene" = signin ] && export FORMALMUSIC_DEMO_SIGNED_OUT=1
mkdir -p "$(dirname "$out")"

if [ "$(uname)" = Darwin ]; then
  FORMALMUSIC_DEMO=1 GPUIX_BACKGROUND=1 FORMALMUSIC_SCREENSHOT=$out FORMALMUSIC_SCREENSHOT_SCENE=$scene \
    cargo run --manifest-path "$root/Cargo.toml" -p formalmusic --release --features screenshot,demo-fixtures
else
  cargo build --manifest-path "$root/Cargo.toml" -p formalmusic --release --features demo-fixtures
  FORMALMUSIC_DEMO=1 FORMALMUSIC_STILL=1 "$root/scripts/linux-headless.sh" "$root/target/release/formalmusic" "$out" 3
fi

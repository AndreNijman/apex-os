#!/bin/bash
# Render a preview GIF of the Rime OS boot splash.
#   render-preview.sh <theme-dir> <out.gif> [render-preview.py options]
# A thin wrapper kept for the documented command: the renderer is
# render-preview.py, a frame-exact simulation of rime-os.script on plymouth's
# script plugin (see its header). For the 60 fps MP4, the contact sheet and the
# per-frame metrics, run render-preview.py directly with an output directory.
set -euo pipefail
DIR=$1; OUT=$2; shift 2
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
python3 "$(dirname "$0")/render-preview.py" "$DIR" "$TMP" --seconds 5 --no-mp4 --gif "$OUT" "$@"
echo "wrote $OUT"

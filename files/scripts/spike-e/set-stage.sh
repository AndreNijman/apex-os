#!/bin/bash
# Spike E — set the one-word control "stage" on the payload disk (host side),
# between VM boots. Also collects any /payload/out artifacts to the host OUT_DIR.
#
# Run: sudo bash set-stage.sh <stage>
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
need_root
STAGE="${1:?usage: set-stage.sh <stage>}"

PMNT="$SPIKE_WORK/mnt/payload"
mkdir -p "$PMNT" "$OUT_DIR"
LOOP="$(losetup --show -f "$PAYLOAD_IMG")"
cleanup() { umount "$PMNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true; }
trap cleanup EXIT
mount "$LOOP" "$PMNT"
echo "$STAGE" > "$PMNT/stage"
# sync artifacts back to host
cp -a "$PMNT/out/." "$OUT_DIR/" 2>/dev/null || true
sync
echo "stage set to: $STAGE"

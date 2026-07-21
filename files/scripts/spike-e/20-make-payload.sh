#!/bin/bash
# Spike E stage 20 — build the payload disk (attached as /dev/vdb in the VM).
#
# Carries into the VM: the fedora-bootc image tar (so the install needs no
# network), the install script, and the partition UUIDs. Receives back: the
# per-stage output artifacts under /payload/out. Also holds the one-word
# "stage" control file the host rewrites between boots.
#
# Run: sudo bash 20-make-payload.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
need_root

[ -f "$BOOTC_IMAGE_TAR" ] || die "missing $BOOTC_IMAGE_TAR (run: podman save $BOOTC_IMAGE -o $BOOTC_IMAGE_TAR)"

PMNT="$SPIKE_WORK/mnt/payload"
mkdir -p "$PMNT"

log "Creating payload disk $PAYLOAD_IMG ($PAYLOAD_SIZE)"
rm -f "$PAYLOAD_IMG"
truncate -s "$PAYLOAD_SIZE" "$PAYLOAD_IMG"
mkfs.ext4 -q -L payload "$PAYLOAD_IMG"

LOOP="$(losetup --show -f "$PAYLOAD_IMG")"
cleanup() { umount "$PMNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true; }
trap cleanup EXIT
mount "$LOOP" "$PMNT"

log "Populating payload"
cp "$BOOTC_IMAGE_TAR" "$PMNT/fedora-bootc-43.tar"
install -m0755 "$HERE/guest/spike-install.sh" "$PMNT/spike-install.sh"
cp "$SPIKE_WORK/uuids.env" "$PMNT/uuids.env"
mkdir -p "$PMNT/out"
echo "setup" > "$PMNT/stage"

ls -la "$PMNT"
sync
umount "$PMNT"; losetup -d "$LOOP"; trap - EXIT
# qemu runs unprivileged; hand the image back to the invoking user.
chown "${SUDO_USER:-root}:${SUDO_USER:-root}" "$PAYLOAD_IMG" 2>/dev/null || true
log "stage 20 done"

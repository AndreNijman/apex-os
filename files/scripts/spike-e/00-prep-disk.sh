#!/bin/bash
# Spike E stage 00 — create the dual-boot VM disk.
#
# Produces a GPT disk with three partitions on a shared layout that mirrors the
# real ThinkPad L16 target (a single shared ESP + an incumbent OS + free space
# for APEX-OS):
#   p1  ESP            FAT32   (shared EFI System Partition)
#   p2  incumbent root ext4    (Alpine, the fake incumbent)
#   p3  apex root      btrfs   (empty; bootc install target)
#
# Run: sudo bash 00-prep-disk.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
need_root

mkdir -p "$SPIKE_WORK"

log "Creating sparse raw disk $DISK_IMG ($DISK_SIZE)"
rm -f "$DISK_IMG"
truncate -s "$DISK_SIZE" "$DISK_IMG"

log "Writing GPT partition table with sgdisk"
sgdisk --zap-all "$DISK_IMG"
sgdisk \
  -n 1:0:+"$ESP_SIZE"        -t 1:ef00 -c 1:"EFI System Partition" \
  -n 2:0:+"$INCUMBENT_SIZE"  -t 2:8304 -c 2:"incumbent-root" \
  -n 3:0:0                   -t 3:8300 -c 3:"apex-root" \
  "$DISK_IMG"
sgdisk -p "$DISK_IMG"

log "Attaching loop device (partscan)"
LOOP="$(losetup --show -f -P "$DISK_IMG")"
echo "loop=$LOOP"
cleanup() { losetup -d "$LOOP" 2>/dev/null || true; }
trap cleanup EXIT
# settle
for _ in $(seq 1 10); do [ -b "${LOOP}p3" ] && break; sleep 0.3; done

log "Formatting filesystems"
mkfs.fat  -F32 -n APEXESP "${LOOP}p1"
mkfs.ext4 -q -L alpineroot "${LOOP}p2"
mkfs.btrfs -q -f -L apexroot "${LOOP}p3"   # empty; bootc install target

log "Capturing partition UUIDs -> $SPIKE_WORK/uuids.env"
ESP_UUID="$(blkid -s UUID -o value "${LOOP}p1")"        # FAT serial (short)
ESP_PARTUUID="$(blkid -s PARTUUID -o value "${LOOP}p1")"
INC_UUID="$(blkid -s UUID -o value "${LOOP}p2")"
APEX_UUID="$(blkid -s UUID -o value "${LOOP}p3")"
cat > "$SPIKE_WORK/uuids.env" <<EOF
ESP_UUID=$ESP_UUID
ESP_PARTUUID=$ESP_PARTUUID
INC_UUID=$INC_UUID
APEX_UUID=$APEX_UUID
EOF
cat "$SPIKE_WORK/uuids.env"

# qemu runs unprivileged; hand the image back to the invoking user.
chown "${SUDO_USER:-root}:${SUDO_USER:-root}" "$DISK_IMG" "$SPIKE_WORK/uuids.env" 2>/dev/null || true

log "stage 00 done"

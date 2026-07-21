#!/bin/bash
# Spike E — end-to-end driver. Documents the canonical stage sequence and runs
# it unattended. Individual stages can also be run by hand (see README).
#
# This is the reusable template for the REAL installs: the same before/install/
# after/verify choreography applies on the L16 and Katana (swap the incumbent
# build for "the machine as it already is", and run the install stage from a
# live USB instead of this VM).
#
# Run: sudo bash 99-run-all.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
need_root
RUNAS="${SUDO_USER:-root}"

vm()  { sudo -u "$RUNAS" bash "$HERE/30-run-vm.sh" "$@"; }
stg() { bash "$HERE/set-stage.sh" "$1"; }

log "STAGE 00 — prep disk";            bash "$HERE/00-prep-disk.sh"
log "STAGE 10 — build incumbent";      bash "$HERE/10-build-incumbent.sh"
log "STAGE 20 — build payload";        bash "$HERE/20-make-payload.sh"

# Extract host-side kernel/initramfs for the direct-kernel setup boot.
LOOP="$(losetup --show -f -P "$DISK_IMG")"; sleep 1
mkdir -p "$SPIKE_WORK/boot" "$SPIKE_WORK/mnt/esp2"
mount "${LOOP}p1" "$SPIKE_WORK/mnt/esp2"
cp "$SPIKE_WORK/mnt/esp2/EFI/alpine/vmlinuz-lts"   "$SPIKE_WORK/boot/"
cp "$SPIKE_WORK/mnt/esp2/EFI/alpine/initramfs-lts" "$SPIKE_WORK/boot/"
umount "$SPIKE_WORK/mnt/esp2"; losetup -d "$LOOP"
chown -R "$RUNAS:$RUNAS" "$SPIKE_WORK/boot"
rm -f "$OVMF_VARS"

log "BOOT setup — create persistent EFISTUB NVRAM entry (direct-kernel)"
stg setup;            DIRECT_KERNEL=1 NIC=off vm setup 120

log "BOOT before — record BEFORE state (boots via NVRAM entry = real test)"
stg before;           vm before 150

log "BOOT install — bootc install to-filesystem"
stg install;          vm install 600

log "BOOT after — record AFTER state; set BootNext=bootc, BootOrder=alpine,bootc"
stg after;            vm after 200

log "BOOT bootc — prove the bootc OS boots (BootNext); it will not power off"
stg verify-incumbent; vm bootc 120

log "BOOT verify-incumbent — prove the incumbent STILL boots via its NVRAM entry"
vm verify-incumbent 150

# Collect all artifacts to host OUT_DIR.
stg after >/dev/null
log "DONE — artifacts in $OUT_DIR"
ls -la "$OUT_DIR" || true

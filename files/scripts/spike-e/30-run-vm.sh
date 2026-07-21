#!/bin/bash
# Spike E stage 30 — launch the VM for one boot under OVMF.
#
# NVRAM (OVMF_VARS) is created once and then PERSISTS across invocations so that
# BootOrder / boot entries survive between stages — this is what lets the spike
# observe whether bootc/bootupd clobbered the incumbent's NVRAM entries.
#
# The disk is /dev/vda (ESP=vda1, alpine=vda2, apex=vda3); payload is /dev/vdb.
# Guest stages end in `poweroff`, so qemu exits on its own; `timeout` is a
# backstop (and is the normal exit for the bootc/incumbent boot-tests, which
# don't power off).
#
# Run: bash 30-run-vm.sh <name> <timeout_secs>
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
NAME="${1:?usage: 30-run-vm.sh <name> <timeout_secs>}"
TIMEOUT="${2:-180}"
LOG="$SPIKE_WORK/serial-$NAME.log"
. "$SPIKE_WORK/uuids.env"

# DIRECT_KERNEL=1 boots Alpine straight from host-side kernel/initramfs copies
# (still under OVMF, so efivarfs is available). Used ONLY for stage=setup, to
# deterministically get into the guest to CREATE the persistent NVRAM entry.
# Every later stage boots normally via that NVRAM entry — which is the actual
# thing under test.
DK_ARGS=()
if [ "${DIRECT_KERNEL:-0}" = "1" ]; then
  DK_ARGS=(-kernel "$SPIKE_WORK/boot/vmlinuz-lts" \
           -initrd "$SPIKE_WORK/boot/initramfs-lts" \
           -append "root=UUID=$INC_UUID rw rootfstype=ext4 console=tty0 console=ttyS0,115200")
fi

# Create persistent NVRAM once.
if [ ! -f "$OVMF_VARS" ]; then
  log "Creating persistent NVRAM $OVMF_VARS from template"
  cp "$OVMF_VARS_TEMPLATE" "$OVMF_VARS"
fi

log "Launching VM '$NAME' (timeout ${TIMEOUT}s, NIC=${NIC:-on}) -> $LOG"
NIC_ARGS=(-netdev user,id=n0 -device virtio-net-pci,netdev=n0)
[ "${NIC:-on}" = "off" ] && NIC_ARGS=()
set +e
timeout --foreground "$TIMEOUT" qemu-system-x86_64 \
  -enable-kvm -machine q35 -cpu host -smp "$VM_CPUS" -m "$VM_RAM" \
  -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
  -drive if=pflash,format=raw,file="$OVMF_VARS" \
  -drive file="$DISK_IMG",format=raw,if=virtio,cache=writeback \
  -drive file="$PAYLOAD_IMG",format=raw,if=virtio,cache=writeback \
  "${NIC_ARGS[@]}" \
  "${DK_ARGS[@]}" \
  -display none -serial "file:$LOG" \
  -no-reboot
rc=$?
set -e
log "qemu exited rc=$rc (124=timeout/killed, 0=guest powered off)"
echo "----- tail of $LOG -----"
tail -n 25 "$LOG" 2>/dev/null || true

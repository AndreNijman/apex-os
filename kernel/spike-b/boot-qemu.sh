#!/bin/bash
# Boot the spike qcow2 headless under QEMU+KVM, capture the serial console.
# The baked-in apex-bootcheck.service prints the running kernel + nvidia module
# state to ttyS0 and powers off, so we just capture serial and grep it.
set -uo pipefail
WORK=/home/andre/apex-os-m0-work/spike-b
QCOW="$WORK/bib-output/qcow2/disk.qcow2"
SERIAL="$WORK/serial.log"
: > "$SERIAL"

if [ ! -f "$QCOW" ]; then echo "MISSING qcow2 at $QCOW"; exit 1; fi

# 4 vCPU, 4G RAM, no display, serial -> file. Guest powers itself off via the
# bootcheck service; we also hard-cap with a timeout as a safety net.
timeout 300 qemu-system-x86_64 \
  -enable-kvm -cpu host -smp 4 -m 4096 \
  -machine q35 \
  -drive file="$QCOW",if=virtio,format=qcow2 \
  -nographic -serial file:"$SERIAL" -monitor none \
  -nic user,model=virtio-net-pci \
  >/dev/null 2>&1
echo "QEMU exited rc=$?"
echo "===== serial.log key lines ====="
grep -aE "Linux version|APEX-BOOTCHECK|APEX uname|APEX system-running|APEX nvidia-ko|Reached target Multi-User|Kernel panic|not syncing" "$SERIAL" | head -40

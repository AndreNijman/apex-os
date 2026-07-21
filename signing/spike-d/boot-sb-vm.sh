#!/usr/bin/env bash
#
# boot-sb-vm.sh — boot a kernel under an SB-ENFORCING OVMF in QEMU and capture
# the serial log. Used to demonstrate that:
#   * an APEX-signed kernel boots (kernel + lockdown come up), and
#   * an unsigned / foreign-signed kernel is refused by the firmware.
#
# Builds a throwaway FAT ESP (superfloppy) containing:
#   /EFI/BOOT/BOOTX64.EFI  <- an APEX-signed UEFI shell (auto-booted)
#   /startup.nsh           <- launches the kernel with a cmdline + initrd
#   /vmlinuz.efi           <- the kernel under test
#   /initramfs.cpio.gz     <- tiny init that proves userspace + powers off
#
# The firmware verifies every LoadImage against db; the shell is APEX-signed so
# it always loads, and it is the shell's LoadImage of the kernel that is the
# actual signature gate under test.
#
# Usage:
#   boot-sb-vm.sh --kernel K --initramfs I --loader SHELL --vars VARS [opts]
# Options:
#   --cmdline "..."   extra kernel cmdline (default sets console + lockdown)
#   --name NAME       label for output logs (default: sbtest)
#   --outdir DIR      where to write ESP + logs (default: $PWD)
#   --timeout SECS    hard kill after SECS (default: 90)
#   --mem MB          guest RAM (default: 2048; keep <=3072)
#   --code FILE       OVMF secure CODE (default: OVMF_CODE.secure.4m.fd)
#
set -euo pipefail

KERNEL= INITRAMFS= LOADER= VARS=
CMDLINE="console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 lockdown=integrity"
NAME=sbtest OUTDIR="$PWD" TIMEOUT=90 MEM=2048
CODE=/usr/share/edk2/x64/OVMF_CODE.secure.4m.fd

while [[ $# -gt 0 ]]; do
  case "$1" in
    --kernel)    KERNEL="$2"; shift 2;;
    --initramfs) INITRAMFS="$2"; shift 2;;
    --loader)    LOADER="$2"; shift 2;;
    --vars)      VARS="$2"; shift 2;;
    --cmdline)   CMDLINE="$2"; shift 2;;
    --name)      NAME="$2"; shift 2;;
    --outdir)    OUTDIR="$2"; shift 2;;
    --timeout)   TIMEOUT="$2"; shift 2;;
    --mem)       MEM="$2"; shift 2;;
    --code)      CODE="$2"; shift 2;;
    *) echo "unknown arg: $1"; exit 2;;
  esac
done

for v in KERNEL INITRAMFS LOADER VARS; do
  [[ -n "${!v}" ]] || { echo "missing --${v,,}"; exit 2; }
  [[ -f "${!v}" ]] || { echo "not a file: ${!v}"; exit 2; }
done
[[ -f "$CODE" ]] || { echo "OVMF secure CODE not found: $CODE"; exit 1; }

mkdir -p "$OUTDIR"
ESP="$OUTDIR/esp-$NAME.img"
VARS_RW="$OUTDIR/vars-$NAME.fd"
SERIAL="$OUTDIR/serial-$NAME.log"
OVMFLOG="$OUTDIR/ovmf-$NAME.log"
NSH="$OUTDIR/startup-$NAME.nsh"

# ---- startup.nsh: launch kernel, and if LoadImage is refused, shut down ----
cat > "$NSH" <<EOF
@echo -on
echo "APEX-SPIKE-D: startup.nsh executing on" %cwd%
fs0:
echo "APEX-SPIKE-D: launching vmlinuz.efi (firmware will verify its signature vs db)"
vmlinuz.efi initrd=\initramfs.cpio.gz $CMDLINE
echo "APEX-SPIKE-D: control returned to shell -> kernel image was REFUSED or exited (lasterror=%lasterror%)"
echo "APEX-SPIKE-D: shutting down"
reset -s
EOF

# ---- build the ESP (superfloppy FAT, no root needed via mtools) ----
rm -f "$ESP"
truncate -s 96M "$ESP"
mkfs.vfat -F 32 -n APEXESP "$ESP" >/dev/null
mmd   -i "$ESP" ::/EFI ::/EFI/BOOT
mcopy -i "$ESP" "$LOADER"    ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$ESP" "$NSH"       ::/startup.nsh
mcopy -i "$ESP" "$KERNEL"    ::/vmlinuz.efi
mcopy -i "$ESP" "$INITRAMFS" ::/initramfs.cpio.gz

# ---- fresh writable copy of the enrolled VARS ----
cp "$VARS" "$VARS_RW"

: > "$SERIAL"; : > "$OVMFLOG"
echo ">> booting '$NAME' (SB enforcing) timeout=${TIMEOUT}s"
echo "   CODE   : $CODE"
echo "   VARS   : $VARS"
echo "   kernel : $KERNEL"
echo "   serial : $SERIAL"

set +e
timeout --signal=KILL "$TIMEOUT" \
qemu-system-x86_64 \
  -machine q35,smm=on,accel=kvm \
  -cpu host -m "$MEM" -smp 2 \
  -global driver=cfi.pflash01,property=secure,value=on \
  -global ICH9-LPC.disable_s3=1 \
  -drive if=pflash,unit=0,format=raw,readonly=on,file="$CODE" \
  -drive if=pflash,unit=1,format=raw,file="$VARS_RW" \
  -drive if=virtio,format=raw,file="$ESP",media=disk \
  -debugcon "file:$OVMFLOG" -global isa-debugcon.iobase=0x402 \
  -serial "file:$SERIAL" \
  -display none -no-reboot 2>>"$OVMFLOG"
rc=$?
set -e

echo ">> qemu exited rc=$rc (124/137 = timeout kill)"
echo "===================== SERIAL ($NAME) ====================="
cat "$SERIAL" 2>/dev/null || true
echo "=========================================================="

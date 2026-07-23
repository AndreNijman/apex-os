#!/usr/bin/env bash
# Build the APEX-OS installer LIVE ISO from installer/Containerfile.installer.
#   rootfs image  ->  squashfs  +  dracut(dmsquash-live) initramfs  ->  xorriso ISO
# Boot-test in QEMU/OVMF before flashing. Run from the repo's installer/ dir.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-/var/tmp/apex-iso-build}"
OCI="$WORK/apex.oci"                       # produced by: skopeo copy containers-storage:localhost/apex-os:daily oci-archive:$OCI:apex-os-daily
OUT="${OUT:-$WORK/apex-os-installer.iso}"
LABEL="APEX-INSTALL"
IMG=apex-installer:latest
ISOROOT="$WORK/isoroot"

mkdir -p "$WORK"
[ -f "$OCI" ] || { echo "ERROR: $OCI missing (run the skopeo export first)"; exit 1; }

echo "== 1. build the installer live-env image =="
cp -f "$OCI" "$HERE/apex.oci"
sudo podman build --isolation=chroot -f "$HERE/Containerfile.installer" -t "$IMG" "$HERE"
rm -f "$HERE/apex.oci"

echo "== 2. export the rootfs =="
rm -rf "$WORK/rootfs"; mkdir -p "$WORK/rootfs"
cid=$(sudo podman create "$IMG")
sudo podman export "$cid" | sudo tar -x -C "$WORK/rootfs"
sudo podman rm "$cid" >/dev/null
KVER=$(sudo ls "$WORK/rootfs/usr/lib/modules" | head -1)
echo "kernel: $KVER"

echo "== 3. dracut live initramfs (dmsquash-live) =="
sudo podman run --rm -v "$WORK":/w "$IMG" \
  dracut --force --no-hostonly --nomdadmconf --nolvmconf \
    --add "dmsquash-live livenet pollcdrom" \
    /w/initrd.img "$KVER"
sudo cp "$WORK/rootfs/usr/lib/modules/$KVER/vmlinuz" "$WORK/vmlinuz"

echo "== 4. squashfs of the rootfs =="
rm -rf "$ISOROOT"; mkdir -p "$ISOROOT/LiveOS" "$ISOROOT/images/pxeboot" "$ISOROOT/EFI/BOOT"
sudo mksquashfs "$WORK/rootfs" "$ISOROOT/LiveOS/squashfs.img" -comp zstd -noappend
sudo cp "$WORK/vmlinuz" "$ISOROOT/images/pxeboot/vmlinuz"
sudo cp "$WORK/initrd.img" "$ISOROOT/images/pxeboot/initrd.img"

echo "== 5. bootloader (UEFI grub2) =="
CMDLINE="root=live:CDLABEL=$LABEL rd.live.image rd.live.overlay.overlayfs=1 quiet"
cat > "$WORK/grub.cfg" <<EOF
set default=0
set timeout=5
menuentry "Install APEX-OS" {
    linux /images/pxeboot/vmlinuz $CMDLINE
    initrd /images/pxeboot/initrd.img
}
menuentry "Install APEX-OS (serial console, for QEMU test)" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200
    initrd /images/pxeboot/initrd.img
}
EOF
# Standalone grub EFI binary embedding the config search; make an efiboot.img (FAT).
grub2-mkstandalone -O x86_64-efi -o "$WORK/BOOTX64.EFI" \
    "boot/grub/grub.cfg=$WORK/grub.cfg" 2>/dev/null || \
grub2-mkstandalone --format=x86_64-efi --output="$WORK/BOOTX64.EFI" \
    "boot/grub/grub.cfg=$WORK/grub.cfg"
# efiboot.img: a FAT image holding /EFI/BOOT/BOOTX64.EFI
dd if=/dev/zero of="$WORK/efiboot.img" bs=1M count=8
mkfs.fat -n APEXEFI "$WORK/efiboot.img"
mmd  -i "$WORK/efiboot.img" ::/EFI ::/EFI/BOOT 2>/dev/null || { command -v mtools >/dev/null || echo "note: mtools needed for mmd/mcopy"; }
mcopy -i "$WORK/efiboot.img" "$WORK/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
cp "$WORK/BOOTX64.EFI" "$ISOROOT/EFI/BOOT/BOOTX64.EFI"
cp "$WORK/grub.cfg"    "$ISOROOT/EFI/BOOT/grub.cfg"
cp "$WORK/efiboot.img" "$ISOROOT/images/efiboot.img"

echo "== 6. xorriso: hybrid UEFI ISO =="
sudo xorriso -as mkisofs \
    -iso-level 3 -V "$LABEL" \
    -eltorito-alt-boot -e images/efiboot.img -no-emul-boot \
    -isohybrid-gpt-basdat \
    -o "$OUT" "$ISOROOT"
echo "== DONE: $OUT =="
ls -lh "$OUT"

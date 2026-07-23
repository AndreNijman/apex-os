#!/usr/bin/env bash
# Build the APEX-OS installer LIVE ISO from installer/Containerfile.installer.
#
#   installer image -> exported rootfs
#                    + APEX image injected into its /var/lib/containers (host-side skopeo)
#                   -> ext4 rootfs.img -> squashfs (classic dmsquash-live layout)
#                    + dracut dmsquash-live initramfs
#                   -> xorriso UEFI ISO  (UEFI-only; no BIOS/legacy boot)
#
# Why the ext4-in-squashfs (dm-snapshot) layout instead of overlayfs live root:
# podman/bootc in the live session need native overlay mounts, and the kernel
# refuses overlay-on-overlayfs. With dm-snapshot the live root is plain ext4,
# so `podman run … bootc install to-disk` behaves exactly like on a normal host.
#
# Boot-test in QEMU/OVMF before flashing. Run from the repo's installer/ dir.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-/var/tmp/apex-iso-build}"
OCI="$WORK/apex.oci"                       # produced by: sudo skopeo copy containers-storage:localhost/apex-os:daily oci-archive:$OCI:apex-os-daily
OUT="${OUT:-$WORK/apex-os-installer.iso}"
LABEL="APEX-INSTALL"
IMG=localhost/apex-installer:latest
ISOROOT="$WORK/isoroot"

# PRODUCTION=1 (default): the flashed-to-USB build. NO unattended install path —
# the marker file is not baked and the unattended boot-menu entry is omitted, so
# `apex.unattended` is inert and nobody can accidentally trigger a disk wipe.
# PRODUCTION=0: test/CI build — bakes the marker + adds the unattended menu entry
# so the QEMU boot-test can drive an end-to-end install headlessly.
PRODUCTION="${PRODUCTION:-1}"
if [ "$PRODUCTION" = 1 ]; then ALLOW_UNATTENDED=0; else ALLOW_UNATTENDED=1; fi
echo "build mode: PRODUCTION=$PRODUCTION (ALLOW_UNATTENDED=$ALLOW_UNATTENDED)"

mkdir -p "$WORK"
[ -f "$OCI" ] || { echo "ERROR: $OCI missing (run the skopeo export first)"; exit 1; }

echo "== 1. build the installer live-env image =="
sudo podman build --build-arg "ALLOW_UNATTENDED=$ALLOW_UNATTENDED" \
  -f "$HERE/Containerfile.installer" -t "$IMG" "$HERE"

echo "== 2. export the rootfs =="
sudo rm -rf "$WORK/rootfs"; mkdir -p "$WORK/rootfs"
cid=$(sudo podman create "$IMG")
sudo podman export "$cid" | sudo tar -x -C "$WORK/rootfs"
sudo podman rm "$cid" >/dev/null
KVER=$(sudo ls "$WORK/rootfs/usr/lib/modules" | head -1)
echo "kernel: $KVER"

echo "== 3. embed the APEX image into the live env's container storage =="
# Host-side (can't be a Containerfile RUN: overlay-on-overlay). The graphroot
# lands inside the exported rootfs so the live session's default
# /var/lib/containers/storage already holds localhost/apex-os:daily.
sudo rm -rf "$WORK/cs-run"
sudo skopeo copy "oci-archive:$OCI" \
  "containers-storage:[overlay@$WORK/rootfs/var/lib/containers/storage+$WORK/cs-run]localhost/apex-os:daily"
sudo rm -rf "$WORK/cs-run"

echo "== 4. dracut live initramfs (dmsquash-live) =="
# Built inside the installer image (same kernel/modules as the live rootfs).
# No 'livenet' (needs dracut-network, and we don't netboot).
# label=disable: the SELinux-enforcing host would otherwise deny writes to /w.
sudo podman run --rm --security-opt label=disable -v "$WORK":/w "$IMG" \
  dracut --force --no-hostonly --nomdadmconf --nolvmconf \
    --add "dmsquash-live pollcdrom" \
    --add-drivers "squashfs iso9660 sr_mod cdrom loop ext4 dm-snapshot" \
    /w/initrd.img "$KVER"
sudo cp "$WORK/rootfs/usr/lib/modules/$KVER/vmlinuz" "$WORK/vmlinuz"

echo "== 5. ext4 rootfs.img inside squashfs (classic LiveOS layout) =="
# dmsquash-live default (dm-snapshot) mode expects squashfs.img containing
# LiveOS/rootfs.img (an ext4 fs image). Size it to the rootfs + 15% + slack.
bytes=$(sudo du -sb --apparent-size "$WORK/rootfs" | cut -f1)
imgsz=$(( bytes + bytes / 7 + 768*1024*1024 ))
sudo rm -rf "$WORK/sqroot"; sudo mkdir -p "$WORK/sqroot/LiveOS" "$WORK/mnt"
sudo truncate -s "$imgsz" "$WORK/sqroot/LiveOS/rootfs.img"
sudo mkfs.ext4 -q -F -L "APEX-LIVE-ROOT" "$WORK/sqroot/LiveOS/rootfs.img"
sudo mount -o loop "$WORK/sqroot/LiveOS/rootfs.img" "$WORK/mnt"
sudo cp -a "$WORK/rootfs/." "$WORK/mnt/"
sudo umount "$WORK/mnt"; sudo rmdir "$WORK/mnt"

rm -rf "$ISOROOT"; mkdir -p "$ISOROOT/LiveOS" "$ISOROOT/images/pxeboot" "$ISOROOT/EFI/BOOT"
sudo mksquashfs "$WORK/sqroot" "$ISOROOT/LiveOS/squashfs.img" \
  -comp zstd -b 1M -noappend -no-progress
sudo rm -rf "$WORK/sqroot"
sudo cp "$WORK/vmlinuz"    "$ISOROOT/images/pxeboot/vmlinuz"
sudo cp "$WORK/initrd.img" "$ISOROOT/images/pxeboot/initrd.img"

echo "== 5b. OCI dir on the ISO (bootc install source) =="
# apex-install passes --source-imgref oci:… pointing here: the oci transport
# streams blobs directly off the ISO. Installing from the embedded
# containers-storage instead would re-tar every layer into /var/tmp (RAM-backed
# in the live env) and OOM on 4G machines.
sudo rm -rf "$ISOROOT/container"
sudo skopeo copy "oci-archive:$OCI" "oci:$ISOROOT/container"

echo "== 6. bootloader (UEFI grub2) =="
# selinux=0: the live env ships no SELinux policy; without this the LSM is
# active-but-policyless and bootc aborts with "Failed to enter install_t
# (running as kernel)". Affects only the live session, not the installed OS.
CMDLINE="root=live:CDLABEL=$LABEL rd.live.image selinux=0"
# Menu config lives ON THE ISO (editable without regenerating BOOTX64.EFI).
# serial+console terminals so headless QEMU (and real serial rigs) get the menu.
cat > "$WORK/grub.cfg" <<EOF
serial --unit=0 --speed=115200
terminal_input serial console
terminal_output serial console

set default=0
set timeout=10

menuentry "Install APEX-OS" {
    linux /images/pxeboot/vmlinuz $CMDLINE quiet
    initrd /images/pxeboot/initrd.img
}
menuentry "Install APEX-OS (verbose boot)" {
    linux /images/pxeboot/vmlinuz $CMDLINE
    initrd /images/pxeboot/initrd.img
}
EOF
# TEST/CI builds only: the unattended-install menu entry (auto-wipes /dev/vda).
# NEVER included in a PRODUCTION build — and even if its cmdline is added by hand,
# apex-install ignores apex.unattended without the (production-absent) marker.
if [ "$PRODUCTION" != 1 ]; then
cat >> "$WORK/grub.cfg" <<EOF
menuentry "Unattended install to /dev/vda -- WIPES /dev/vda (QEMU/CI only)" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 apex.unattended apex.disk=/dev/vda apex.user=andre apex.pass=testpass apex.host=apex apex.karg=console=ttyS0,115200 apex.poweroff
    initrd /images/pxeboot/initrd.img
}
EOF
fi
# Standalone grub EFI binary; the embedded (memdisk) config only locates the ISO
# by label and chains to /EFI/BOOT/grub.cfg above. Do NOT change \$prefix here:
# modules (configfile.mod, search.mod, …) live in the memdisk and grub loads
# them from \$prefix on demand — repointing it breaks module loading.
cat > "$WORK/grub-embed.cfg" <<EOF
search --no-floppy --set=root --label $LABEL
configfile (\$root)/EFI/BOOT/grub.cfg
EOF
sudo podman run --rm --security-opt label=disable -v "$WORK":/w "$IMG" \
  grub2-mkstandalone -O x86_64-efi -o /w/BOOTX64.EFI \
    "boot/grub/grub.cfg=/w/grub-embed.cfg"
# efiboot.img: FAT image holding /EFI/BOOT/BOOTX64.EFI (El Torito UEFI image).
efisz=$(( ($(stat -c%s "$WORK/BOOTX64.EFI") / 1048576 + 8) ))
rm -f "$WORK/efiboot.img"
mkfs.fat -C -n APEXEFI "$WORK/efiboot.img" $(( efisz * 1024 ))
mmd   -i "$WORK/efiboot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$WORK/efiboot.img" "$WORK/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
sudo cp "$WORK/BOOTX64.EFI" "$ISOROOT/EFI/BOOT/BOOTX64.EFI"
sudo cp "$WORK/grub.cfg"    "$ISOROOT/EFI/BOOT/grub.cfg"
sudo mkdir -p "$ISOROOT/images"
sudo cp "$WORK/efiboot.img" "$ISOROOT/images/efiboot.img"

echo "== 7. xorriso: UEFI ISO (El Torito for CD/QEMU + appended ESP for USB) =="
sudo xorriso -as mkisofs \
    -iso-level 3 -rational-rock -joliet -joliet-long \
    -V "$LABEL" \
    -e images/efiboot.img -no-emul-boot \
    -append_partition 2 0xef "$WORK/efiboot.img" \
    -o "$OUT" "$ISOROOT"
echo "== DONE: $OUT =="
ls -lh "$OUT"

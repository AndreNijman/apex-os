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
# Which edition this ISO installs: daily | gaming-nvidia | gaming-mesa.
# This is NOT cosmetic. It names the embedded storage tag AND is stamped into
# the live env so apex-install derives its --target-imgref from it. Hardcoding
# `daily` here (as this script used to) produced a Gaming ISO that installed the
# right bits but recorded the DAILY registry ref as the upgrade origin — the
# machine would silently convert itself to Daily on the first `bootc upgrade`,
# dropping the NVIDIA driver. Keep this parameterised.
EDITION="${EDITION:-daily}"
OCI="$WORK/apex.oci"                       # produced by: sudo skopeo copy containers-storage:localhost/apex-os:$EDITION oci-archive:$OCI:apex-os-$EDITION
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
# A previous run may have left overlay/subvol mounts under rootfs (step 3's
# containers-storage embed). Detach them deepest-first or `rm -rf` fails
# "device busy" and set -e aborts the build.
mount | awk -v d="$WORK/rootfs" 'index($3,d)==1 {print $3}' | sort -r \
  | while read -r m; do sudo umount -l "$m" 2>/dev/null || true; done
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
  "containers-storage:[overlay@$WORK/rootfs/var/lib/containers/storage+$WORK/cs-run]localhost/apex-os:${EDITION}"
sudo rm -rf "$WORK/cs-run"

# Stamp the edition so apex-install derives IMAGE and --target-imgref from it
# rather than assuming daily. Asserted below, because a wrong or missing stamp
# is silent at install time and only bites on the first `bootc upgrade`.
sudo install -Dm644 /dev/null "$WORK/rootfs/usr/lib/apex-installer/edition"
printf '%s\n' "$EDITION" | sudo tee "$WORK/rootfs/usr/lib/apex-installer/edition" >/dev/null
grep -qx "$EDITION" "$WORK/rootfs/usr/lib/apex-installer/edition" \
  || { echo "FATAL: edition stamp not written"; exit 1; }
echo "edition stamped: $EDITION"

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
# +40% and a 1.5G floor of slack. `du --apparent-size` undercounts real ext4 cost
# (metadata, block rounding), and the previous +15%/768M left the live root 96%
# full (~525MB free) — zero margin for logs, /tmp or a container scratch dir.
imgsz=$(( bytes + bytes * 2 / 5 + 1536*1024*1024 ))
sudo rm -rf "$WORK/sqroot"; sudo mkdir -p "$WORK/sqroot/LiveOS" "$WORK/mnt"
sudo truncate -s "$imgsz" "$WORK/sqroot/LiveOS/rootfs.img"
sudo mkfs.ext4 -q -F -L "APEX-LIVE-ROOT" "$WORK/sqroot/LiveOS/rootfs.img"
sudo mount -o loop "$WORK/sqroot/LiveOS/rootfs.img" "$WORK/mnt"
sudo cp -a "$WORK/rootfs/." "$WORK/mnt/"
sudo umount "$WORK/mnt"; sudo rmdir "$WORK/mnt"

# sudo: a previous run's step 5b leaves $ISOROOT/container root-owned.
sudo rm -rf "$ISOROOT"; mkdir -p "$ISOROOT/LiveOS" "$ISOROOT/images/pxeboot" "$ISOROOT/EFI/BOOT"
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
# Serial is CONDITIONAL (apex-logs 48). Unconditionally running \`serial\` then
# \`terminal_output serial console\` is fine under QEMU but hostile on real
# laptops with no UART: the command can fail and take the console terminal down
# with it, and a floating RS-232 line can inject phantom keypresses into the
# menu. Guard it so headless/serial rigs still work while real hardware is never
# put at risk by hardware it does not have.
if serial --unit=0 --speed=115200; then
    terminal_input serial console
    terminal_output serial console
fi

# shim/grub are loaded from the ESP, but the kernel + initrd live on the ISO9660
# filesystem — point \$root at it by volume label before referencing those paths.
search --no-floppy --set=root --label $LABEL

set default=0
set timeout=10

# NO \`quiet\` on the default entry. This is an INSTALLER on unknown hardware —
# there is no splash to protect, and \`quiet\` turns every possible failure
# (KMS bringing up no display, dmsquash-live not finding the ISO, the installer
# unit dying) into an identical featureless black screen. An Acer Aspire hit
# exactly that: menu appeared, then nothing, with no way to tell which stage
# failed. Text boot costs nothing here and makes the failure legible.
#
# console=tty0 LAST so the screen is the primary console; ttyS0 first keeps
# QEMU/CI serial observability.
menuentry "Install APEX-OS" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 console=tty0
    initrd /images/pxeboot/initrd.img
}
# For machines whose GPU the kernel cannot mode-set. "Menu, then black" is the
# signature symptom, and this is the standard escape: no KMS, firmware
# framebuffer only. The installer is a TUI, so it loses nothing.
menuentry "Install APEX-OS (safe graphics — try this if the screen goes black)" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 console=tty0 nomodeset
    initrd /images/pxeboot/initrd.img
}
# Drops to a dracut shell if the live root is not found, instead of hanging
# black. Use when the USB enumerates slowly or the ISO label is not matched.
menuentry "Install APEX-OS (troubleshoot — dracut shell on failure)" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 console=tty0 rd.shell rd.debug
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
# ── SECURE BOOT: use Fedora's SIGNED shim chain, not a self-built binary ─────
# A `grub2-mkstandalone` BOOTX64.EFI is unsigned, so SB firmware refuses it
# ("Access Denied -- rejected probably by Secure Boot", reproduced under OVMF).
# Instead ship the standard, already-signed chain:
#   BOOTX64.EFI  = shimx64.efi  (signed by the Microsoft UEFI CA → firmware trusts it)
#   grubx64.efi  = Fedora's signed grub2 (shim verifies it against its embedded Fedora cert)
#   mmx64.efi    = MokManager, for enrolling our own key later (Stage B)
# The live kernel is Fedora's, which is already signed, so the whole chain
# validates with SB ON and no user action.
#
# Fedora's signed grub is built with prefix /EFI/fedora, and when loaded from
# /EFI/BOOT it looks for its config next to itself; ship grub.cfg in BOTH places
# so either resolution order finds it.
# `|| true` on every find: this script runs under `set -e`, and `find` exits
# non-zero when any listed path is missing (/usr/share/shim does not exist on a
# stock Fedora rootfs) — which silently killed the build at this step.
SHIM=$(sudo find "$WORK/rootfs/boot/efi" -name 'shimx64.efi' 2>/dev/null | head -1 || true)
GRUBEFI=$(sudo find "$WORK/rootfs/boot/efi" -name 'grubx64.efi' 2>/dev/null | head -1 || true)
MMEFI=$(sudo find "$WORK/rootfs/boot/efi" -name 'mmx64.efi' 2>/dev/null | head -1 || true)
[ -n "$SHIM" ] && [ -n "$GRUBEFI" ] \
  || { echo "BUILD ASSERT FAILED: signed shimx64.efi/grubx64.efi not found in the rootfs (shim-x64 + grub2-efi-x64 installed?)"; exit 1; }
echo "shim:    $SHIM"
echo "grubefi: $GRUBEFI"

# efiboot.img: FAT image holding the whole signed chain (El Torito UEFI image).
rm -f "$WORK/efiboot.img"
mkfs.fat -C -n APEXEFI "$WORK/efiboot.img" 20480
mmd   -i "$WORK/efiboot.img" ::/EFI ::/EFI/BOOT ::/EFI/fedora
# install -m 0644, not cp: Fedora ships these EFI binaries mode 700 root:root and
# `cp` preserves that, so the UNPRIVILEGED mcopy below could not read them
# ("Permission denied") and set -e killed the build.
sudo install -m 0644 "$SHIM"    "$WORK/BOOTX64.EFI"
sudo install -m 0644 "$GRUBEFI" "$WORK/grubx64.efi"
mcopy -i "$WORK/efiboot.img" "$WORK/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$WORK/efiboot.img" "$WORK/grubx64.efi" ::/EFI/BOOT/grubx64.efi
mcopy -i "$WORK/efiboot.img" "$WORK/grub.cfg"    ::/EFI/BOOT/grub.cfg
mcopy -i "$WORK/efiboot.img" "$WORK/grub.cfg"    ::/EFI/fedora/grub.cfg
if [ -n "$MMEFI" ]; then sudo install -m 0644 "$MMEFI" "$WORK/mmx64.efi"; mcopy -i "$WORK/efiboot.img" "$WORK/mmx64.efi" ::/EFI/BOOT/mmx64.efi; fi

sudo mkdir -p "$ISOROOT/EFI/BOOT" "$ISOROOT/EFI/fedora" "$ISOROOT/images"
sudo cp "$WORK/BOOTX64.EFI" "$ISOROOT/EFI/BOOT/BOOTX64.EFI"
sudo cp "$WORK/grubx64.efi" "$ISOROOT/EFI/BOOT/grubx64.efi"
sudo cp "$WORK/grub.cfg"    "$ISOROOT/EFI/BOOT/grub.cfg"
sudo cp "$WORK/grub.cfg"    "$ISOROOT/EFI/fedora/grub.cfg"
[ -n "$MMEFI" ] && sudo cp "$WORK/mmx64.efi" "$ISOROOT/EFI/BOOT/mmx64.efi"
sudo cp "$WORK/efiboot.img" "$ISOROOT/images/efiboot.img"

echo "== 6b. build-time invariants (fail loudly rather than ship a broken ISO) =="
# CRITICAL-1 shipped because nothing asserted the live env could actually run the
# installer: `clear` (ncurses) was missing, so every install died the moment the
# user confirmed. Assert the things the installer depends on, in the ROOTFS.
_need_bin() { sudo test -x "$WORK/rootfs/usr/bin/$1" || sudo test -x "$WORK/rootfs/usr/sbin/$1" \
    || { echo "BUILD ASSERT FAILED: /usr/bin/$1 missing from the live rootfs"; exit 1; }; }
for b in clear whiptail podman lsblk useradd chpasswd mount umount blkid udevadm partprobe awk sed \
         mkfs.btrfs findmnt tput mktemp basename dirname chroot tee find df grep \
         mokutil efibootmgr; do
  _need_bin "$b"
done
sudo test -x "$WORK/rootfs/usr/bin/apex-install" \
  || { echo "BUILD ASSERT FAILED: apex-install missing"; exit 1; }
sudo bash -n "$WORK/rootfs/usr/bin/apex-install" \
  || { echo "BUILD ASSERT FAILED: apex-install has a syntax error"; exit 1; }
# Production must NOT carry the unattended marker.
if [ "$PRODUCTION" = 1 ]; then
  if sudo test -e "$WORK/rootfs/usr/share/apex-installer/allow-unattended"; then
    echo "BUILD ASSERT FAILED: production build contains the unattended marker"; exit 1; fi
  if grep -qi 'apex.unattended' "$WORK/grub.cfg"; then
    echo "BUILD ASSERT FAILED: production grub.cfg contains an unattended entry"; exit 1; fi
  echo "asserts OK: no unattended marker, no unattended menu entry"
else
  echo "asserts OK (test build: unattended intentionally present)"
fi

echo "== 7. xorriso: UEFI ISO (El Torito for CD/QEMU + appended ESP for USB) =="
# -appended_part_as_gpt + -partition_offset 16: without these the image carries an
# MBR-only table whose partition 1 starts at LBA 0, which some UEFI firmwares
# dislike when booting from USB. Produces a valid GPT with the ESP intact and the
# APEX-INSTALL label still resolvable from both whole-disk and partition views.
sudo xorriso -as mkisofs \
    -iso-level 3 -rational-rock -joliet -joliet-long \
    -V "$LABEL" \
    -e images/efiboot.img -no-emul-boot \
    -append_partition 2 0xef "$WORK/efiboot.img" \
    -appended_part_as_gpt -partition_offset 16 \
    -o "$OUT" "$ISOROOT"

# Checksum the ISO we just built (it used to record the PREVIOUS build's hash).
sudo sha256sum "$OUT" | sudo tee "$OUT.sha256" >/dev/null
echo "== DONE: $OUT =="
ls -lh "$OUT"; cat "$OUT.sha256"

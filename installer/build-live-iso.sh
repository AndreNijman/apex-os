#!/usr/bin/env bash
# Build the APEX-OS installer LIVE ISO from installer/Containerfile.installer.
#
#   installer image -> exported rootfs
#                    + APEX image injected into its /var/lib/containers (host-side skopeo)
#                   -> ext4 rootfs.img -> squashfs (classic dmsquash-live layout)
#                    + dracut dmsquash-live initramfs
#                   -> xorriso hybrid ISO (UEFI incl. Secure Boot + legacy BIOS)
#
# Why the ext4-in-squashfs (dm-snapshot) layout instead of overlayfs live root:
# podman/bootc in the live session need native overlay mounts, and the kernel
# refuses overlay-on-overlayfs. With dm-snapshot the live root is plain ext4,
# so `podman run … bootc install to-disk` behaves exactly like on a normal host.
#
# Boot-test in QEMU BOTH ways (SeaBIOS and OVMF) before flashing. Run from the
# repo's installer/ dir.
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
# Overridable so a throwaway probe image can be built into a bootable ISO
# without clobbering the real installer tag. The default is the production one.
IMG="${IMG:-localhost/apex-installer:latest}"
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

# LEGACY-BIOS-ONLY video handoff. On BIOS there is no GOP: something must
# program a VESA linear framebuffer before userspace starts, or sysfb has
# nothing to register, no DRM device node ever appears, and the graphical
# installer cannot start -- the launcher paints its DRM-nodes-absent bug
# screen on a text console. QEMU hides this failure (its bochs GPU has a
# native kernel driver that needs no firmware framebuffer), so it was only
# caught by booting BIOS with nomodeset, which is exactly what every real
# driverless legacy machine looks like.
#
# DO NOT "simplify" this back to gfxpayload. Fedora's i386-pc grub cannot do
# the upstream video handoff: its linux command is the 16-bit-entry loader
# (the module imports grub_relocator16_boot and resets the card to text mode
# right before jumping) and contains no gfxpayload handling at all -- measured
# on grub2-pc-modules 2.12-43.fc43 by dumping the ELF symbols and strings of
# linux.mod, and confirmed in QEMU: with gfxpayload set, the kernel still came
# up on the 80x25 VGA text console. What the 16-bit boot protocol DOES offer
# is vga=791 (VESA mode 0x317, 1024x768 16bpp linear): grub parses it into the
# boot header, the kernel sets the mode itself in real mode via the video
# BIOS, sysfb registers it, simpledrm binds it, and the GUI paints -- verified
# in QEMU with nomodeset (simple-framebuffer + simpledrm in the boot log, GUI
# painted at 1024x768, bochs driver absent).
# Worst case on a pre-VBE-2.0 card without that mode: the kernel prints
# Undefined video mode number, waits 30 seconds for a key, then boots in text
# mode -- the pre-fix behaviour, only slower. It cannot hang the boot. The
# troubleshoot entry below deliberately omits it as the escape hatch.
# On UEFI, biosfb expands to nothing: the UEFI cmdline stays byte-identical
# to the UEFI-only builds and the kernel takes the GOP framebuffer as before.
if [ "\$grub_platform" = "pc" ]; then
    set biosfb=vga=791
else
    set biosfb=
fi

# shim/grub are loaded from the ESP, but the kernel + initrd live on the ISO9660
# filesystem — point \$root at it by volume label before referencing those paths.
# On the BIOS path both grub and the kernel live on the ISO9660 fs and this
# search works identically, so the same file serves both firmwares unmodified.
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
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 console=tty0 \$biosfb
    initrd /images/pxeboot/initrd.img
}
# For machines whose GPU the kernel cannot mode-set. "Menu, then black" is the
# signature symptom, and this is the standard escape: no KMS, firmware
# framebuffer only.
#
# The graphical installer still works here, and that is not luck. nomodeset
# only stops NATIVE DRM drivers; the live kernel has CONFIG_DRM_SIMPLEDRM=y and
# CONFIG_SYSFB_SIMPLEFB=y (both built in, not modules), so simpledrm binds the
# boot-time framebuffer (the GOP one on UEFI, the vga=791 VESA one on BIOS)
# and a /dev/dri card node exists regardless -- do not assume it is card0,
# the number floats. cage runs on it with WLR_RENDERER=pixman (dumb buffers,
# no render node — simpledrm has none) and GTK renders with GSK_RENDERER=cairo
# (shm, no GL). Nothing in the installer's display path needs a real GPU
# driver.
#
# NOTE for anyone editing this heredoc: it is UNQUOTED (<<EOF), so backticks and
# dollar-variables in these comments are interpreted by the shell. Both bit this
# comment block during editing: a backtick-quoted word ran as a command, and a
# dollar-word tripped the unbound-variable check. Keep prose free of both.
#
# This is worth stating because the launcher used to treat "no native KMS" as a
# reason to give up on the GUI, which was wrong and is what stranded users in
# the old text installer.
menuentry "Install APEX-OS (safe graphics — try this if the screen goes black)" {
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 console=tty0 nomodeset \$biosfb
    initrd /images/pxeboot/initrd.img
}
# Drops to a dracut shell if the live root is not found, instead of hanging
# black. Use when the USB enumerates slowly or the ISO label is not matched.
# Deliberately does NOT carry the biosfb vga mode: this entry doubles as the
# escape hatch for a machine whose video BIOS misbehaves on the VESA mode set,
# so it must stay bootable with the firmware console untouched.
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
    linux /images/pxeboot/vmlinuz $CMDLINE console=ttyS0,115200 apex.unattended apex.disk=/dev/vda apex.user=andre apex.pass=testpass apex.host=apex apex.karg=console=ttyS0,115200 apex.poweroff \$biosfb
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

echo "== 6a. bootloader (legacy BIOS grub2: El Torito core + isohybrid MBR) =="
# grub2 for BIOS too, NOT isolinux/syslinux: one bootloader means ONE menu file
# — the exact grub.cfg written above is read unmodified by the BIOS core image
# (its embedded prefix is /boot/grub2), so entries, cmdlines and the safety
# reasoning in the comments can never drift apart between firmwares. Secure
# Boot is untouched: this core image is only ever executed by legacy BIOS
# firmware; UEFI still loads the signed shim chain from step 6. The layout
# (grub2 El Torito entry + grub2-mbr boot code + appended GPT ESP) is the same
# one every shipping Ubuntu hybrid ISO uses, so the xorriso combination in
# step 7 is field-proven rather than invented here.
#
# Built INSIDE the installer image like the dracut step: the host may lack
# grub2-mkimage/the i386-pc module set (grub2-pc-modules is in the image for
# exactly this). The embedded module list covers everything grub.cfg executes
# (search_label for the root hunt, serial+terminal+test+echo for the guarded
# console setup, linux+boot to start the kernel, part_msdos+part_gpt+biosdisk+
# iso9660 so a dd'd USB enumerates). all_video stays embedded only as a
# videoinfo debugging aid at the grub prompt — the BIOS framebuffer is set by
# the KERNEL via vga=791 because Fedora's BIOS grub cannot set it (see the
# grub.cfg heredoc comment). The full i386-pc tree is ALSO shipped on the ISO
# at /boot/grub2/i386-pc so any future grub.cfg edit that needs one more
# module autoloads it from the medium instead of dying with "command not
# found" only on BIOS machines.
sudo rm -rf "$WORK/grub-i386-pc"
sudo podman run --rm --security-opt label=disable -v "$WORK":/w "$IMG" \
  bash -c "grub2-mkimage -O i386-pc-eltorito -d /usr/lib/grub/i386-pc -p /boot/grub2 \
      -o /w/eltorito.img \
      biosdisk iso9660 part_msdos part_gpt normal search search_label configfile \
      linux echo test serial terminal all_video boot \
    && cp /usr/lib/grub/i386-pc/boot_hybrid.img /w/boot_hybrid.img \
    && mkdir -p /w/grub-i386-pc \
    && cp /usr/lib/grub/i386-pc/*.mod /usr/lib/grub/i386-pc/*.lst /w/grub-i386-pc/"
sudo mkdir -p "$ISOROOT/boot/grub2/i386-pc"
sudo cp "$WORK/eltorito.img"      "$ISOROOT/images/eltorito.img"
sudo cp "$WORK/grub.cfg"          "$ISOROOT/boot/grub2/grub.cfg"
sudo cp -a "$WORK/grub-i386-pc/." "$ISOROOT/boot/grub2/i386-pc/"

echo "== 6b. build-time invariants (fail loudly rather than ship a broken ISO) =="
# CRITICAL-1 shipped because nothing asserted the live env could actually run the
# installer: `clear` (ncurses) was missing, so every install died the moment the
# user confirmed. Assert the things the installer depends on, in the ROOTFS.
_need_bin() { sudo test -x "$WORK/rootfs/usr/bin/$1" || sudo test -x "$WORK/rootfs/usr/sbin/$1" \
    || { echo "BUILD ASSERT FAILED: /usr/bin/$1 missing from the live rootfs"; exit 1; }; }
for b in clear podman lsblk useradd chpasswd mount umount blkid udevadm partprobe awk sed \
         mkfs.btrfs findmnt tput mktemp basename dirname chroot tee find df grep \
         mokutil efibootmgr; do
  _need_bin "$b"
done
sudo test -x "$WORK/rootfs/usr/bin/apex-install" \
  || { echo "BUILD ASSERT FAILED: apex-install missing"; exit 1; }
sudo bash -n "$WORK/rootfs/usr/bin/apex-install" \
  || { echo "BUILD ASSERT FAILED: apex-install has a syntax error"; exit 1; }

# ── The GUI is now the ONLY front end — assert it can actually come up ───────
# whiptail is deliberately NOT in the list above any more: the text installer is
# gone. That removes the safety net this script used to lean on, so everything
# the graphical installer needs has to be proven HERE, in the rootfs that is
# about to be sealed into a squashfs, not just in the container image it came
# from. Every failure in this area so far has been silent — a missing typelib,
# a missing seat backend, absent firmware — and each one shipped an ISO that
# booted to a black screen. If any of these is missing there is no fallback UI
# left to rescue the user, so the build must stop instead.
for b in cage seatd Xwayland apex-installer-gui apex-installer-launch; do
  _need_bin "$b"
done
sudo chroot "$WORK/rootfs" python3 -c \
  'import gi; gi.require_version("Gtk","4.0"); gi.require_version("Adw","1"); from gi.repository import Gtk, Adw, Gdk, GLib, Gio, Pango' \
  || { echo "BUILD ASSERT FAILED: the live rootfs cannot import GTK4/libadwaita — the GUI would not start (cairo typelib / gobject-introspection missing again?)"; exit 1; }
sudo chroot "$WORK/rootfs" python3 -m py_compile /usr/bin/apex-installer-gui \
  || { echo "BUILD ASSERT FAILED: apex-installer-gui has a Python syntax error"; exit 1; }
sudo rm -rf "$WORK/rootfs/usr/bin/__pycache__"
sudo bash -n "$WORK/rootfs/usr/bin/apex-installer-launch" \
  || { echo "BUILD ASSERT FAILED: apex-installer-launch has a syntax error"; exit 1; }
# Enablement, not just presence. An installed-but-not-enabled seatd is exactly
# the kind of thing that looks fine in `rpm -q` and leaves cage unable to take
# a seat on tty1 at boot, which is a black screen with no diagnosis.
#
# -L, not -e. These are symlinks whose targets are ABSOLUTE paths inside the
# live rootfs (/usr/lib/systemd/system/…), and `test -e` FOLLOWS them — which
# resolves against the build host's root, where those units do not exist. The
# first version of this assert failed on a rootfs that was in fact correct.
for u in apex-installer.service seatd.service; do
  sudo test -L "$WORK/rootfs/etc/systemd/system/multi-user.target.wants/$u" \
    || { echo "BUILD ASSERT FAILED: $u is not enabled in the live rootfs"; exit 1; }
done
sudo test -L "$WORK/rootfs/etc/systemd/system/getty@tty1.service" \
  || { echo "BUILD ASSERT FAILED: getty@tty1 is not masked — it would fight cage for tty1"; exit 1; }
echo "asserts OK: GUI stack present, importable, and enabled"

# ── BIOS boot artifacts: prove the legacy path exists before xorriso runs ────
# The ISO is dual-firmware now. A missing or truncated BIOS core image would
# still produce an ISO that boots fine on every UEFI machine we test on, and
# the regression would only surface in the field on exactly the machines this
# path exists for — so a build that cannot prove the BIOS artifacts must stop.
sudo test -s "$WORK/eltorito.img" \
  || { echo "BUILD ASSERT FAILED: eltorito.img (BIOS grub core) missing or empty"; exit 1; }
_sz=$(sudo stat -c%s "$WORK/eltorito.img")
[ "$_sz" -ge 100000 ] \
  || { echo "BUILD ASSERT FAILED: eltorito.img is only $_sz bytes — grub2-mkimage embedded too little (module list wrong?)"; exit 1; }
_sz=$(sudo stat -c%s "$WORK/boot_hybrid.img")
[ "$_sz" -gt 0 ] && [ "$_sz" -le 512 ] \
  || { echo "BUILD ASSERT FAILED: boot_hybrid.img is $_sz bytes — must be 1..512 to fit the MBR boot-code area"; exit 1; }
sudo cmp -s "$WORK/grub.cfg" "$ISOROOT/boot/grub2/grub.cfg" \
  || { echo "BUILD ASSERT FAILED: /boot/grub2/grub.cfg missing or differs from the UEFI menu — the one-menu-for-both-firmwares invariant is broken"; exit 1; }
sudo test -f "$ISOROOT/boot/grub2/i386-pc/normal.mod" \
  || { echo "BUILD ASSERT FAILED: i386-pc module tree missing from the ISO (BIOS grub could not autoload anything)"; exit 1; }
# The config must actually get a framebuffer set on BIOS, or driverless legacy
# machines boot to the DRM-nodes-absent bug screen instead of the GUI (the
# single most likely way to break this path — see the vga=791 comment in the
# grub.cfg heredoc: gfxpayload is a NO-OP on Fedora's BIOS grub, the kernel
# has to set the mode itself). Check both the guard that sets biosfb and that
# at least the default + safe-graphics entries actually reference it.
grep -q 'biosfb=vga=791' "$WORK/grub.cfg" \
  || { echo "BUILD ASSERT FAILED: grub.cfg lost its BIOS vga=791 framebuffer handoff — driverless legacy machines would get no GUI"; exit 1; }
[ "$(grep -c '\$biosfb' "$WORK/grub.cfg")" -ge 2 ] \
  || { echo "BUILD ASSERT FAILED: fewer than 2 menu entries reference biosfb — the vga=791 handoff is set but unused"; exit 1; }
echo "asserts OK: BIOS grub core, isohybrid MBR, shared menu, module tree, vga=791 handoff"
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

echo "== 7. xorriso: hybrid BIOS+UEFI ISO (El Torito for CD/QEMU + MBR/GPT for dd'd USB) =="
# -appended_part_as_gpt + -partition_offset 16: without these the image carries an
# MBR-only table whose partition 1 starts at LBA 0, which some UEFI firmwares
# dislike when booting from USB. Produces a valid GPT with the ESP intact and the
# APEX-INSTALL label still resolvable from both whole-disk and partition views.
#
# BIOS side (everything before -eltorito-alt-boot): -b makes the grub2 core the
# FIRST El Torito entry, which BIOS firmware picks when booting the ISO as a
# CD; --grub2-boot-info patches the core with its own LBA so the SAME core can
# also be entered from the MBR path; --grub2-mbr installs grub's boot_hybrid
# code in the system area so a dd'd USB stick boots on legacy BIOS;
# --mbr-force-bootable sets the active flag some BIOSes insist on before they
# will boot a disk at all. The UEFI side is UNCHANGED from the UEFI-only build:
# same efiboot.img as an alt El Torito entry, same appended GPT ESP — a UEFI
# machine (Secure Boot included) sees exactly what it saw before. This exact
# combination is what Ubuntu's shipping hybrid ISOs use.
sudo xorriso -as mkisofs \
    -iso-level 3 -rational-rock -joliet -joliet-long \
    -V "$LABEL" \
    --grub2-mbr "$WORK/boot_hybrid.img" \
    --mbr-force-bootable \
    -b images/eltorito.img -no-emul-boot -boot-load-size 4 \
    -boot-info-table --grub2-boot-info \
    -eltorito-alt-boot \
    -e images/efiboot.img -no-emul-boot \
    -append_partition 2 0xef "$WORK/efiboot.img" \
    -appended_part_as_gpt -partition_offset 16 \
    -o "$OUT" "$ISOROOT"

# Post-build proof that the EMITTED image advertises both firmware paths.
# xorriso reads back its own boot records; the four properties below are
# precisely what each firmware needs (BIOS: x86 El Torito entry + MBR boot
# code; UEFI: EFI El Torito entry + GPT ESP). If any is absent the image
# cannot boot somewhere we claim it does, so it must not ship.
_rep=$(sudo xorriso -indev "$OUT" -report_el_torito plain -report_system_area plain 2>/dev/null)
echo "$_rep" | grep -q 'El Torito boot img :   1  BIOS' \
  || { echo "BUILD ASSERT FAILED: emitted ISO has no BIOS El Torito boot entry"; exit 1; }
echo "$_rep" | grep -q 'El Torito boot img :   2  UEFI' \
  || { echo "BUILD ASSERT FAILED: emitted ISO has no UEFI El Torito boot entry"; exit 1; }
echo "$_rep" | grep -q 'grub2-mbr' \
  || { echo "BUILD ASSERT FAILED: emitted ISO system area lacks the grub2 isohybrid MBR (dd'd USB would not BIOS-boot)"; exit 1; }
echo "$_rep" | grep -q '28732ac11ff8d211ba4b00a0c93ec93b' \
  || { echo "BUILD ASSERT FAILED: emitted ISO has no ESP-typed GPT partition (UEFI-from-USB regression)"; exit 1; }
echo "asserts OK: emitted ISO carries BIOS+UEFI El Torito entries, isohybrid MBR, GPT ESP"

# Checksum the ISO we just built (it used to record the PREVIOUS build's hash).
sudo sha256sum "$OUT" | sudo tee "$OUT.sha256" >/dev/null
echo "== DONE: $OUT =="
ls -lh "$OUT"; cat "$OUT.sha256"

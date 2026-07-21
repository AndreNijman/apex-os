#!/bin/bash
# Spike E stage 10 — build the fake incumbent OS (Alpine) with an L16-faithful
# classic-EFISTUB layout on the SHARED ESP.
#
#   * Alpine root on /dev/vda2 (ext4)
#   * kernel + initramfs copied ONTO the shared ESP at /EFI/alpine/  (like Void)
#   * a persistent efibootmgr EFISTUB entry is created on first boot (stage=setup)
#   * podman + tooling installed so `bootc install` runs from inside this VM
#
# First boot is bootstrapped by a UEFI-shell startup.nsh (tool-free), which
# launches the EFISTUB kernel directly; stage=setup then writes the real NVRAM
# entry and removes the bootstrap.
#
# Run: sudo bash 10-build-incumbent.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"
need_root
. "$SPIKE_WORK/uuids.env"

ROOTMNT="$SPIKE_WORK/mnt/root"
ESPMNT="$SPIKE_WORK/mnt/esp"
LOOP=""
cleanup() {
  set +e
  for m in "$ROOTMNT/dev/pts" "$ROOTMNT/dev" "$ROOTMNT/proc" "$ROOTMNT/sys" "$ROOTMNT/run" "$ESPMNT" "$ROOTMNT"; do
    mountpoint -q "$m" && umount -R "$m" 2>/dev/null
  done
  [ -n "$LOOP" ] && losetup -d "$LOOP" 2>/dev/null
}
trap cleanup EXIT

mkdir -p "$ROOTMNT" "$ESPMNT"
log "Attaching loop"
LOOP="$(losetup --show -f -P "$DISK_IMG")"
echo "loop=$LOOP"
for _ in $(seq 1 10); do [ -b "${LOOP}p3" ] && break; sleep 0.3; done

log "Mounting incumbent root + ESP"
mount "${LOOP}p2" "$ROOTMNT"
mount "${LOOP}p1" "$ESPMNT"

log "Extracting Alpine minirootfs"
tar -xzf "$ALPINE_ROOTFS" -C "$ROOTMNT"

log "Configuring apk repositories"
mkdir -p "$ROOTMNT/etc/apk"
cat > "$ROOTMNT/etc/apk/repositories" <<EOF
$ALPINE_REPO
$ALPINE_REPO_COMMUNITY
EOF
cp /etc/resolv.conf "$ROOTMNT/etc/resolv.conf"

log "Bind-mounting pseudo-fs for chroot (apk runs from inside the rootfs)"
mount -t proc proc "$ROOTMNT/proc"
mount --rbind /sys "$ROOTMNT/sys"
mount --rbind /dev "$ROOTMNT/dev"
mount -t tmpfs tmpfs "$ROOTMNT/run"

log "Installing packages (chroot apk)"
chroot "$ROOTMNT" apk update
chroot "$ROOTMNT" apk add \
  alpine-base linux-lts linux-firmware-none mkinitfs \
  util-linux agetty blkid lsblk findmnt sfdisk \
  e2fsprogs e2fsprogs-extra dosfstools btrfs-progs cryptsetup lvm2 \
  efibootmgr \
  podman crun conmon netavark aardvark-dns containers-common fuse-overlayfs \
  bash coreutils findutils gptfdisk

KVER="$(ls "$ROOTMNT/lib/modules" | head -1)"
log "Kernel version: $KVER"
log "EFI_STUB check:"
grep -E 'CONFIG_EFI_STUB|CONFIG_EFI=' "$ROOTMNT/boot/config-${KVER}" || echo "(config not found)"

log "mkinitfs configuration + build"
cat > "$ROOTMNT/etc/mkinitfs/mkinitfs.conf" <<EOF
features="ata base ide scsi usb virtio ext4 btrfs nvme cdrom"
EOF
chroot "$ROOTMNT" mkinitfs -o "/boot/initramfs-lts" "$KVER"
ls -la "$ROOTMNT/boot"

log "System configuration (fstab, hostname, root pw, cgroups, inittab)"
cat > "$ROOTMNT/etc/fstab" <<EOF
UUID=$INC_UUID   /          ext4  rw,relatime            0 1
UUID=$ESP_UUID   /boot/efi  vfat  rw,relatime,fmask=0077,dmask=0077  0 2
EOF
echo "apex-incumbent" > "$ROOTMNT/etc/hostname"
echo "root:apex" | chroot "$ROOTMNT" chpasswd

# cgroup v2 unified for podman; disable openrc verbose noise
sed -i 's/^#\?rc_cgroup_mode=.*/rc_cgroup_mode="unified"/' "$ROOTMNT/etc/rc.conf" || \
  echo 'rc_cgroup_mode="unified"' >> "$ROOTMNT/etc/rc.conf"

# containers.conf: cgroupfs manager + file events (no systemd on Alpine)
mkdir -p "$ROOTMNT/etc/containers"
cat > "$ROOTMNT/etc/containers/containers.conf" <<EOF
[engine]
cgroup_manager = "cgroupfs"
events_logger = "file"
EOF

# Minimal-but-sufficient service set (block devs come from devtmpfs; the
# controller also mounts pseudo-fs defensively).
for s in devfs dmesg; do chroot "$ROOTMNT" rc-update add "$s" sysinit 2>/dev/null || true; done
for s in bootmisc hwclock sysctl modules; do chroot "$ROOTMNT" rc-update add "$s" boot 2>/dev/null || true; done
for s in mount-ro killprocs; do chroot "$ROOTMNT" rc-update add "$s" shutdown 2>/dev/null || true; done

log "Installing in-guest controller + inittab hook"
install -D -m0755 "$HERE/guest/spike-controller.sh" "$ROOTMNT/usr/local/sbin/spike-controller.sh"
# Autologin root on serial (debug) + run the controller once after default rl.
cat > "$ROOTMNT/etc/inittab" <<'EOF'
::sysinit:/sbin/openrc sysinit
::sysinit:/sbin/openrc boot
::wait:/sbin/openrc default
::wait:/usr/local/sbin/spike-controller.sh
ttyS0::respawn:/sbin/agetty -a root -L 115200 ttyS0 vt100
tty1::respawn:/sbin/agetty -a root 38400 tty1
::ctrlaltdel:/sbin/reboot
::shutdown:/sbin/openrc shutdown
EOF

log "Copying kernel + initramfs ONTO the shared ESP (/EFI/alpine)"
mkdir -p "$ESPMNT/EFI/alpine"
cp "$ROOTMNT/boot/vmlinuz-lts"    "$ESPMNT/EFI/alpine/vmlinuz-lts"
cp "$ROOTMNT/boot/initramfs-lts"  "$ESPMNT/EFI/alpine/initramfs-lts"
file "$ESPMNT/EFI/alpine/vmlinuz-lts" || true

log "Writing first-boot UEFI-shell bootstrap (startup.nsh)"
cat > "$ESPMNT/startup.nsh" <<EOF
@echo -off
echo APEX-OS spike-e: first-boot bootstrap, launching Alpine EFISTUB kernel...
fs0:
\\EFI\\alpine\\vmlinuz-lts initrd=\\EFI\\alpine\\initramfs-lts root=UUID=$INC_UUID rw rootfstype=ext4 console=tty0 console=ttyS0,115200
EOF
cat "$ESPMNT/startup.nsh"

log "ESP contents after build:"
find "$ESPMNT" | sort

sync
log "stage 10 done"

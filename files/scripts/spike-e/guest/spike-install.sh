#!/bin/sh
# Spike E — the actual `bootc install to-filesystem` invocation.
#
# Runs inside the incumbent VM (invoked by spike-controller.sh, stage=install).
# This is the reusable template for the REAL installs on the L16 / Katana:
# hand-prepared partitions + shared ESP, containerized bootc install.
#
# Target layout prepared here (non-LUKS variant):
#   /dev/vda3 (empty btrfs)  ->  /target            (new APEX-OS root)
#   /dev/vda1 (shared ESP)   ->  /target/boot/efi   (shared with the incumbent)
# bootc puts /boot on the root fs (no separate /boot). For the LUKS variant see
# docs/m0-results.md (needs a separate unencrypted /boot + --boot-mount-spec).
set -u

PAYLOAD=/payload
BOOTC_IMAGE="quay.io/fedora/fedora-bootc:43"
BOOTC_TAR="$PAYLOAD/fedora-bootc-43.tar"
. "$PAYLOAD/uuids.env"

echo "=== spike-install: environment ==="
uname -a
podman --version
echo "APEX_UUID=$APEX_UUID  INC_UUID=$INC_UUID  ESP_UUID=$ESP_UUID"

echo "=== ensuring cgroup v2 (unified) is mounted for podman/crun ==="
# The Alpine minirootfs does not mount cgroups by default; podman v5 + crun
# need cgroup2 at /sys/fs/cgroup. (On a Fedora live-USB install host this is
# already the case — this is a harness detail, not a real-install step.)
if ! awk '$2=="/sys/fs/cgroup" && $3=="cgroup2"{f=1} END{exit !f}' /proc/mounts; then
  umount -R /sys/fs/cgroup 2>/dev/null || true
  mount -t cgroup2 none /sys/fs/cgroup && echo "mounted cgroup2" || echo "WARN cgroup2 mount failed"
fi
export PODMAN_IGNORE_CGROUPSV1_WARNING=1
grep -E 'cgroup' /proc/mounts || true

echo "=== starting udev (bootc install correlates devices via /run/udev) ==="
# A normal install host (Fedora live-USB, a bootc system) runs systemd-udevd,
# so /run/udev exists and is populated. Alpine's minirootfs runs no udev, and
# bootc install aborts with "Comparing filesystems at /run/udev ...". Start
# eudev + populate the db so the container (bind-mounted below) sees it.
mkdir -p /run/udev
if command -v udevd >/dev/null 2>&1; then
  udevd --daemon 2>/dev/null || /sbin/udevd --daemon 2>/dev/null || true
  udevadm trigger 2>/dev/null || true
  udevadm settle --timeout=20 2>/dev/null || true
fi
ls /run/udev 2>/dev/null

echo "=== loading bootc image from payload tar ==="
if ! podman image exists "$BOOTC_IMAGE"; then
  podman load -i "$BOOTC_TAR"
fi
podman images

echo "=== mounting install target ==="
mkdir -p /target
mount /dev/vda3 /target
mkdir -p /target/boot/efi
mount /dev/vda1 /target/boot/efi
findmnt /target
findmnt /target/boot/efi

# Kernel args for the installed system. root= points at the apex btrfs; serial
# console so the host can see the bootc OS come up during the boot-test.
KARGS="--karg=root=UUID=$APEX_UUID --karg=rw --karg=console=tty0 --karg=console=ttyS0,115200"

run_install() {
  echo "=== bootc install to-filesystem (attempt: $1) ==="
  set -x
  podman run --rm --privileged --pid=host --network=host \
    -v /dev:/dev \
    -v /run/udev:/run/udev \
    -v /var/lib/containers:/var/lib/containers \
    -v /target:/target \
    "$BOOTC_IMAGE" \
    bootc install to-filesystem \
      $KARGS \
      --skip-fetch-check \
      $2 \
      /target
  rc=$?
  set +x
  return $rc
}

# Primary attempt: as faithful as possible to a real install (let bootc manage
# SELinux + bootloader normally).
run_install "primary" ""
rc=$?

if [ "$rc" -ne 0 ]; then
  echo "!!! primary attempt failed rc=$rc — retrying with --disable-selinux"
  echo "!!! (installing FROM a non-SELinux host; documented gotcha)"
  # clean any partial state on the target before retry
  umount /target/boot/efi 2>/dev/null || true
  umount /target 2>/dev/null || true
  wipefs -a /dev/vda3 2>/dev/null || true
  mkfs.btrfs -q -f -L apexroot /dev/vda3
  mount /dev/vda3 /target
  mkdir -p /target/boot/efi
  mount /dev/vda1 /target/boot/efi
  run_install "disable-selinux" "--disable-selinux"
  rc=$?
fi

echo "=== install rc=$rc ; unmounting target ==="
sync
umount /target/boot/efi 2>/dev/null || true
umount /target 2>/dev/null || true
echo "SPIKE-INSTALL-RC=$rc"
exit $rc

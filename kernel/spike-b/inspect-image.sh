#!/bin/bash
# Run all in-image spike checks against the built spike image.
set -uo pipefail
IMG="${1:-localhost/apex-kernel-spike:latest}"

sudo podman run --rm "$IMG" bash -c '
set -uo pipefail
KVER="$(cat /usr/lib/apex-cachyos-kver)"
CONF="/usr/lib/modules/${KVER}/config"
echo "########## KVER=${KVER}"
echo "########## uname vmlinuz present:"; ls -la /usr/lib/modules/${KVER}/vmlinuz /usr/lib/modules/${KVER}/initramfs.img

echo "########## CONFIG TABLE"
for opt in CONFIG_ANDROID_BINDERFS CONFIG_ANDROID_BINDER_IPC CONFIG_NTSYNC \
           CONFIG_SCHED_EXT CONFIG_SCHED_CLASS_EXT CONFIG_DRM_ACCEL_AMDXDNA \
           CONFIG_HZ CONFIG_HZ_1000 CONFIG_PREEMPT CONFIG_PREEMPT_VOLUNTARY \
           CONFIG_PREEMPT_DYNAMIC CONFIG_PREEMPT_LAZY CONFIG_FUTEX_WAITV; do
  line="$(grep -E "^${opt}=" "$CONF" 2>/dev/null)"
  if [ -n "$line" ]; then echo "$line"; else
    if grep -q "^# ${opt} is not set" "$CONF" 2>/dev/null; then echo "${opt}=(not set)"; else echo "${opt}=ABSENT"; fi
  fi
done

echo "########## NVIDIA AKMOD STATUS"
cat /usr/lib/apex-nvidia-akmod-status 2>&1 || echo "no status file"
echo "--- modinfo nvidia (against cachyos kver) ---"
modinfo -k "${KVER}" nvidia 2>&1 | grep -iE "^filename|^version|^vermagic|^license" || echo "modinfo nvidia FAILED to resolve"
echo "--- built kmod rpm installed? ---"
rpm -qa | grep -iE "kmod-nvidia|nvidia" 2>&1 || echo "no nvidia rpms"
echo "--- nvidia .ko file on disk? ---"
ls -la /usr/lib/modules/${KVER}/extra/nvidia/ 2>&1 || echo "no extra/nvidia dir"

echo "########## SCX VERSIONS"
rpm -q scx-scheds scx-tools 2>&1
echo "--- scx scheduler binaries ---"
ls /usr/bin/scx_* 2>&1 || echo "no scx_ binaries"
command -v scxctl scx_loader 2>&1 || echo "scxctl/scx_loader not on PATH"

echo "########## LAYERING SUPPORT"
echo "--- rpm-ostree present? ---"
if command -v rpm-ostree >/dev/null 2>&1; then rpm -q rpm-ostree; else echo "rpm-ostree ABSENT"; fi
echo "--- bootc version ---"; bootc --version
echo "--- dnf5 present? ---"; rpm -q dnf5 2>&1

echo "########## KARGS"
cat /usr/lib/bootc/kargs.d/10-apex-serial.toml 2>&1
'

#!/bin/sh
# APEX-OS M0 Spike E — shared config/helpers for the dual-boot to-filesystem rehearsal.
# Sourced by the numbered stage scripts. POSIX sh.
#
# SAFETY: every destructive operation here targets ONLY files inside $SPIKE_WORK
# and loop devices we attach to those files. Nothing touches the host ESP,
# host efibootmgr/NVRAM, host mounts, or the host bootloader.

set -eu

# Work dir holding all big artifacts (NOT in the git repo).
: "${SPIKE_WORK:=/home/andre/apex-os-m0-work/spike-e}"

# Inputs (fetched by the operator / earlier stages).
: "${ALPINE_VER:=3.24.1}"
: "${ALPINE_ROOTFS:=$SPIKE_WORK/alpine-minirootfs-${ALPINE_VER}-x86_64.tar.gz}"
: "${ALPINE_REPO:=https://dl-cdn.alpinelinux.org/alpine/latest-stable/main}"
: "${ALPINE_REPO_COMMUNITY:=https://dl-cdn.alpinelinux.org/alpine/latest-stable/community}"
: "${BOOTC_IMAGE:=quay.io/fedora/fedora-bootc:43}"
: "${BOOTC_IMAGE_TAR:=$SPIKE_WORK/fedora-bootc-43.tar}"

# Disk geometry.
: "${DISK_IMG:=$SPIKE_WORK/apex-dualboot.img}"
: "${DISK_SIZE:=30G}"
: "${ESP_SIZE:=1024MiB}"          # 1 GiB ESP (roomy, mimics a real shared ESP)
: "${INCUMBENT_SIZE:=12GiB}"      # incumbent (Alpine) root
# apex root = remainder of the disk

# Payload disk (carries the bootc image tar + in-guest scripts, receives output).
: "${PAYLOAD_IMG:=$SPIKE_WORK/payload.img}"
: "${PAYLOAD_SIZE:=6G}"

# Firmware.
: "${OVMF_CODE:=/usr/share/edk2/x64/OVMF_CODE.4m.fd}"
: "${OVMF_VARS_TEMPLATE:=/usr/share/edk2/x64/OVMF_VARS.4m.fd}"
: "${OVMF_VARS:=$SPIKE_WORK/OVMF_VARS.4m.fd}"   # per-VM writable NVRAM (persists BootOrder)

# VM.
: "${VM_RAM:=3072}"               # MiB, <= 3G per the spike constraint
: "${VM_CPUS:=2}"
: "${SERIAL_LOG:=$SPIKE_WORK/serial.log}"
: "${QMP_SOCK:=$SPIKE_WORK/qmp.sock}"

# Output capture dir (host side).
: "${OUT_DIR:=$SPIKE_WORK/out}"

log() { printf '\n>>> %s\n' "$*" >&2; }
die() { printf '!!! %s\n' "$*" >&2; exit 1; }
need_root() { [ "$(id -u)" -eq 0 ] || die "must run under sudo"; }

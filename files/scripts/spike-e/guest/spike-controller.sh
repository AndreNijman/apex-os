#!/bin/sh
# Spike E in-guest controller (runs inside the Alpine incumbent VM).
#
# Installed to /usr/local/sbin/spike-controller.sh and invoked once per boot
# from /etc/inittab (::wait:) after the default runlevel is up. It reads a
# single-word "stage" from the shared payload disk and performs that stage,
# writing artifacts back to the payload disk and echoing markers to the serial
# console so the host orchestrator can follow along. Every stage ends by
# powering off, returning control to the host between steps.
#
# Stages: setup | before | install | after | verify-incumbent
set -u

VDA=/dev/vda            # main dual-boot disk (ESP=vda1, alpine=vda2, apex=vda3)
ESP_PART=1
PAYLOAD=/dev/vdb        # payload disk (ext4): bootc image tar + scripts + out/
PMNT=/payload
ESPMNT=/boot/efi

# Force all controller output onto the serial port the host captures,
# independent of whichever console= the boot entry selected for /dev/console.
exec >/dev/ttyS0 2>&1 || true
banner() { printf '\n########## SPIKE-E %s ##########\n' "$*"; }
say()    { printf 'SPIKE: %s\n' "$*"; }

# --- bring up the pieces every stage needs -------------------------------
mount -t efivarfs efivarfs /sys/firmware/efi/efivars 2>/dev/null || true
modprobe ext4 2>/dev/null || true
mkdir -p "$PMNT" "$ESPMNT"
mount "$PAYLOAD" "$PMNT" 2>/dev/null || mount -t ext4 "$PAYLOAD" "$PMNT"
mkdir -p "$PMNT/out"
mount "$VDA$ESP_PART" "$ESPMNT" 2>/dev/null || mount -t vfat "$VDA$ESP_PART" "$ESPMNT" 2>/dev/null || true

STAGE="$(cat "$PMNT/stage" 2>/dev/null || echo none)"
banner "STAGE=$STAGE BEGIN"

. "$PMNT/uuids.env" 2>/dev/null || true

finish() {
  rc="${1:-0}"
  sync
  banner "STAGE=$STAGE DONE rc=$rc"
  umount "$ESPMNT" 2>/dev/null || true
  umount "$PMNT" 2>/dev/null || true
  sync
  poweroff -f
  # in case poweroff -f is slow
  sleep 5; /sbin/poweroff 2>/dev/null || true
  sleep 600
}

case "$STAGE" in
  setup)
    # First boot arrived via the UEFI-shell startup.nsh (or fallback loader).
    # Create the persistent classic-EFISTUB NVRAM entry that mimics the L16's
    # Void entry: loader = the kernel itself, cmdline (incl. initrd=) in
    # LoadOptions. Then make it first in BootOrder and drop the bootstrap.
    say "creating EFISTUB boot entry for the incumbent"
    # idempotent: remove any prior entry with our label
    for n in $(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*Alpine (incumbent).*/\1/p'); do
      efibootmgr -b "$n" -B >/dev/null 2>&1 || true
    done
    efibootmgr --create --disk "$VDA" --part "$ESP_PART" \
      --label "Alpine (incumbent)" \
      --loader '\EFI\alpine\vmlinuz-lts' \
      --unicode "initrd=\\EFI\\alpine\\initramfs-lts root=UUID=$INC_UUID rw rootfstype=ext4 console=tty0 console=ttyS0,115200"
    # put our entry first
    A="$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*Alpine (incumbent).*/\1/p' | head -1)"
    [ -n "$A" ] && efibootmgr -o "$A" || true
    # remove the first-boot bootstrap so the ESP "before" state is pure EFISTUB
    rm -f "$ESPMNT/startup.nsh"
    rm -rf "$ESPMNT/EFI/BOOT" "$ESPMNT/loader"
    efibootmgr -v > "$PMNT/out/setup-efibootmgr.txt" 2>&1
    say "EFISTUB entry created (Boot$A)"
    finish 0
    ;;

  before)
    say "recording BEFORE state"
    efibootmgr -v > "$PMNT/out/before-efibootmgr.txt" 2>&1
    { echo "# find $ESPMNT"; find "$ESPMNT" | sort; } > "$PMNT/out/before-esp-tree.txt" 2>&1
    { echo "# du -ab bytes"; du -ab "$ESPMNT" | sort -k2; } > "$PMNT/out/before-esp-du.txt" 2>&1
    { echo "# df"; df -h; echo; echo "# blkid"; blkid; } > "$PMNT/out/before-blk.txt" 2>&1
    say "BEFORE captured"
    finish 0
    ;;

  install)
    say "running bootc install to-filesystem (see install.log)"
    sh "$PMNT/spike-install.sh" 2>&1 | tee "$PMNT/out/install.log"
    rc=$?
    say "install finished rc=$rc"
    finish "$rc"
    ;;

  after)
    say "recording AFTER state"
    efibootmgr -v > "$PMNT/out/after-efibootmgr.txt" 2>&1
    { echo "# find $ESPMNT"; find "$ESPMNT" | sort; } > "$PMNT/out/after-esp-tree.txt" 2>&1
    { echo "# du -ab bytes"; du -ab "$ESPMNT" | sort -k2; } > "$PMNT/out/after-esp-du.txt" 2>&1
    # inspect the apex btrfs target that bootc wrote to
    mkdir -p /mnt/apex
    if mount "$VDA"3 /mnt/apex 2>/dev/null; then
      { echo "# apex root top-level"; ls -la /mnt/apex; \
        echo; echo "# ostree deploy dirs"; find /mnt/apex -maxdepth 4 -name '*.0' -o -maxdepth 4 -name 'deploy' 2>/dev/null | head; \
        echo; echo "# /boot inside apex root"; ls -la /mnt/apex/boot 2>/dev/null; } \
        > "$PMNT/out/after-apex-tree.txt" 2>&1
      umount /mnt/apex 2>/dev/null || true
    fi
    # Identify the bootc entry the install created (label typically contains
    # "Fedora" / "bootc" / "Linux") and the incumbent entry.
    efibootmgr -v > "$PMNT/out/after-efibootmgr-pristine.txt" 2>&1   # BootOrder as bootc left it
    A="$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*Alpine (incumbent).*/\1/p' | head -1)"
    B="$(efibootmgr | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*\(Fedora\|bootc\|Fedora Linux\|Linux\).*/\1/p' | grep -v "${A:-ZZZZ}" | head -1)"
    echo "incumbent_entry=$A bootc_entry=$B" > "$PMNT/out/entries.txt"
    say "entries: alpine=$A bootc=$B"
    # Orchestrate the two boot-tests without any host-side NVRAM editing:
    #   BootOrder = alpine first, bootc second (so the boot AFTER the next one
    #   returns to Alpine for verify-incumbent); BootNext = bootc (so the very
    #   next boot proves the bootc OS boots).
    if [ -n "$A" ] && [ -n "$B" ]; then
      efibootmgr -o "$A,$B" || true
      efibootmgr -n "$B"    || true
    fi
    efibootmgr -v > "$PMNT/out/after-efibootmgr-orchestrated.txt" 2>&1
    say "AFTER captured; BootNext=bootc set"
    finish 0
    ;;

  verify-incumbent)
    say "verifying the incumbent still boots"
    { echo "# uname"; uname -a; echo; echo "# /etc/os-release"; cat /etc/os-release; \
      echo; echo "# /proc/cmdline"; cat /proc/cmdline; \
      echo; echo "# efibootmgr -v"; efibootmgr -v; } \
      > "$PMNT/out/verify-incumbent.txt" 2>&1
    say "incumbent boot confirmed (Alpine $(uname -r))"
    finish 0
    ;;

  *)
    say "unknown stage '$STAGE' — powering off"
    finish 0
    ;;
esac

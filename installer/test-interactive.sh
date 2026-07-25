#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-interactive.sh — exercise apex-install's INTERACTIVE path end to end.
#
#  WHY: the QEMU boot-test only drives the *unattended* path (apex.unattended),
#  which skips disk enumeration and the dialogs entirely. A crash in the
#  interactive path therefore shipped unnoticed: `label="…$([ "$rm" = 1 ] && …)"`
#  exits non-zero for a NON-removable disk, which trips the ERR trap and killed
#  the installer while merely listing disks (apex-logs 41).
#
#  WHAT: runs the real apex-install with `whiptail` stubbed (canned answers) and
#  every destructive tool stubbed (podman/mount/useradd/…), so the complete shell
#  flow — enumeration, guards, dialogs, confirm, post-install — executes with the
#  ERR trap ARMED, against the real `lsblk` output of this machine. NOTHING is
#  written to any disk.
#
#  PASS = the script reaches "APEX-OS installed" and never prints
#         "Unexpected error on line".
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/apex-install"
# Which install mode to drive: `disk` (whole-disk wipe) or `partition` (dual-boot).
# `--both` runs each in turn; both must pass.
if [ "${1:-}" = "--both" ]; then
  rc_all=0
  for m in disk partition; do
    echo "######## TEST_MODE=$m ########"
    TEST_MODE="$m" bash "$0" || rc_all=1
  done
  exit "$rc_all"
fi
TEST_MODE="${TEST_MODE:-disk}"
TMP=$(mktemp -d); BIN="$TMP/bin"; mkdir -p "$BIN"
trap 'rm -rf "$TMP"' EXIT

# Target the stubs will "install" to — a fake block node inside the scratch dir.
# NEVER under /dev (see the guard below).
FAKE_DISK="${FAKE_DISK:-$TMP/fakedisk}"

# ── stub: whiptail (answers by --title; whiptail returns its result on stderr) ─
cat > "$BIN/whiptail" <<STUB
#!/usr/bin/env bash
title=""; prev=""
for a in "\$@"; do [ "\$prev" = "--title" ] && title="\$a"; prev="\$a"; done
# Echo every dialog to stdout so die()'s message (rendered via --msgbox) is
# visible to the assertions instead of being swallowed by the stub.
echo "[dialog] title=\$title :: \$*"
case "\$title" in
  *"Select the target disk"*)      printf '%s' "$FAKE_DISK" >&2 ;;
  *"How should"*)                  printf '%s' "$TEST_MODE"  >&2 ;;
  *"Select the target PARTITION"*) printf '%s' "${FAKE_DISK}2" >&2 ;;
  *"Create your account"*)         printf '%s' "andre"      >&2 ;;
  *Password*)                      printf '%s' "testpass"   >&2 ;;
  *Hostname*)                      printf '%s' "apex"       >&2 ;;
  *"Type to confirm"*)             printf '%s' "ERASE"      >&2 ;;
esac
exit 0
STUB

# ── stubs: everything that could touch a real disk / real accounts ────────────
for t in podman mount umount useradd chpasswd chroot setfiles poweroff sync partprobe \
         mkfs.btrfs mkfs.ext4 mkfs.xfs udevadm blkid; do
  printf '#!/usr/bin/env bash\necho "[stub] %s $*"\nexit 0\n' "$t" > "$BIN/$t"
done
# `podman image exists` must succeed so preflight passes.
printf '#!/usr/bin/env bash\necho "[stub] podman $*"\nexit 0\n' > "$BIN/podman"
# lsblk stays REAL for enumeration (that is the code under test), except the
# post-install partition lookup on the fake disk.
# Emulate a realistic target disk:
#   <fake>1 = EFI System Partition (must never be offered / never overwritten)
#   <fake>2 = the installable partition
#   <fake>3 = an "other OS" partition that must be preserved and listed as kept
cat > "$BIN/lsblk" <<STUB
#!/usr/bin/env bash
args="\$*"
d="$FAKE_DISK"; b=\$(basename "\$d")
case "\$args" in
  *"\$d"*) ;;
  *) exec /usr/bin/lsblk "\$@" ;;
esac
# whole-disk queries
case "\$args" in
  *NAME,PARTTYPE*)      echo "\${b}1 c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
                        echo "\${b}2 0fc63daf-8483-4772-8e79-3d69d8477de4"
                        echo "\${b}3 0fc63daf-8483-4772-8e79-3d69d8477de4"; exit 0 ;;
  *NAME,TYPE*)          echo "\${b}1 part"; echo "\${b}2 part"; echo "\${b}3 part"; exit 0 ;;
  *NAME,SIZE,FSTYPE,LABEL*) echo "\${b}1 512M vfat ESP"
                            echo "\${b}2 40G btrfs apex"
                            echo "\${b}3 100G ext4 OTHER_OS"; exit 0 ;;
  *SERIAL,WWN*)         echo "FAKESERIAL 0xfake"; exit 0 ;;
esac
# per-partition queries
case "\$args" in
  *"\${d}2"*|*"\${d}3"*|*"\${d}1"*)
    case "\$args" in
      *MOUNTPOINT*) echo ""; exit 0 ;;
      *-bdno*SIZE*|*-bdno\ SIZE*) echo "42949672960"; exit 0 ;;
      *FSTYPE*)     echo "btrfs"; exit 0 ;;
      *LABEL*)      echo "apex"; exit 0 ;;
      *SIZE*)       echo "40G"; exit 0 ;;
    esac ;;
esac
case "\$args" in *SIZE*) echo "40G"; exit 0 ;; esac
exec /usr/bin/lsblk "\$@"
STUB
# `[ -b \$DISK ]` must pass for the fake target.
cat > "$BIN/test" <<'STUB'
#!/usr/bin/env bash
exec /usr/bin/test "$@"
STUB
# mktemp -d → a prepared tree containing a fake ostree deployment, so the
# post-install account step finds a deploy dir and runs its real logic.
DEPLOY="$TMP/root/ostree/deploy/default/deploy/abc.0"
mkdir -p "$DEPLOY/etc" "$DEPLOY/var/home" "$TMP/root/ostree/deploy/default/var"
# The installer now REFUSES to finish without the SELinux relabel tooling in the
# target (an unlabeled /etc/passwd = login denied = unusable system). Provide the
# fixture so the happy path can complete; `chroot`/`setfiles` are stubbed inert.
mkdir -p "$DEPLOY/usr/sbin" "$DEPLOY/etc/selinux/targeted/contexts/files"
printf 'SELINUXTYPE=targeted\n' > "$DEPLOY/etc/selinux/config"
: > "$DEPLOY/etc/selinux/targeted/contexts/files/file_contexts"
printf '#!/bin/sh\nexit 0\n' > "$DEPLOY/usr/sbin/setfiles"; chmod +x "$DEPLOY/usr/sbin/setfiles"
printf 'root:x:0:0::/root:/bin/bash\n' > "$DEPLOY/etc/passwd"
cat > "$BIN/mktemp" <<STUB
#!/usr/bin/env bash
if [ "\${1:-}" = "-d" ]; then echo "$TMP/root"; else exec /usr/bin/mktemp "\$@"; fi
STUB
chmod +x "$BIN"/*

echo "== running apex-install (interactive path, all writes stubbed) =="
OUT="$TMP/out.txt"
# The target must satisfy `[ -b "$DISK" ]`. Create the fake block node INSIDE the
# scratch dir — never under /dev.
#   Previously this did `mknod /dev/apex-test-fake b 7 200` (major 7 = loop, i.e.
#   it aliased /dev/loop200) and then `rm -f "$FAKE_DISK"` unconditionally — so
#   running with FAKE_DISK=/dev/nvme0n1 as root would have DELETED that device
#   node. Refuse anything outside the scratch dir, and only remove what we made.
case "$FAKE_DISK" in
  "$TMP"/*) : ;;
  *) echo "REFUSING: FAKE_DISK must live inside the scratch dir ($TMP), got '$FAKE_DISK'"; exit 1 ;;
esac
[ -e "$FAKE_DISK" ] && { echo "REFUSING: $FAKE_DISK already exists"; exit 1; }
created_node=0
if [ "$(id -u)" = 0 ]; then
  # major 259 minor 4095: an unused blk-ext number, and it is inside $TMP anyway.
  mknod "$FAKE_DISK" b 259 4095 2>/dev/null && created_node=1
  # Partition nodes too: partition mode checks `[ -b "$TARGET" ]` on <disk>2.
  for i in 1 2 3; do mknod "${FAKE_DISK}$i" b 259 $((4090+i)) 2>/dev/null || true; done
fi
[ "$created_node" = 1 ] || { echo "SKIP: need root to create the fake block node"; exit 0; }

PATH="$BIN:$PATH" APEX_OCI_DIR=/nonexistent bash "$SCRIPT" >"$OUT" 2>&1
rc=$?
[ "$created_node" = 1 ] && rm -f "$FAKE_DISK"

echo "--- output ---"; cat "$OUT"; echo "--------------"
fail=0
if grep -q "Unexpected error on line" "$OUT"; then
  echo "FAIL: ERR trap fired — $(grep -o 'Unexpected error on line [0-9]*' "$OUT" | head -1)"; fail=1
fi
if grep -q "APEX-INSTALL-FAILED" "$OUT"; then
  echo "FAIL: installer reported failure"; fail=1
fi
if ! grep -q "APEX-OS installed" "$OUT"; then
  echo "FAIL: never reached the completion message (rc=$rc)"; fail=1
fi
[ "$fail" = 0 ] && { echo "PASS: interactive path completed with the ERR trap armed."; exit 0; }
exit 1

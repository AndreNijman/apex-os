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
TMP=$(mktemp -d); BIN="$TMP/bin"; mkdir -p "$BIN"
trap 'rm -rf "$TMP"' EXIT

# Target the stubs will "install" to — a fake path, never a real device.
FAKE_DISK="${FAKE_DISK:-/dev/apex-test-fake}"

# ── stub: whiptail (answers by --title; whiptail returns its result on stderr) ─
cat > "$BIN/whiptail" <<STUB
#!/usr/bin/env bash
title=""; prev=""
for a in "\$@"; do [ "\$prev" = "--title" ] && title="\$a"; prev="\$a"; done
# Echo every dialog to stdout so die()'s message (rendered via --msgbox) is
# visible to the assertions instead of being swallowed by the stub.
echo "[dialog] title=\$title :: \$*"
case "\$title" in
  *"Select the target disk"*) printf '%s' "$FAKE_DISK" >&2 ;;
  *"Create your account"*)    printf '%s' "andre"      >&2 ;;
  *Password*)                 printf '%s' "testpass"   >&2 ;;
  *Hostname*)                 printf '%s' "apex"       >&2 ;;
  *"Type to confirm"*)        printf '%s' "ERASE"      >&2 ;;
esac
exit 0
STUB

# ── stubs: everything that could touch a real disk / real accounts ────────────
for t in podman mount umount useradd chpasswd chroot setfiles poweroff sync partprobe; do
  printf '#!/usr/bin/env bash\necho "[stub] %s $*"\nexit 0\n' "$t" > "$BIN/$t"
done
# `podman image exists` must succeed so preflight passes.
printf '#!/usr/bin/env bash\necho "[stub] podman $*"\nexit 0\n' > "$BIN/podman"
# lsblk stays REAL for enumeration (that is the code under test), except the
# post-install partition lookup on the fake disk.
cat > "$BIN/lsblk" <<STUB
#!/usr/bin/env bash
for a in "\$@"; do
  case "\$a" in "$FAKE_DISK") echo "apex-test-fake1"; exit 0 ;; esac
done
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
cat > "$BIN/mktemp" <<STUB
#!/usr/bin/env bash
if [ "\${1:-}" = "-d" ]; then echo "$TMP/root"; else exec /usr/bin/mktemp "\$@"; fi
STUB
chmod +x "$BIN"/*

echo "== running apex-install (interactive path, all writes stubbed) =="
OUT="$TMP/out.txt"
# `-b $FAKE_DISK` must be true → run the script with a bash whose `test` builtin
# we cannot stub, so create the fake device node instead (root) or fall back to
# a regular file + accept the guard message.
if [ "$(id -u)" = 0 ] && [ ! -e "$FAKE_DISK" ]; then
  mknod "$FAKE_DISK" b 7 200 2>/dev/null || true
fi

PATH="$BIN:$PATH" APEX_OCI_DIR=/nonexistent bash "$SCRIPT" >"$OUT" 2>&1
rc=$?
[ -e "$FAKE_DISK" ] && rm -f "$FAKE_DISK"

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

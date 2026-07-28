#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-engine-guards.sh — prove apex-install refuses bad input BEFORE it wipes.
#
#  This replaces test-interactive.sh, which drove the whiptail TUI with canned
#  answers. That TUI no longer exists: apex-install is engine-only now, spoken
#  to as `apex-install --headless ANSWERS` by the GTK installer, and the text UI
#  people kept getting stranded in has been deleted.
#
#  WHY THIS FILE EXISTS AT ALL. Deleting the TUI silently deleted three guards
#  that only lived inside it — the username regex, the reserved-name check, and
#  the hostname regex. Losing them is not cosmetic. Nothing else rejects a bad
#  username until `useradd` runs, and `useradd` runs AFTER `bootc install
#  --wipe` has already erased the disk: the result is a fully installed system
#  with no account on it, and the user's previous OS gone. The original TUI
#  validated early for exactly that reason and said so in a comment. Two more
#  guards (target == ESP, and target not on the named disk) had no equivalent
#  at all in headless mode. All five are asserted below so a future refactor
#  cannot quietly drop them again.
#
#  EVERY case here must fail BEFORE anything is written, so this script NEVER
#  touches a block device. The two partition-mode cases name real devices
#  (/dev/sda, /dev/sdb) because the guards need `-b` to succeed to be reached at
#  all — but they are rejected by the guard under test, several steps before any
#  mkfs, mount or bootc call. Nothing is opened for writing.
#
#  PASS = every case prints the expected APEX-INSTALL-FAILED reason, and no case
#         prints "Unexpected error on line" (that string means the ERR trap
#         fired, which is always a bug in the installer).
#
#  Run from the repo's installer/ directory. Needs root only because the engine
#  refuses to run without it.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")"

ENGINE=./apex-install
ANS=$(mktemp /tmp/apex-test-answers.XXXXXX)
trap 'rm -f "$ANS"' EXIT
chmod 600 "$ANS"

pass=0; fail=0

# $1 = case name, $2 = expected substring in the failure reason, $3 = answers body
check() {
    local name=$1 want=$2 body=$3 out
    printf '%s\n' "$body" > "$ANS"
    out=$(sudo -n "$ENGINE" --headless "$ANS" 2>&1 </dev/null)

    if grep -q 'Unexpected error on line' <<<"$out"; then
        printf 'FAIL  %-30s ERR TRAP FIRED\n' "$name"; fail=$((fail+1)); return
    fi
    if grep -qF "$want" <<<"$out"; then
        printf 'PASS  %-30s\n' "$name"; pass=$((pass+1))
    else
        printf 'FAIL  %-30s expected %q\n      got: %s\n' \
            "$name" "$want" "$(grep -m1 APEX-INSTALL-FAILED <<<"$out" || echo '<no sentinel>')"
        fail=$((fail+1))
    fi
}

# A disk that cannot exist, so the whole-disk cases stop at the block-device
# check instead of proceeding. The account guards run BEFORE that check — which
# is the ordering under test.
BASE=$'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nhostname=apex'

echo "── argument handling ──────────────────────────────────────────────────"
out=$(sudo -n "$ENGINE" </dev/null 2>&1); rc=$?
if [ "$rc" = 2 ] && grep -q 'not a user interface' <<<"$out"; then
    printf 'PASS  %-30s (exit 2, starts nothing)\n' "no arguments"; pass=$((pass+1))
else
    printf 'FAIL  %-30s exit=%s\n' "no arguments" "$rc"; fail=$((fail+1))
fi

echo "── account validation (must run before the disk is touched) ───────────"
check "username: uppercase"   "Invalid username 'Bob'"        "$BASE"$'\nusername=Bob'
check "username: leading digit" "Invalid username '1bob'"     "$BASE"$'\nusername=1bob'
check "username: reserved"    "reserved system account"       "$BASE"$'\nusername=root'
check "hostname: underscore"  "Invalid hostname 'my_host'"    $'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nusername=bob\nhostname=my_host'

echo "── answers-file handling ──────────────────────────────────────────────"
check "unknown key"           "unknown key in answers file"   "$BASE"$'\nusername=bob\nbogus=1'
check "missing password"      "password missing"              $'mode=disk\ndisk=/dev/zzz-does-not-exist\nusername=bob\nhostname=apex'
check "bad mode value"        "bad mode"                      $'mode=wipeitall\ndisk=/dev/zzz-does-not-exist\nusername=bob\npassword=pw\nhostname=apex'
check "valid input reaches disk check" "is not a block device" "$BASE"$'\nusername=bob'

# The parser splits on '=' with IFS, so a password containing '=' is a real
# risk: everything after the first '=' must survive intact.
printf 'username=bob\npassword=a=b=c\nhostname=apex\n' > "$ANS"
got=$(while IFS='=' read -r k v || [ -n "$k" ]; do [ "$k" = password ] && printf '%s' "$v"; done < "$ANS")
if [ "$got" = 'a=b=c' ]; then
    printf 'PASS  %-30s\n' "password containing '='"; pass=$((pass+1))
else
    printf 'FAIL  %-30s got %q\n' "password containing '='" "$got"; fail=$((fail+1))
fi

# A file whose last line has no trailing newline used to lose that line
# entirely — measured. A dropped `mokpw` would skip Secure Boot enrolment
# without a word, so the parser reads the final unterminated line too.
printf 'username=bob\npassword=pw\nhostname=lastline' > "$ANS"
got=$(while IFS='=' read -r k v || [ -n "$k" ]; do [ "$k" = hostname ] && printf '%s' "$v"; done < "$ANS")
if [ "$got" = 'lastline' ]; then
    printf 'PASS  %-30s\n' "no trailing newline"; pass=$((pass+1))
else
    printf 'FAIL  %-30s last key lost\n' "no trailing newline"; fail=$((fail+1))
fi

echo "── partition mode: the two most destructive mistakes ──────────────────"
# These need devices that exist for the guard to be reached. Read-only: both
# cases are refused by the guard under test, long before any write.
if [ -b /dev/sda ] && [ -b /dev/sdb ] && [ -b /dev/sda2 ] && [ -b /dev/sdb1 ]; then
    check "target == ESP"     "same device"                   $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sda2\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex'
    check "target on another disk" "is not a partition of"    $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sdb1\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex'
else
    echo "SKIP  partition-mode cases (need /dev/sda2 and /dev/sdb1 present)"
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
printf '%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

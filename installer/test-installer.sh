#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-installer.sh — test what actually ships: the engine's input guards,
#  and that every page of the GTK installer really draws.
#
#  (Renamed from test-engine-guards.sh when the GUI half was added. That file
#  in turn replaced test-interactive.sh, which drove the whiptail TUI with
#  canned answers — the TUI no longer exists: apex-install is engine-only now,
#  spoken to as `apex-install --headless ANSWERS` by the GTK installer, and
#  the text UI people kept getting stranded in has been deleted.)
#
#  ── Half 1: the engine refuses bad input BEFORE it wipes ────────────────────
#  Deleting the TUI silently deleted three guards that only lived inside it —
#  the username regex, the reserved-name check, and the hostname regex. Losing
#  them is not cosmetic. Nothing else rejects a bad username until `useradd`
#  runs, and `useradd` runs AFTER `bootc install --wipe` has already erased the
#  disk: the result is a fully installed system with no account on it, and the
#  user's previous OS gone. The original TUI validated early for exactly that
#  reason and said so in a comment. Two more guards (target == ESP, and target
#  not on the named disk) had no equivalent at all in headless mode. All five
#  are asserted below so a future refactor cannot quietly drop them again.
#
#  EVERY case here must fail BEFORE anything is written, so this half NEVER
#  touches a block device. The two partition-mode cases name real devices
#  (/dev/sda, /dev/sdb) because the guards need `-b` to succeed to be reached at
#  all — but they are rejected by the guard under test, several steps before any
#  mkfs, mount or bootc call. Nothing is opened for writing.
#
#  ── Half 2: every GUI page must draw ────────────────────────────────────────
#  The GUI is now the ONLY front end. If a page fails to render, or lays out so
#  its buttons land off-screen, the user is stranded with no fallback — and a
#  syntax-clean file proves nothing about either. So every page named in the
#  GUI's own registry is rendered headless (cage + wlroots-headless + grim in
#  the apex-guitest container) and the screenshot is measured, not just stat'd:
#  a produced PNG is NOT a pass — a blank or single-colour frame means the page
#  did not draw. Pages render at 1024x600 and 1366x768, the realistic
#  worst-case laptop panels; one page already clipped its action row at 720 px
#  (measured), which is exactly the failure class this half exists to catch.
#  Pixel checks alone are not enough, though: GTK prefers to SQUASH mid-page
#  widgets over pushing the action row off-screen (measured: at 1024x600 the
#  account page swallows the Computer-name field whole, buttons still visible),
#  so every page is also measured — GTK is asked for the page's minimum height
#  at each panel width, and it must fit. The exact pass criteria are documented
#  inline below. No disk, real or virtual, is enumerated (lsblk is stubbed
#  inside the container), let alone touched.
#
#  PASS = every engine case prints its expected APEX-INSTALL-FAILED reason and
#         never "Unexpected error on line" (that string means the ERR trap
#         fired, which is always a bug in the installer), and every GUI page
#         passes every render check at every size.
#
#  Run from the repo's installer/ directory. Needs passwordless root (sudo -n):
#  the engine refuses to run unprivileged, and the render container lives in
#  ROOT podman storage (built here on first run if missing).
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")"

ENGINE=./apex-install
ANS=$(mktemp /tmp/apex-test-answers.XXXXXX)

# ── Getting the engine as far as its own guards ──────────────────────────────
#
# Every case below feeds the engine an answers file and expects a named refusal.
# None of them could reach one. apex-install:353 refuses to continue unless the
# APEX-OS image is present in ROOT podman storage, and that check runs BEFORE
# argument parsing — so on any machine that is not the ISO build box the engine
# died at preflight and every case in the three engine sections reported the
# same "image is not present" text instead of the guard under test.
#
# That was not a regression. `git log -S` puts the image check in dddabd6f
# (2026-07-23) and these cases in 33b744d5, five days later: they were written
# against an engine that already refused them, and only ever passed where root
# podman storage happened to hold localhost/apex-os:daily. pr-validation.yml
# runs this suite on a bare ubuntu-24.04 runner, so they were dead in CI too.
# The tell that needs no theory: the "no arguments" case asserts exit 2, and
# preflight's die() exits 1.
#
# apex-install:56 is IMAGE="${APEX_IMAGE:-localhost/apex-os:${EDITION}}", with
# the comment "override with APEX_IMAGE=... for testing". An empty tar imported
# by podman is a valid image with no layers — no network, no build, removed
# again on exit, so the suite does not depend on the ambient store either.
#
# sudo's env_reset strips APEX_* from the caller's environment, so this must be
# passed as `sudo -n APEX_IMAGE=...` on each invocation and cannot be exported.
SCRATCH_IMAGE="localhost/apex-engine-probe:test"
ENGINE_IMAGE=""
scratch_made=0
BUILD_CTX=""
LOOP_IMG=""
LOOP_DEV=""
cleanup() {
    rm -f "$ANS"
    # shellcheck disable=SC2033  # the real losetup; the stub further down is scoped to one case
    [ -n "$LOOP_DEV" ] && sudo -n losetup -d "$LOOP_DEV" 2>/dev/null || true
    [ -n "$LOOP_IMG" ] && rm -f "$LOOP_IMG"
    [ -n "$BUILD_CTX" ] && rm -rf "$BUILD_CTX"
    [ "$scratch_made" = 1 ] && sudo -n podman rmi -f "$SCRATCH_IMAGE" >/dev/null 2>&1
}
trap cleanup EXIT
chmod 600 "$ANS"

ensure_engine_image() {
    command -v podman >/dev/null 2>&1 || return 1
    sudo -n true 2>/dev/null || return 1
    if sudo -n podman image exists localhost/apex-os:daily 2>/dev/null; then
        ENGINE_IMAGE="localhost/apex-os:daily"; return 0
    fi
    local t; t=$(mktemp /tmp/apex-empty.XXXXXX.tar) || return 1
    tar -cf "$t" -T /dev/null 2>/dev/null \
        && sudo -n podman import -q "$t" "$SCRATCH_IMAGE" >/dev/null 2>&1
    local rc=$?
    rm -f "$t"
    [ "$rc" = 0 ] || return 1
    scratch_made=1
    ENGINE_IMAGE="$SCRATCH_IMAGE"
    return 0
}

pass=0; fail=0

# Skipping here is honest and failing is not: with no image the engine cannot be
# exercised at all, and a suite that reports 20 failures on a laptop teaches
# people to ignore it. But it must be LOUD, because a silent skip of the engine
# half is how this went unnoticed for six weeks.
ENGINE_RUNNABLE=1
if ! ensure_engine_image; then
    ENGINE_RUNNABLE=0
    echo "SKIP: the engine half cannot run here — preflight needs an APEX-OS image in"
    echo "      ROOT podman storage and neither one nor passwordless podman is available."
fi

# $1 = case name, $2 = expected substring in the failure reason, $3 = answers body
check() {
    local name=$1 want=$2 body=$3 out
    if [ "$ENGINE_RUNNABLE" != 1 ]; then
        printf 'SKIP  %-30s no engine image\n' "$name"; return
    fi
    printf '%s\n' "$body" > "$ANS"
    out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 "$ENGINE" --headless "$ANS" 2>&1 </dev/null)

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
# `encrypt=no` is here because the engine now REFUSES an answers file that
# does not say, one way or the other, whether to encrypt the disk. It is not
# a default this suite is choosing: a missing key is its own refusal, and
# installer/test-installer-luks.sh is the suite that asserts that. Without
# it every case below would stop at the encryption question instead of the
# guard it is actually testing.
BASE=$'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nhostname=apex\nencrypt=no'

echo "── argument handling ──────────────────────────────────────────────────"
out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" "$ENGINE" </dev/null 2>&1); rc=$?
if [ "$ENGINE_RUNNABLE" != 1 ]; then
    printf 'SKIP  %-30s no engine image\n' "no arguments"
elif [ "$rc" = 2 ] && grep -q 'not a user interface' <<<"$out"; then
    printf 'PASS  %-30s (exit 2, starts nothing)\n' "no arguments"; pass=$((pass+1))
else
    printf 'FAIL  %-30s exit=%s\n' "no arguments" "$rc"; fail=$((fail+1))
fi

echo "── account validation (must run before the disk is touched) ───────────"
check "username: uppercase"   "Invalid username 'Bob'"        "$BASE"$'\nusername=Bob'
check "username: leading digit" "Invalid username '1bob'"     "$BASE"$'\nusername=1bob'
check "username: reserved"    "reserved system account"       "$BASE"$'\nusername=root'
check "hostname: underscore"  "Invalid hostname 'my_host'"    $'mode=disk\ndisk=/dev/zzz-does-not-exist\npassword=pw\nusername=bob\nhostname=my_host\nencrypt=no'

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
    check "target == ESP"     "same device"                   $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sda2\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex\nencrypt=no'
    check "target on another disk" "is not a partition of"    $'mode=partition\ndisk=/dev/sda\ntarget=/dev/sdb1\nesp=/dev/sda2\nusername=bob\npassword=pw\nhostname=apex\nencrypt=no'
else
    echo "SKIP  partition-mode cases (need /dev/sda2 and /dev/sdb1 present)"
fi

echo "── final confirmation: binds the exact device before any write ───────"
if [ "$ENGINE_RUNNABLE" = 1 ] && command -v losetup >/dev/null \
   && LOOP_IMG=$(mktemp /var/tmp/apex-confirm-loop.XXXXXX); then
    truncate -s 18G "$LOOP_IMG"
    # shellcheck disable=SC2033  # the real losetup, deliberately (see cleanup)
    LOOP_DEV=$(sudo -n losetup --find --show "$LOOP_IMG" 2>/dev/null || true)
    if [ -n "$LOOP_DEV" ]; then
        fp=$(lsblk -bdnP -o MAJ:MIN,SIZE,WWN,SERIAL,PTUUID,PARTUUID,PARTTYPE "$LOOP_DEV")
        base=$(printf 'mode=disk\ndisk=%s\nusername=bob\npassword=pw\nhostname=apex\nencrypt=no\n' "$LOOP_DEV")
        check "missing typed confirmation" "The final confirmation (typing ERASE) is missing" "$base"
        check "wrong confirmed target" "The confirmation was typed for" \
            "$base"$'\nconfirmed=ERASE\nconfirm_target=/dev/not-this-loop\n'"confirm_disk_id=$fp"
        check "changed disk identity" "is not the disk that was confirmed" \
            "$base"$'\nconfirmed=ERASE\n'"confirm_target=$LOOP_DEV"$'\nconfirm_disk_id=changed'
        printf '%s\nconfirmed=ERASE\nconfirm_target=%s\nconfirm_disk_id=%s\n' \
            "$base" "$LOOP_DEV" "$fp" > "$ANS"
        out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 \
              "$ENGINE" --headless "$ANS" 2>&1 </dev/null)
        if grep -q 'APEX-INSTALL-DRYRUN-OK' <<<"$out"; then
            printf 'PASS  %-30s\n' "exact device dry run"; pass=$((pass+1))
        else
            printf 'FAIL  %-30s %s\n' "exact device dry run" \
                "$(grep -m1 APEX-INSTALL-FAILED <<<"$out" || echo no-sentinel)"
            fail=$((fail+1))
        fi
        # The GUI can die mid-install and the engine must still finish. Same
        # dry run, with stdout and stderr on a pipe whose reader is already
        # gone. Before the relay, the bare `echo` of the final sentinel failed
        # there and the ERR trap recorded a finished install as a failure.
        _rc=$(python3 -c 'import os, subprocess, sys
r, w = os.pipe(); os.close(r)
print(subprocess.call(sys.argv[1:], stdin=subprocess.DEVNULL, stdout=w, stderr=w))' \
              sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 "$ENGINE" --headless "$ANS")
        if [ "$_rc" = 0 ] && sudo -n grep -q 'APEX-DRY-RUN: validation complete' /var/log/apex-install.log; then
            printf 'PASS  %-30s\n' "engine outlives a dead GUI"; pass=$((pass+1))
        else
            printf 'FAIL  %-30s rc=%s %s\n' "engine outlives a dead GUI" "$_rc" \
                "$(sudo -n tail -1 /var/log/apex-install.log 2>/dev/null)"; fail=$((fail+1))
        fi
        # Partition mode binds THREE identities — disk, root partition, ESP —
        # and each one on its own must be able to stop the install. The disk
        # below mimics a dual-boot layout (ESP, a partition for APEX, a
        # partition that must survive), all inside the sparse loop image
        # allocated above; the engine stays in dry-run mode throughout.
        if [[ "$LOOP_DEV" == /dev/loop* ]] && command -v sgdisk >/dev/null \
           && command -v mkfs.vfat >/dev/null; then
            sudo -n sgdisk --zap-all "$LOOP_DEV" >/dev/null 2>&1
            sudo -n sgdisk -n1:0:+300M -t1:ef00 -c1:"EFI system partition" \
                -n2:0:+14G -t2:8300 -c2:apex-root \
                -n3:0:0 -t3:0700 -c3:"Basic data partition" "$LOOP_DEV" >/dev/null 2>&1
            sudo -n partprobe "$LOOP_DEV" >/dev/null 2>&1
            sudo -n udevadm settle --timeout=10 >/dev/null 2>&1
            esp="${LOOP_DEV}p1"; target="${LOOP_DEV}p2"; kept="${LOOP_DEV}p3"
            if [ -b "$esp" ] && [ -b "$target" ] && [ -b "$kept" ]; then
                sudo -n mkfs.vfat -F32 -n SYSTEM "$esp" >/dev/null 2>&1
                disk_fp=$(lsblk -bdnP -o MAJ:MIN,SIZE,WWN,SERIAL,PTUUID,PARTUUID,PARTTYPE "$LOOP_DEV")
                target_fp=$(lsblk -bdnP -o MAJ:MIN,SIZE,WWN,SERIAL,PTUUID,PARTUUID,PARTTYPE "$target")
                esp_fp=$(lsblk -bdnP -o MAJ:MIN,SIZE,WWN,SERIAL,PTUUID,PARTUUID,PARTTYPE "$esp")
                kept_fp=$(lsblk -bdnP -o MAJ:MIN,SIZE,WWN,SERIAL,PTUUID,PARTUUID,PARTTYPE "$kept")
                pbase=$(printf 'mode=partition\ndisk=%s\ntarget=%s\nesp=%s\nusername=bob\npassword=pw\nhostname=apex\nencrypt=no\nconfirmed=ERASE\nconfirm_target=%s\nconfirm_disk_id=%s\nconfirm_target_id=%s\nconfirm_esp_id=%s\n' \
                    "$LOOP_DEV" "$target" "$esp" "$target" "$disk_fp" "$target_fp" "$esp_fp")
                check "changed root identity" "is not the partition that was confirmed" \
                    "${pbase/confirm_target_id=$target_fp/confirm_target_id=changed}"
                check "changed ESP identity" "is not the EFI System Partition that was confirmed" \
                    "${pbase/confirm_esp_id=$esp_fp/confirm_esp_id=changed}"
                # The confirmation named p2; an answers file that now says p3
                # (the partition that must survive) is the renamed-device case
                # in miniature, and must be refused on the name alone.
                check "target swapped after confirm" "The confirmation was typed for" \
                    "${pbase/target=$target/target=$kept}"
                # …and a correct NAME carrying another partition's identity is
                # refused on the identity, which is the case a name check misses.
                check "identity of a kept partition" "is not the partition that was confirmed" \
                    "${pbase/confirm_target_id=$target_fp/confirm_target_id=$kept_fp}"
                printf '%s\n' "$pbase" > "$ANS"
                out=$(sudo -n APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 \
                      "$ENGINE" --headless "$ANS" 2>&1 </dev/null)
                if grep -q 'APEX-INSTALL-DRYRUN-OK' <<<"$out"; then
                    printf 'PASS  %-30s\n' "partition dry run"; pass=$((pass+1))
                else
                    printf 'FAIL  %-30s %s\n' "partition dry run" \
                        "$(grep -m1 APEX-INSTALL-FAILED <<<"$out" || echo no-sentinel)"
                    fail=$((fail+1))
                fi
            else
                echo "SKIP  partition confirmation cases (loop partitions unavailable)"
            fi
        fi
    else
        echo "SKIP  confirmation loop tests (no free loop device)"
    fi
else
    echo "SKIP  confirmation loop tests (no engine or losetup)"
fi

echo
echo "── netinstall staging: never RAM, never the disk being wiped ──────────"
# A network install used to `podman pull` into the live environment's
# containers-storage, which on a booted ISO is the RAM overlay: ~12 GB of
# decompressed layers in RAM, plus several more when bootc re-tarred them into
# /var/tmp. It staged to disk instead, and these guard the chooser that decides
# WHERE. Two of them are the difference between a working install and a
# destroyed one:
#
#   * a tmpfs must never be chosen — that IS the RAM overlay, the whole bug;
#   * the disk about to be repartitioned must never be chosen — staging onto it
#     means bootc wipes the image out from under itself mid-install.
#
# The functions are sourced out of the shipped engine rather than copied, so
# this tests what installs, not a paraphrase of it.
_fns=$(mktemp /tmp/apex-scratch-fns.XXXXXX)
sed -n '/^scratch_fs_ok()/,/^}/p;/^pick_scratch()/,/^}/p;/^stage_budget_kb()/,/^}/p;/^stage_setup()/,/^}/p;/^stage_teardown()/,/^}/p' "$ENGINE" > "$_fns"
if [ ! -s "$_fns" ]; then
    printf 'FAIL  %-30s could not extract the chooser from %s\n' "scratch chooser" "$ENGINE"
    fail=$((fail+1))
else
(
    set +u
    NEED_SCRATCH_GB=32
    DISK=/dev/sdz
    # shellcheck disable=SC1090
    . "$_fns"
    _p=0; _f=0
    _ck() {  # name, got, want
        if [ "$2" = "$3" ]; then printf 'PASS  %-30s\n' "$1"; _p=$((_p+1))
        else printf 'FAIL  %-30s want %s got %s\n' "$1" "$3" "$2"; _f=$((_f+1)); fi
    }
    mkdir -p /dev/shm/apex-scratch-test
    scratch_fs_ok /dev/shm/apex-scratch-test && r=yes || r=no
    _ck "tmpfs refused"              "$r" no
    scratch_fs_ok /var/tmp && r=yes || r=no
    _ck "real filesystem accepted"   "$r" yes
    # shellcheck disable=SC2034  # read by scratch_fs_ok, sourced above
    ( NEED_SCRATCH_GB=999999; scratch_fs_ok /var/tmp ) && r=yes || r=no
    _ck "too small refused"          "$r" no
    scratch_fs_ok /no/such/dir && r=yes || r=no
    _ck "missing directory refused"  "$r" no
    # shellcheck disable=SC2034  # read by scratch_fs_ok, sourced above
    ( DISK=$(df -P /var/tmp | awk 'NR==2{print $1}'); scratch_fs_ok /var/tmp ) && r=yes || r=no
    _ck "target disk refused"        "$r" no
    mkdir -p /var/tmp/apex-scratch-ovr
    out=$(APEX_OCI_SCRATCH=/var/tmp/apex-scratch-ovr pick_scratch || true)
    _ck "override honoured"          "$out" /var/tmp/apex-scratch-ovr
    out=$(APEX_OCI_SCRATCH=/dev/shm/apex-scratch-test pick_scratch || true)
    _ck "override onto tmpfs refused" "${out:-<empty>}" "<empty>"

    # ── The machine this installer will meet most often ────────────────────
    # A laptop with ONE internal disk, booted from a plain single-partition
    # USB. Nothing qualifies, by construction: the live session automounts
    # nothing under /run/media because the stick has no second partition,
    # /mnt and /media are empty, and /var/tmp IS the RAM overlay. Until the
    # fallback existed the chooser answered with nothing and the install
    # aborted, telling the user to get "the full offline ISO" — which has
    # never been published. The candidate list is substituted here rather
    # than simulated so the case is the real chooser's answer to the real
    # shape of that machine.
    out=$( APEX_SCRATCH_CANDIDATES="/dev/shm/apex-scratch-test /no/such/dir" \
           pick_scratch || true )
    _ck "single-disk USB falls back"  "${out:-<empty>}" "@target"
    # …and the tmpfs in that list was refused on the way past, not chosen:
    # falling back to RAM is the bug 63857891 fixed and this must not undo.
    _ck "fallback is not the tmpfs"   "$(printf '%s' "$out" | grep -c '/dev/shm' || true)" 0
    # A real scratch volume still wins — the fallback is a fallback.
    out=$( APEX_SCRATCH_CANDIDATES="/dev/shm/apex-scratch-test /var/tmp" \
           pick_scratch || true )
    _ck "spare volume still preferred" "${out:-<empty>}" "/var/tmp/apex-install-scratch"
    rmdir /dev/shm/apex-scratch-test /var/tmp/apex-scratch-ovr 2>/dev/null

    # ── How much of the target the download may take ───────────────────────
    # The only part of staging-on-target that can be exercised without a block
    # device, and the part that decides whether a disk is erased for nothing.
    # STAGE_RESERVE_GB is what keeps room for the OS itself: the staged blobs
    # and the installed system are on the same filesystem at the same time.
    STAGE_RESERVE_GB=15
    _gb() { echo $(( $1 * 1024 * 1024 )); }
    out=$(stage_budget_kb "$(_gb 200)" || echo REFUSED)
    _ck "200 GB target accepted"      "$out" "$(_gb 185)"
    out=$(stage_budget_kb "$(_gb 47)" || echo REFUSED)
    _ck "47 GB target accepted"       "$out" "$(_gb 32)"
    out=$(stage_budget_kb "$(_gb 46)" || echo REFUSED)
    _ck "46 GB target refused"        "$out" REFUSED
    out=$(stage_budget_kb "$(_gb 20)" || echo REFUSED)
    _ck "20 GB target refused"        "$out" REFUSED
    out=$(stage_budget_kb "not-a-number" || echo REFUSED)
    _ck "unreadable free space refused" "$out" REFUSED

    # ── The staging image must leave the target root PRISTINE ──────────────
    # `bootc install to-filesystem` refuses a target that is not empty — its
    # own error says "Requiring directory contains only mount points" — so the
    # backing file is created on the target, handed to a loop device and then
    # UNLINKED. Nothing here opens a block device: losetup, mkfs.xfs and mount
    # are stubbed, and the only real work is the sparse file, which the
    # function under test is supposed to remove. If the unlink is ever
    # simplified out, every staged install fails on hardware and nothing else
    # in this suite would notice.
    _st=$(mktemp -d /var/tmp/apex-stage-probe.XXXXXX)
    (
      # shellcheck disable=SC2034  # LOG and STAGE_DIR are read by stage_setup,
      # which is sourced from the engine above, not defined here.
      LOG=/dev/null
      log() { :; }
      losetup() { echo /dev/loop-probe; }
      mkfs.xfs() { :; }
      mount()    { :; }
      df()       { command df "$@"; }
      # shellcheck disable=SC2034
      STAGE_DIR="$_st/mnt"
      # Bound the sparse ceiling to just over the engine's own budget, whatever
      # this machine has free. Tied to NEED_SCRATCH_GB rather than a number: a
      # fixed 25 GB ceiling went stale the day the budget moved to 32 and this
      # case reported SETUP-FAILED for a stage_setup that was fine.
      _avail_gb=$(command df -PBG "$_st" | awk 'NR==2{gsub(/G/,"",$4); print $4+0}')
      STAGE_RESERVE_GB=$(( _avail_gb - NEED_SCRATCH_GB - 3 ))
      [ "$STAGE_RESERVE_GB" -ge 1 ] || STAGE_RESERVE_GB=1
      mkdir -p "$_st/root"
      stage_setup "$_st/root" >/dev/null 2>&1 || { echo "SETUP-FAILED"; exit 0; }
      [ -e "$_st/root/.apex-stage.img" ] && echo "LEFT-BEHIND" && exit 0
      [ "$STAGE_TMPDIR" = "$_st/mnt/tmp" ] || { echo "TMPDIR=$STAGE_TMPDIR"; exit 0; }
      printf '%s\n' "${STAGE_BOOTC_ARGS[*]}"
    ) > "$_st/out" 2>&1
    _ck "staging image is unlinked"   "$(cat "$_st/out")" "--skip-finalize"
    rm -rf "$_st"
    echo "$_p $_f" > /tmp/apex-scratch-counts
)
read -r _sp _sf < /tmp/apex-scratch-counts 2>/dev/null || { _sp=0; _sf=1; }
pass=$((pass + _sp)); fail=$((fail + _sf))
rm -f "$_fns" /tmp/apex-scratch-counts
fi

# What the engine must and must not say about staging.
#
# The refusal these three assertions used to guard — "There is nowhere to put
# the download", with "the full offline ISO" named as the way out — was a dead
# end: that ISO has never been published, and on the commonest machine this
# installer meets there was no other way forward either. It is GONE, and its
# absence is asserted, because reintroducing it would put the dead end back.
for _gone in "There is nowhere to put the download" \
             "Use the full offline ISO. It carries the OS and needs no staging at all."; do
    if grep -qF "$_gone" "$ENGINE"; then
        printf 'FAIL  %-30s the dead-end refusal is back in the engine\n' "no dead end: ${_gone:0:18}"; fail=$((fail+1))
    else
        printf 'PASS  %-30s\n' "no dead end: ${_gone:0:18}"; pass=$((pass+1))
    fi
done
# And what must be there instead.
#
#   * the reassurance the external-scratch path can still honestly give;
#   * the warning the fallback path must give in its place, because there the
#     download and the destruction are the same step;
#   * the reachability probe that is the last free check before the wipe;
#   * --skip-finalize, without which a completely successful staged install
#     reports as a failure (the loop device holds a writable fd, so bootc's
#     closing remount-read-only fails with EBUSY).
for _want in "Nothing has been erased" \
             "downloads onto it as it goes" \
             "skopeo inspect --raw" \
             "--skip-finalize"; do
    if grep -qF -- "$_want" "$ENGINE"; then
        printf 'PASS  %-30s\n' "engine says: ${_want:0:22}"; pass=$((pass+1))
    else
        printf 'FAIL  %-30s missing from the engine\n' "engine says: ${_want:0:22}"; fail=$((fail+1))
    fi
done

# And the engine must not have quietly kept the old RAM-filling path.
if grep -qE '^\s*if podman pull' "$ENGINE"; then
    printf 'FAIL  %-30s engine still uses `podman pull` to fetch the OS\n' "no podman pull"; fail=$((fail+1))
else
    printf 'PASS  %-30s\n' "no podman pull"; pass=$((pass+1))
fi

# Every `skopeo copy` must name its temp dir. TMPDIR alone does not reach the
# containers-storage destination: skopeo 1.22 takes that from containers.conf's
# image_copy_tmp_dir (/var/tmp), which on a live ISO is the 5.3 GB overlay.
# A VM install from the netinstall ISO filled it and died silently at
# "Preparing the installer runtime"; a host run cannot see this, because the
# host's /var/tmp is huge. So it is checked here, statically, on every line.
_bare=$(grep -nE '^\s*(if\s+)?skopeo\s+copy' "$ENGINE" || true)
_named=$(grep -cE '^\s*(if\s+)?skopeo\s+"\$\{SKOPEO_TMP\[@\]\}"\s+copy' "$ENGINE" || true)
if [ -n "$_bare" ]; then
    printf 'FAIL  %-30s %s\n' "skopeo copy names --tmpdir" "bare skopeo copy at line(s): $(cut -d: -f1 <<<"$_bare" | tr '\n' ' ')"; fail=$((fail+1))
elif [ "${_named:-0}" -lt 2 ]; then
    printf 'FAIL  %-30s %s\n' "skopeo copy names --tmpdir" "expected both netinstall copies to pass SKOPEO_TMP, found $_named"; fail=$((fail+1))
else
    printf 'PASS  %-30s\n' "skopeo copy names --tmpdir"; pass=$((pass+1))
fi

# A published netinstall ISO downloads a pinned DIGEST. Once :apex moves on,
# that digest is untagged, and deleting untagged package versions would break
# every ISO in the wild at its first pull. Two halves: no workflow may delete
# package versions, and every release pins its digest with a durable
# netinstall-<release> tag through pin-netinstall-image.yml (write-once, and
# byte-for-byte: --preserve-digests).
_wf=../.github/workflows
# Comment lines are skipped: explaining why deletion is dangerous is not deletion.
_del=$(grep -nE 'delete-package-versions|/packages/container/[^[:space:]]*/versions/|(-X|--method)[[:space:]]+DELETE[^#]*packages' "$_wf"/*.yml 2>/dev/null \
       | grep -vE '^[^:]+:[0-9]+:[[:space:]]*#' || true)
if [ -n "$_del" ]; then
    printf 'FAIL  %-30s %s\n' "no GHCR version deletion" "$(head -1 <<<"$_del")"; fail=$((fail+1))
else
    printf 'PASS  %-30s\n' "no GHCR version deletion"; pass=$((pass+1))
fi
echo "── one engine at a time, and a front end that died can reattach ───────"
# A VT switch can take cage (and so the GUI) down mid-install while the engine
# keeps writing. The fresh front end must never start a second engine, and a
# second engine must refuse before it touches the first one's log or mounts.
_lock_ln=$(grep -n 'flock -n 9' "$ENGINE" | head -1 | cut -d: -f1)
_trunc_ln=$(grep -n '^: > "\$LOG"' "$ENGINE" | head -1 | cut -d: -f1)
_um_ln=$(grep -n '^unmount_target$' "$ENGINE" | head -1 | cut -d: -f1)
if [ -n "$_lock_ln" ] && [ -n "$_trunc_ln" ] && [ -n "$_um_ln" ] \
   && [ "$_lock_ln" -lt "$_trunc_ln" ] && [ "$_lock_ln" -lt "$_um_ln" ]; then
    printf 'PASS  %-30s\n' "lock before log and mounts"; pass=$((pass+1))
else
    printf 'FAIL  %-30s %s\n' "lock before log and mounts" "flock at ${_lock_ln:-?}, log truncation at ${_trunc_ln:-?}, unmount_target at ${_um_ln:-?}"; fail=$((fail+1))
fi
if [ "$ENGINE_RUNNABLE" = 1 ] && command -v flock >/dev/null; then
    _L=/run/apex-install-test.$$.lock
    sudo -n sh -c 'printf "log of the install that is running\n" > /var/log/apex-install.log'
    sudo -n timeout 20 flock "$_L" sleep 20 & _holder=$!
    sleep 1
    printf 'mode=disk\ndisk=/dev/null\nusername=bob\npassword=pw\nhostname=apex\nencrypt=no\n' > "$ANS"
    out=$(sudo -n APEX_INSTALL_LOCK="$_L" APEX_IMAGE="$ENGINE_IMAGE" APEX_DRY_RUN=1 "$ENGINE" --headless "$ANS" 2>&1 </dev/null); _rc=$?
    kill "$_holder" 2>/dev/null; wait "$_holder" 2>/dev/null
    if [ "$_rc" = 1 ] && grep -q 'already running' <<<"$out" \
       && sudo -n grep -q 'log of the install that is running' /var/log/apex-install.log; then
        printf 'PASS  %-30s\n' "second engine refuses cleanly"; pass=$((pass+1))
    else
        printf 'FAIL  %-30s rc=%s %s\n' "second engine refuses cleanly" "$_rc" "$(tail -1 <<<"$out")"; fail=$((fail+1))
    fi
    sudo -n rm -f "$_L"
else
    echo "SKIP  second engine refuses cleanly (no engine or flock)"
fi
# The result record a reattaching front end reads: root-only, and it carries
# the recovery key (the only on-machine copy once the front end is gone).
_rf=$(mktemp -d /var/tmp/apex-result-test.XXXXXX)
(
    set +u
    eval "$(sed -n '/^write_result() {/,/^}/p' "$ENGINE")"
    RESULT_ON=1; RESULT_FILE="$_rf/sub/result"
    INSTALL_MODE=disk; DISK=/dev/vda; TARGET=/dev/vda; USERNAME=bob; HOSTNAME=apex
    RESULT_RECOVERY_KEY=abcd-efgh; RESULT_RECOVERY_SAVED=apex-recovery-key-apex.txt; RESULT_RECOVERY_UNSAVED=
    write_result ok ""
    stat -c %a "$RESULT_FILE"; cat "$RESULT_FILE"
) > "$_rf/out" 2>&1
if grep -qx 600 "$_rf/out" && grep -qx 'status=ok' "$_rf/out" && grep -qx 'recovery_key=abcd-efgh' "$_rf/out" \
   && grep -qx 'username=bob' "$_rf/out"; then
    printf 'PASS  %-30s\n' "engine records its result"; pass=$((pass+1))
else
    printf 'FAIL  %-30s %s\n' "engine records its result" "$(tr '\n' ' ' < "$_rf/out")"; fail=$((fail+1))
fi
# ...and the GUI reads it back into the state its done page draws from.
printf 'status=failed\nmessage=disk went away\nmode=disk\ndisk=/dev/vda\ntarget=\nusername=bob\nhostname=apex\nrecovery_key=abcd\nrecovery_saved=\nrecovery_unsaved=x\n' > "$_rf/result"
if APEX_RESULT_FILE="$_rf/result" python3 -c "
import os, re, subprocess
src = open('apex-installer-gui').read()
g = {'os': os, 're': re, 'subprocess': subprocess}
exec(compile(src[src.index('ENGINE = '):src.index('def netinstall')].replace('ENGINE = ', 'ENGINE_ = ', 1), 'gui', 'exec'), g)
r = g['read_result']()
assert r and r['status'] == 'failed' and r['message'] == 'disk went away' and r['recovery_key'] == 'abcd', r
" 2>"$_rf/pyerr"; then
    printf 'PASS  %-30s\n' "GUI reads the result back"; pass=$((pass+1))
else
    printf 'FAIL  %-30s %s\n' "GUI reads the result back" "$(tail -1 "$_rf/pyerr")"; fail=$((fail+1))
fi
# A withdrawn pinned image is not a network failure, and must not say it is.
(
    set +u
    eval "$(sed -n '/^pinned_image_gone() {/,/^}/p' "$ENGINE")"
    LOG="$_rf/log"; printf 'reading manifest sha256:00 in ghcr.io/andrenijman/apex-os: manifest unknown\n' > "$LOG"
    NET_SOURCE_IMAGE=ghcr.io/andrenijman/apex-os@sha256:00; TARGET_IMAGE=ghcr.io/andrenijman/apex-os:apex
    pinned_image_gone && echo GONE-PINNED
    NET_SOURCE_IMAGE=$TARGET_IMAGE
    pinned_image_gone || echo UNPINNED-NOT-GONE
    printf 'dial tcp: lookup ghcr.io: no such host\n' > "$LOG"; NET_SOURCE_IMAGE=ghcr.io/andrenijman/apex-os@sha256:00
    pinned_image_gone || echo OFFLINE-NOT-GONE
) > "$_rf/gone" 2>&1
if [ "$(tr '\n' ' ' < "$_rf/gone")" = "GONE-PINNED UNPINNED-NOT-GONE OFFLINE-NOT-GONE " ]; then
    printf 'PASS  %-30s\n' "withdrawn image told apart"; pass=$((pass+1))
else
    printf 'FAIL  %-30s %s\n' "withdrawn image told apart" "$(tr '\n' ' ' < "$_rf/gone")"; fail=$((fail+1))
fi
# A relaunched session must land on tty1. seatd binds it to whichever VT is in
# front, and after a crash that is the VT the user switched to: in a VM the
# relaunched GUI came up on tty2 while tty1 showed boot messages. So the
# launcher brings tty1 forward before every start, and a chvt that never
# returns must not keep cage from starting.
mkdir -p "$_rf/bin"
printf '#!/bin/sh\necho "chvt $*" >> "%s/order"\n' "$_rf" > "$_rf/bin/chvt"
printf '#!/bin/sh\necho gui >> "%s/order"\n' "$_rf" > "$_rf/bin/gui"
chmod +x "$_rf/bin/chvt" "$_rf/bin/gui"
_launch_fns="$(sed -n '/^front_tty1() {/,/^}/p; /^start_gui() {/,/^}/p' apex-installer-launch)"
( set +u; PATH="$_rf/bin:$PATH"; LOG="$_rf/launch.log"; log() { :; }; GUI_CMD=("$_rf/bin/gui")
  eval "$_launch_fns"; start_gui 1 ) >/dev/null 2>&1
_order="$(tr '\n' ' ' < "$_rf/order" 2>/dev/null)"
printf '#!/bin/sh\nexec sleep 30\n' > "$_rf/bin/chvt"; : > "$_rf/order"
_t0=$SECONDS
( set +u; PATH="$_rf/bin:$PATH"; LOG="$_rf/launch.log"; log() { :; }; GUI_CMD=("$_rf/bin/gui")
  eval "$_launch_fns"; start_gui 2 ) >/dev/null 2>&1
_hung=$((SECONDS - _t0))
if [ "$_order" = "chvt 1 gui " ] && grep -qx gui "$_rf/order" && [ "$_hung" -le 10 ]; then
    printf 'PASS  %-30s\n' "relaunch lands on tty1"; pass=$((pass+1))
else
    printf 'FAIL  %-30s order=[%s] hung-chvt start took %ss\n' "relaunch lands on tty1" "$_order" "$_hung"; fail=$((fail+1))
fi
rm -rf "$_rf"

_pin="$_wf/pin-netinstall-image.yml"
if [ -f "$_pin" ] && grep -q -- '--preserve-digests' "$_pin" \
   && grep -q 'netinstall-\$RELEASE' "$_pin" && grep -q 'write-once' "$_pin" \
   && grep -q 'pin-netinstall-image.yml' ./build-live-iso.sh; then
    printf 'PASS  %-30s\n' "netinstall digest gets pinned"; pass=$((pass+1))
else
    printf 'FAIL  %-30s %s\n' "netinstall digest gets pinned" \
        "pin-netinstall-image.yml missing or not write-once/--preserve-digests, or build-live-iso.sh no longer says to run it"
    fail=$((fail+1))
fi

echo "── GUI: every page must draw — it is the only front end there is ──────"

GUI=./apex-installer-gui
GUITEST=localhost/apex-guitest:latest       # gtk4/libadwaita/cage/grim/python3-cairo
RANDR=localhost/apex-guitest-randr:latest   # + wlr-randr, to drive the output geometry
SIZES="1024x600 1366x768"

# The page list comes from the GUI's own registry (the add_named loop in
# startup()), never from a list here that would rot the first time a page is
# added — the secureboot page appeared exactly that way.
PAGES=$(sed -n '/for name, build in (/,/):/p' "$GUI" \
        | grep -oE '"[a-z]+"' | tr -d '"' | awk '!seen[$0]++' | xargs)

gui_skip=0
case " $PAGES " in
    *" welcome "*) [ "$(wc -w <<<"$PAGES")" -ge 6 ] || gui_skip=1 ;;
    *) gui_skip=1 ;;
esac
if [ "$gui_skip" = 1 ]; then
    printf 'FAIL  %-30s could not read the page registry from %s (got: "%s")\n' \
        "gui: page registry" "$GUI" "$PAGES"
    fail=$((fail+1))
fi

# The render images live in ROOT podman storage; build them here if absent so
# the test is runnable on a fresh machine. wlroots' headless output is
# hard-wired to 1280x720 — the only supported way to get another geometry is
# the wlr-output-management protocol, which cage speaks and wlr-randr drives.
# Hence the one-package derived image.
# An EMPTY build context, made here rather than named. This used to be
# /var/empty, which exists on Fedora and does NOT exist on a stock
# ubuntu-24.04 GitHub runner — so `podman build` failed instantly with a
# missing-context error, and because the build's output went to /dev/null the
# suite reported only "could not build", with the actual reason discarded.
#
# That went unnoticed because this job is gated on installer changes and the
# roadmap branches had not touched installer/ until now. It is a pre-existing
# defect surfaced by this branch, not one it introduced.
BUILD_CTX=$(mktemp -d /tmp/apex-guitest-ctx.XXXXXX)
build_img() {   # build_img <tag> <containerfile-on-stdin>
    local tag=$1 err
    if err=$(sudo -n podman build -t "$tag" -f - "$BUILD_CTX" 2>&1 >/dev/null); then
        return 0
    fi
    # The reason, not just the verdict. A build that fails for no stated cause
    # is the shape of bug this whole file exists to prevent.
    printf 'FAIL  %-30s could not build %s\n' "gui: render image" "$tag"
    printf '      podman said: %s\n' "$(printf '%s' "$err" | tail -3 | tr '\n' ' ')"
    fail=$((fail+1)); gui_skip=1
    return 1
}

if [ "$gui_skip" = 0 ] && ! sudo -n podman image exists "$GUITEST" 2>/dev/null; then
    echo "      ($GUITEST missing — building it, first run only)"
    printf 'FROM registry.fedoraproject.org/fedora:43\nRUN dnf install -y cage gtk4 libadwaita python3-gobject gobject-introspection python3-cairo cairo-gobject mesa-dri-drivers seatd grim && dnf clean all\n' \
        | build_img "$GUITEST"
fi
if [ "$gui_skip" = 0 ] && ! sudo -n podman image exists "$RANDR" 2>/dev/null; then
    printf 'FROM %s\nRUN dnf install -y wlr-randr && dnf clean all\n' "$GUITEST" \
        | build_img "$RANDR"
fi

if [ "$gui_skip" = 0 ]; then
    WORK=$(mktemp -d /tmp/apex-gui-render.XXXXXX)
    mkdir -p "$WORK/gui" "$WORK/stub"
    # SELinux denies the container read access to $HOME even :ro, so the GUI is
    # copied beside the output dir and the whole thing is mounted :Z. The copy
    # is made fresh every run — it IS the file under test, just relabelled.
    cp "$GUI" "$WORK/gui/apex-installer-gui"

    # Stub lsblk (PATH-first inside the container): the container has no disks,
    # which would render only the empty-state pages. This presents a realistic
    # dual-boot table — ESP + Windows + Linux + a crypto_LUKS partition — so
    # disk/mode/part/confirm draw their full lists, including the blocked
    # container-member row and confirm's ERASED/KEPT/SHARED verdicts. Nothing
    # real is enumerated, let alone touched.
    cat > "$WORK/stub/lsblk" <<'STUB'
#!/usr/bin/env bash
case "$*" in
  *NAME,MOUNTPOINT*) exit 0 ;;   # live-media scan: nothing here is live media
  *NAME,SIZE,TYPE,MODEL,TRAN,RM,SERIAL*)
    echo 'NAME="vda" SIZE="512G" TYPE="disk" MODEL="APEX Test SSD" TRAN="nvme" RM="0" SERIAL="APXTEST01"'; exit 0 ;;
  *NAME,TYPE,SIZE,FSTYPE,LABEL,PARTTYPE*)
    printf '%s\n' \
      'vda1 part 512M vfat ESP c12a7328-f81f-11d2-ba4b-00a0c93ec93b' \
      'vda2 part 220G ntfs Windows ebd0a0a2-b9e5-4433-87c0-68b6b72699c7' \
      'vda3 part 240G btrfs Linux 0fc63daf-8483-4772-8e79-3d69d8477de4' \
      'vda4 part 50G crypto_LUKS vault 0fc63daf-8483-4772-8e79-3d69d8477de4'; exit 0 ;;
  *TYPE,SIZE,FSTYPE,LABEL*)
    printf '%s\n' 'part 512M vfat ESP' 'part 220G ntfs Windows' \
                  'part 240G btrfs Linux' 'part 50G crypto_LUKS vault'; exit 0 ;;
esac
exit 0
STUB

    # Inert engine stand-in for the run page. Without it the GUI's spawn of
    # /usr/bin/apex-install fails instantly and the page bounces to "done"
    # before grim fires — the screenshot would show the wrong page. It emits
    # the two status lines the page displays, then idles. Touches nothing.
    cat > "$WORK/stub/apex-install" <<'STUB'
#!/usr/bin/env bash
echo "Installing APEX-OS to /dev/vda3 (partition of /dev/vda) … (full log: /var/log/apex-install.log)"
echo "Do not power off — /dev/vda3 is being erased and rewritten from here on."
sleep 300
STUB
    # Exec bits matter: a non-executable stub is silently SKIPPED by the PATH
    # search and the REAL lsblk answers instead — measured: the disk page came
    # back listing this machine's actual drives through the container's /sys.
    chmod +x "$WORK/stub/"*

    # Measures every screenshot. One line per PNG:
    #   METRIC <file> <w> <h> <bytes> <ncolours> <bottom_clean> <actionpx> <right_clean>
    #
    # Criteria (thresholds applied by the shell below):
    #  ncolours ≥ 32 and ≥ 10000 bytes — "the page drew". A rendered page has
    #    HUNDREDS of distinct colours from font antialiasing alone (welcome
    #    measures ~700, 44 KB); a frame where GTK died is the compositor's
    #    solid fill: 1 colour, a few KB of PNG. The thresholds sit far from
    #    both, so neither theme tweaks nor compression changes can flip them.
    #  bottom_clean — frame() gives every page a 32 px background-only margin
    #    BELOW the action row. Any non-background pixel in the bottom 12 rows
    #    means the column overflowed the window and was clipped — the buttons
    #    are (at least partly) off-screen. The GUI is the only front end, so an
    #    unreachable Continue is a stranded user; this is that detector.
    #  actionpx ≥ 150 — non-background pixels in rows [h-90, h-12). A visible
    #    action row (min-height-44 buttons sitting directly above the margin)
    #    paints thousands there. This closes the one hole in bottom_clean: an
    #    overflow that happens to cut inside the background gap just ABOVE the
    #    buttons leaves the bottom strip clean while the buttons are still
    #    off-screen. The run page has no buttons by design (an install must not
    #    be abortable mid-write); its "Do not power off" caption occupies the
    #    same band, so the check holds there too.
    #  right_clean — the same idea sideways: a three-button action row that
    #    does not fit paints the last 8 columns; catches horizontal clipping.
    cat > "$WORK/analyze.py" <<'PY'
import cairo, os
OUT = "/out"
for f in sorted(os.listdir(OUT)):
    if not f.endswith(".png"):
        continue
    p = os.path.join(OUT, f)
    s = cairo.ImageSurface.create_from_png(p)
    w, h, stride = s.get_width(), s.get_height(), s.get_stride()
    ints = memoryview(bytes(s.get_data())).cast("I")  # one uint32 per pixel
    spx = stride // 4
    bg = ints[0]  # (0,0) sits inside the page's top margin: always background
    colours = set()
    for y in range(h):
        colours.update(ints[y*spx : y*spx + w])
    bottom_clean = int(all(v == bg for y in range(h-12, h)
                           for v in ints[y*spx : y*spx + w]))
    right_clean = int(all(ints[y*spx + x] == bg
                          for y in range(h) for x in range(w-8, w)))
    actionpx = sum(1 for y in range(max(0, h-90), h-12)
                   for v in ints[y*spx : y*spx + w] if v != bg)
    print("METRIC", f, w, h, os.path.getsize(p), len(colours),
          bottom_clean, actionpx, right_clean)
PY

    # The pixel checks cannot see a widget squashed in the MIDDLE of a page:
    # when a page is taller than the panel, GTK shrinks body children below
    # their minimum instead of pushing the action row off — measured at
    # 1024x600, where the account page kept its buttons but swallowed the
    # Computer-name entry whole. So ask GTK itself: import the real GUI (its
    # __main__ guard makes that safe), build every page from the builders
    # registry, and print each page's MINIMUM height at each panel width. A
    # page whose minimum exceeds the panel height cannot be laid out without
    # squashing or clipping something — that is the assertion.
    cat > "$WORK/measure.py" <<'PY'
import importlib.util, os
from importlib.machinery import SourceFileLoader
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Gtk

# SourceFileLoader explicitly: the GUI has no .py extension, so
# spec_from_file_location alone cannot infer a loader for it.
loader = SourceFileLoader("apexgui", "/out/gui/apex-installer-gui")
spec = importlib.util.spec_from_loader("apexgui", loader)
mod = importlib.util.module_from_spec(spec)
loader.exec_module(mod)

widths = [int(x) for x in os.environ["MEASURE_WIDTHS"].split()]
app = mod.Installer()

def measure(_app):
    # Runs after the GUI's own activate handler, so builders exist and the
    # APEX_GUI_* state has been seeded exactly as in a jump-to-page render.
    for name, build in app.builders.items():
        page = build()
        for w in widths:
            print("MEASURE", name, w, page.measure(Gtk.Orientation.VERTICAL, w)[0],
                  flush=True)
    app.quit()

app.connect("activate", measure)
app.run(None)
PY

    # Runs INSIDE the container: for each geometry × page, start cage on a
    # headless output, let the first client resize it with wlr-randr, exec the
    # real GUI jumped to the page via APEX_GUI_PAGE (its documented test
    # affordance), screenshot with grim, tear down. Measure everything at the
    # end in one pass.
    cat > "$WORK/inner.sh" <<'INNER'
#!/usr/bin/env bash
set -u
sizes=$1; pages=$2
export XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export GSK_RENDERER=cairo GDK_BACKEND=wayland LIBGL_ALWAYS_SOFTWARE=1
export PATH=/out/stub:$PATH
install -m 0755 /out/stub/apex-install /usr/bin/apex-install
# Jump-to-page state: a partition-mode install of /dev/vda3, so confirm shows
# a per-partition verdict list and done shows the partition-mode success text.
export APEX_GUI_MODE=partition APEX_GUI_DISK=/dev/vda \
       APEX_GUI_TARGET=/dev/vda3 APEX_GUI_ESP=/dev/vda1 APEX_GUI_OK=1
for size in $sizes; do
  for p in $pages; do
    rm -f "$XDG_RUNTIME_DIR"/wayland*   # fresh socket → grim finds wayland-0
    APEX_GUI_PAGE=$p timeout 30 cage -- bash -c \
      "wlr-randr --output HEADLESS-1 --custom-mode $size >/dev/null 2>&1; sleep 1; exec python3 /out/gui/apex-installer-gui" \
      2>/dev/null &
    cpid=$!
    sleep 6                             # measured: first frame lands well within this
    grim "/out/$size-$p.png" 2>/dev/null || echo "RENDER-FAIL $size-$p"
    kill "$cpid" 2>/dev/null; wait "$cpid" 2>/dev/null
  done
done
# Layout audit (see measure.py): one more cage session, no screenshot — the
# client measures every page at every panel width and prints MEASURE lines.
rm -f "$XDG_RUNTIME_DIR"/wayland*
MEASURE_WIDTHS="$(for s in $sizes; do printf '%s ' "${s%x*}"; done)" \
  APEX_GUI_PAGE=confirm timeout 60 cage -- python3 /out/measure.py 2>/dev/null
exec python3 /out/analyze.py
INNER

    sudo -n podman run --rm --network=none -v "$WORK":/out:Z "$RANDR" \
        bash /out/inner.sh "$SIZES" "$PAGES" >"$WORK/render.log" 2>&1 || true

    for size in $SIZES; do
        for p in $PAGES; do
            name="gui: $p @ $size"
            line=$(grep -m1 "^METRIC $size-$p\.png " "$WORK/render.log" || true)
            if [ -z "$line" ]; then
                printf 'FAIL  %-30s no screenshot produced (see %s/render.log)\n' \
                    "$name" "$WORK"
                fail=$((fail+1)); continue
            fi
            read -r _ _ w h bytes ncolours bclean apx rclean <<<"$line"
            why=""
            [ "${w}x${h}" = "$size" ] \
                || why="rendered ${w}x${h}, wanted $size (mode-set failed)"
            if [ "$ncolours" -lt 32 ] || [ "$bytes" -lt 10000 ]; then
                why="${why:+$why; }blank frame ($ncolours colours, $bytes bytes) — page did not draw"
            fi
            [ "$bclean" = 1 ] \
                || why="${why:+$why; }content clipped at the BOTTOM edge — action row off-screen"
            [ "$apx" -ge 150 ] \
                || why="${why:+$why; }action-row band empty — buttons not visible"
            [ "$rclean" = 1 ] \
                || why="${why:+$why; }content clipped at the RIGHT edge"
            W=${size%x*}; H=${size#*x}
            minh=$(grep -m1 "^MEASURE $p $W " "$WORK/render.log" | awk '{print $4}')
            if [ -z "$minh" ]; then
                why="${why:+$why; }page was never measured (measure.py died — see render.log)"
            elif [ "$minh" -gt "$H" ]; then
                why="${why:+$why; }needs ${minh}px height at ${W}px wide — a $size panel squashes or hides part of it"
            fi
            if [ -z "$why" ]; then
                printf 'PASS  %-30s\n' "$name"; pass=$((pass+1))
            else
                printf 'FAIL  %-30s %s\n' "$name" "$why"; fail=$((fail+1))
            fi
        done
    done
    echo "      (screenshots kept in $WORK for eyeballing)"
fi

echo
echo "──────────────────────────────────────────────────────────────────────"
printf '%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-initramfs-budget.sh — both-ways proof for the initramfs gates.
#
#  ── What this exists to catch ───────────────────────────────────────────────
#
#  A Containerfile assertion is the one kind of test that cannot be run by
#  running the test suite: it only exists inside a build, so nobody has ever
#  watched it go red. This repo has paid five days of image builds for that
#  family, and it happened again here — Containerfile.apex asserted
#
#      grep -qE 'kernel/drivers/net/ethernet/'  ->  FATAL
#
#  meaning "the initramfs has no network", against a slim initramfs that
#  CONTAINS eleven files under kernel/drivers/net. It would have FATALed every
#  build on the very artifact it was written to pass.
#
#  So the gates moved into files/scripts/check-initramfs-budget.sh and this
#  suite runs them against REAL captured listings, one fat and one slim, and
#  asserts each gate's verdict BY NAME in both directions. A gate that cannot
#  be shown red is not a gate.
#
#  ── Where the fixtures come from, and what they are not ─────────────────────
#
#  Both are unmodified `lsinitrd` listings of initramfs images built on
#  2026-09-21 inside a REAL bootc-installed APEX deployment (kernel
#  7.2.5-cachyos1.fc43.x86_64), with the exact dracut flags
#  Containerfile.apex:244 uses:
#
#      fat.list.gz    v0 — today's shipped configuration. 359.1 MiB.
#      slim.list.gz   v3 — what this branch ships.         98.5 MiB.
#
#  They are checked in because a GitHub runner has no lsinitrd, no initramfs
#  and no kernel to build one from, and because a fixture that is a MEASUREMENT
#  keeps its value when the next person doubts the number.
#
#  HONESTY ABOUT THE FOUR UNLOCK-CHAIN GATES. The lab disk these listings come
#  from predates files/dracut/apex-unlock-hint/, so neither fixture contains
#  apex-unlock-hint or apex-vconsole-credential, and those four gates are red
#  on BOTH. That is a property of the lab disk, not of this branch. Rather than
#  pretend otherwise, this suite exercises those gates against a SYNTHETIC
#  listing written here — clearly labelled, never presented as a measurement —
#  so they too are proven to fail both ways. The nine gates that this branch
#  actually changes are all measured on unmodified real artifacts.
#
#  Runs anywhere: no root, no kvm, no podman, no lsinitrd.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

GATE=files/scripts/check-initramfs-budget.sh
FIX=tests/fixtures/initramfs

PASS=0
FAILED=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$*"; }

[ -x "$GATE" ] || { echo "FATAL: $GATE is missing or not executable"; exit 1; }
for f in fat slim; do
    [ -s "$FIX/$f.list.gz" ] || { echo "FATAL: fixture $FIX/$f.list.gz is missing"; exit 1; }
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

gunzip -c "$FIX/fat.list.gz"  > "$WORK/fat.list"
gunzip -c "$FIX/slim.list.gz" > "$WORK/slim.list"

# Measured bytes of the two images these listings were taken from. Passed in
# rather than read off the listing so the ESP-budget arithmetic is exercised
# with the real figures.
FAT_INITRD=376595424
SLIM_INITRD=103256987
VMLINUZ=16910408

# Run the gate once per fixture and keep the output; every assertion below
# reads these, so the script under test runs twice, not twenty times.
"$GATE" --list "$WORK/fat.list"  --initrd-bytes "$FAT_INITRD"  --vmlinuz-bytes "$VMLINUZ" > "$WORK/fat.out"  2>&1
FAT_RC=$?
"$GATE" --list "$WORK/slim.list" --initrd-bytes "$SLIM_INITRD" --vmlinuz-bytes "$VMLINUZ" > "$WORK/slim.out" 2>&1
SLIM_RC=$?

# Verdict for one named gate: prints `ok`, `FAIL`, or `absent`. Reads a FILE,
# never a pipe into `grep -q` — the gate script itself shipped that bug for an
# afternoon: grep -q exits on match, the writer takes SIGPIPE, and pipefail
# turns 141 into the pipeline's status, so a match reads as a miss.
verdict() {
    local out="$1" gate="$2" line
    line="$(grep -E "^  (ok|FAIL) +${gate}(:|\$)" "$out" | head -1)"
    case "$line" in
        '  ok'*)   echo ok ;;
        '  FAIL'*) echo FAIL ;;
        *)         echo absent ;;
    esac
}

expect() {   # expect <file> <gate> <ok|FAIL> <why>
    local got; got="$(verdict "$1" "$2")"
    if [ "$got" = "$3" ]; then
        ok "$2 is $3 on $(basename "$1" .out) — $4"
    else
        bad "$2 on $(basename "$1" .out): expected $3, got $got — $4"
    fi
}

echo "── the six gates this branch changes, both ways ─────────────────────────"

# Each of these is satisfied by an ABSENCE, which is exactly the shape that
# rots silently: it passes on an empty file, on a truncated listing, and on a
# typo in its own regex. The fat fixture is the proof that it does not.
expect "$WORK/fat.out"  esp-budget         FAIL "375.2 MiB/deployment needs 1173 MiB of ESP"
expect "$WORK/slim.out" esp-budget         ok   "114.6 MiB/deployment needs 391 MiB, fits 512"
expect "$WORK/fat.out"  no-gpu-firmware    FAIL "nvidia+amdgpu GSP blobs are 203 MiB of the fat image"
expect "$WORK/slim.out" no-gpu-firmware    ok   ""
expect "$WORK/fat.out"  no-kms-driver      FAIL "nvidia/amdgpu/i915/nouveau/radeon/xe all present"
expect "$WORK/slim.out" no-kms-driver      ok   "nothing left to evict simpledrm during probe"
expect "$WORK/fat.out"  no-network-modules FAIL "network, network-manager, kernel-network-modules, nfs, nvmf"
expect "$WORK/slim.out" no-network-modules ok   ""
expect "$WORK/fat.out"  no-networkmanager  FAIL "usr/bin/NetworkManager, 3.7 MB"
expect "$WORK/slim.out" no-networkmanager  ok   ""
expect "$WORK/fat.out"  net-driver-tree    FAIL "322 files / 21.9 MiB of =drivers/net"
expect "$WORK/slim.out" net-driver-tree    ok   "11 files / 1.5 MiB of dependency residue"

echo "── the gates that must NOT have been collateral damage ──────────────────"

# Omitting dracut modules is how you silently lose plymouth or the unlock
# chain, and a lost one looks identical to a working build until someone boots
# it. These assert the slimming did not take them.
expect "$WORK/slim.out" plymouth-engine     ok "the engine survived omitting the DRIVERS, not drm"
expect "$WORK/slim.out" plymouth-theme      ok "apex-os-chartreuse is still in the initrd"
expect "$WORK/slim.out" unlock-cryptsetup   ok "an encrypted root can still be opened"
expect "$WORK/slim.out" unlock-tpm2-token   ok "a TPM keyslot is still usable"
expect "$WORK/slim.out" unlock-keymaps      ok "a non-us keymap is still present"
expect "$WORK/slim.out" unlock-crypt-module ok "rd.luks.* is still honoured"

echo "── the whole-run verdict, both ways ─────────────────────────────────────"
[ "$FAT_RC" -ne 0 ] \
    && ok "the fat listing exits non-zero (rc=$FAT_RC)" \
    || bad "the fat listing exited 0 — the gate cannot fail, which is the whole defect"

# The slim fixture still exits non-zero, and this suite says WHY out loud
# rather than quietly expecting 0: the four unlock-hint gates are red because
# the lab disk predates files/dracut/apex-unlock-hint/. Asserting "slim exits
# 0" here would be a lie that a real image build would not tell.
for g in unlock-hint unlock-hint-wants unlock-vconsole unlock-vconsole-wants; do
    expect "$WORK/slim.out" "$g" FAIL "absent from the LAB DISK, which predates the dracut module"
done
[ "$SLIM_RC" -ne 0 ] \
    && ok "the slim listing exits non-zero too (rc=$SLIM_RC) — those four gates, and nothing else" \
    || bad "the slim listing exited 0 while four of its gates read FAIL — the exit status is lying"

echo "── synthetic: the unlock-chain gates fail both ways too ─────────────────"

# NOT A MEASUREMENT. A hand-written minimal listing, in lsinitrd's format, that
# carries exactly the entries the four gates above look for. It exists so those
# gates are proven to go green when the files are there — which neither real
# fixture can show, because neither real image has them.
mk_synth() {   # mk_synth <outfile> <with-unlock: yes|no>
    local out="$1" with="$2"
    cat > "$out" <<'EOF'
Image: synthetic: 1M
========================================================================
Version: dracut-107-8.fc43

Arguments:  --force --no-hostonly --reproducible --zstd

dracut modules:
base
crypt
plymouth
systemd-cryptsetup
========================================================================
-rwxr-xr-x   1 root     root        86688 Jan  1  1970 usr/bin/systemd-cryptsetup
-rwxr-xr-x   1 root     root        28144 Jan  1  1970 usr/lib64/cryptsetup/libcryptsetup-token-systemd-tpm2.so
-rw-r--r--   1 root     root          847 Jan  1  1970 usr/lib/kbd/keymaps/legacy/i386/qwertz/de.map.gz
-rwxr-xr-x   1 root     root        12345 Jan  1  1970 usr/lib64/plymouth/two-step.so
-rw-r--r--   1 root     root         1234 Jan  1  1970 usr/share/plymouth/themes/apex-os-chartreuse/apex-os-chartreuse.plymouth
EOF
    if [ "$with" = yes ]; then
        cat >> "$out" <<'EOF'
-rwxr-xr-x   1 root     root         4321 Jan  1  1970 usr/bin/apex-unlock-hint
lrwxrwxrwx   1 root     root           40 Jan  1  1970 usr/lib/systemd/system/initrd.target.wants/apex-unlock-hint.service
-rwxr-xr-x   1 root     root         4321 Jan  1  1970 usr/bin/apex-vconsole-credential
lrwxrwxrwx   1 root     root           46 Jan  1  1970 usr/lib/systemd/system/sysinit.target.wants/apex-vconsole-credential.service
EOF
    fi
}

mk_synth "$WORK/synth-with.list" yes
mk_synth "$WORK/synth-without.list" no
"$GATE" --list "$WORK/synth-with.list"    --initrd-bytes 1048576 --vmlinuz-bytes "$VMLINUZ" > "$WORK/synth-with.out"    2>&1
SYNTH_WITH_RC=$?
"$GATE" --list "$WORK/synth-without.list" --initrd-bytes 1048576 --vmlinuz-bytes "$VMLINUZ" > "$WORK/synth-without.out" 2>&1

for g in unlock-hint unlock-hint-wants unlock-vconsole unlock-vconsole-wants; do
    expect "$WORK/synth-with.out"    "$g" ok   "present in the synthetic listing"
    expect "$WORK/synth-without.out" "$g" FAIL "removed from the synthetic listing"
done
[ "$SYNTH_WITH_RC" -eq 0 ] \
    && ok "a listing that satisfies every gate exits 0 — the suite can go green" \
    || { bad "the fully-satisfying synthetic listing exited $SYNTH_WITH_RC, so NOTHING can pass this gate"; sed 's/^/        /' "$WORK/synth-with.out"; }

echo "── the script refuses what it cannot read ───────────────────────────────"

# "Could not check" must never read as "fine". An empty or non-lsinitrd file
# has no module section and no entries, and the gate has to say so rather than
# sail through every absence-shaped assertion with an empty haystack.
: > "$WORK/empty.list"
"$GATE" --list "$WORK/empty.list" --initrd-bytes 1 >"$WORK/empty.out" 2>&1
[ $? -ne 0 ] && grep -q 'FATAL' "$WORK/empty.out" \
    && ok "an empty listing is FATAL, not a clean sheet of passes" \
    || bad "an empty listing did not fail — every absence gate would pass on nothing"

"$GATE" >"$WORK/noargs.out" 2>&1
[ $? -ne 0 ] \
    && ok "no arguments is an error, not a silent success" \
    || bad "the gate exited 0 with no listing at all"

echo
echo "test-apex-initramfs-budget: ${PASS} passed, ${FAILED} failed"
[ "$FAILED" -eq 0 ] || exit 1
exit 0

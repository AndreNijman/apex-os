#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-bootc-install-guard.sh — the efivars guard, proven in BOTH directions.
#
#  What it guards, in one paragraph: on 2026-09-20 `installer/apex-install`
#  ran inside a `--privileged --pid=host` container and reached bootc's
#  bootloader step without `--generic-image`. bootupd deleted the host's
#  Boot0000 and recreated it against the ESP inside the image file being built.
#  The laptop would not boot. BOOT-BREAKAGE-2026-09-20.md.
#
#  ═══ WHICH LAYER IS WHICH ═══
#
#  PREVENTION is `--generic-image` ("Changes to the system firmware will be
#  skipped"), asserted in the argv as `generic-image-present` and mutated below.
#  DETECTION is nvram-guard, exercised below in both directions.
#  The efivars tmpfs is DEFENCE IN DEPTH and INERT against bootc, which
#  nsenters into the host's mount namespace for that step; it is still asserted
#  and still mutated, because keeping it working is cheap, but the suite must
#  not be read as saying it protects anything. The first version of this file
#  passed 52/0 while its central claim was false, because every assertion was
#  about arguments and none was about which argument does the work.
#
#  ═══ THE DEFECT THIS SUITE REFUSES TO BE ═══
#
#  This repository's dominant defect is a gate that runs and inspects nothing,
#  and five Containerfile assertions that could only ever fail once cost five
#  days of image builds. So every claim below is made by something other than
#  the thing being tested:
#
#    * that the mask reaches podman is read out of the STUB PODMAN's recorded
#      argv, not out of the wrapper's own stdout;
#    * that the assertion fires is proven by MUTATING a copy of the wrapper —
#      deleting the one line that adds the mask — and watching the named
#      assertion `efivars-mask-present` go red while the stub podman is never
#      invoked at all. The mutation itself is verified to have changed
#      something, because a sed that matched nothing would "pass" this;
#    * that the NVRAM diff inspects something is proven by moving each of its
#      two sources independently and requiring each one alone to fail the run;
#    * that the repo scan works is proven against PLANTED fixtures, one
#      violating and one compliant, because the repository has zero loopback
#      callers today and a scan finding nothing proves nothing.
#
#  ═══ WHAT THE REPO SCAN CANNOT COVER, SAID OUT LOUD ═══
#
#  The scan matches the literal `--via-loopback`. NEITHER of the two real
#  incidents contained that string: both were `apex-install` handing bootc a
#  loop DEVICE it had attached itself, on a `to-filesystem` install. So the
#  scan covers the shape a human types at a prompt, and covers apex-install's
#  path NOT AT ALL — that one is covered by `installer/test-installer-luks.sh`'s
#  NVRAM section and by the live suite asserting nvram-guard's verdict. Saying
#  so here is the point: a scan believed to cover more than it does is how the
#  second incident happened.
#
#  ═══ NOTHING DANGEROUS RUNS ═══
#
#  No `bootc install` is executed, no container is started, no EFI variable is
#  written and no real NVRAM is read: `podman` and `efibootmgr` are stubs on
#  PATH and the guard's firmware root is pointed at a fixture tree through
#  APEX_NVRAM_EFI_ROOT. The assertion under test is about LAUNCH ARGUMENTS,
#  which can be checked without launching anything.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WRAPPER="$REPO/tests/lab/bootc-install-lab"
GUARD="$REPO/tests/lab/nvram-guard"

PASS=0 FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  ok   %s\n' "$*"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$*"; }
sec() { printf '\n== %s ==\n' "$*"; }
eq()  { [[ "$1" == "$2" ]] && ok "$3 == $1" || bad "$3: want '$1', got '$2'"; }

for f in "$WRAPPER" "$GUARD"; do
    [[ -f "$f" ]] || { echo "FATAL: missing $f" >&2; exit 1; }
    [[ -x "$f" ]] || { echo "FATAL: $f is not executable in the repo" >&2; exit 1; }
done

# The wrapper refuses a target on tmpfs, and /tmp on the machine this was
# written on IS a tmpfs (15 GB of a 29 GB RAM). A work directory under /tmp
# would therefore make every wrapper case fail for the wrong reason, and the
# suite would read as a regression in the mask. /var/tmp is disk-backed here
# and on GitHub's runners; if it ever is not, say so rather than mis-reporting.
WORK="$(mktemp -d /var/tmp/bootc-install-guard.XXXXXX)" || { echo "FATAL: cannot make a work dir" >&2; exit 1; }
trap 'rm -rf "$WORK"' EXIT
wfs="$(stat -f -c %T "$WORK" 2>/dev/null || echo unknown)"
if [[ "$wfs" == tmpfs || "$wfs" == ramfs ]]; then
    echo "FATAL: $WORK is on $wfs; the wrapper refuses tmpfs targets by design." >&2
    echo "       Point TMPDIR at real disk and re-run." >&2
    exit 1
fi

# ── the stubs ───────────────────────────────────────────────────────────────
BIN="$WORK/bin"; REC="$WORK/rec"; mkdir -p "$BIN" "$REC"
export REC

cat > "$BIN/podman" <<'STUB'
#!/bin/sh
# Records the argv it was handed and does nothing else. This file is the
# independent party: if the mask is not here, it never reached podman.
: > "$REC/podman.argv"
for a in "$@"; do printf '%s\n' "$a" >> "$REC/podman.argv"; done
exit 0
STUB

cat > "$BIN/efibootmgr" <<'STUB'
#!/bin/sh
# Records every argv (so the suite can prove the guard never writes NVRAM) and
# prints the Nth canned output, so the two snapshots can be made to differ.
printf '%s\n' "$*" >> "$REC/efibootmgr.argv"
n=$(cat "$REC/ebm.n" 2>/dev/null || echo 0); n=$((n + 1)); printf '%s\n' "$n" > "$REC/ebm.n"
if [ -f "$REC/ebm.out.$n" ]; then cat "$REC/ebm.out.$n"; else cat "$REC/ebm.out.default" 2>/dev/null; fi
STUB
chmod +x "$BIN/podman" "$BIN/efibootmgr"
export PATH="$BIN:$PATH"

# A firmware tree that is not this machine's.
EFI="$WORK/efi"; mkdir -p "$EFI/efivars"
G=8be4df61-93ca-11d2-aa0d-00e098032b8c
printf 'fake-boot0000' > "$EFI/efivars/Boot0000-$G"
printf 'fake-bootorder' > "$EFI/efivars/BootOrder-$G"
export APEX_NVRAM_EFI_ROOT="$EFI"

cat > "$REC/ebm.out.default" <<'EBM'
BootCurrent: 0000
Timeout: 0 seconds
BootOrder: 0000,0004
Boot0000* APEX-OS	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\shimx64.efi
Boot0004* Linux-Firmware-Updater	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\fwupdx64.efi
EBM

reset_recordings() {
    rm -f "$REC/podman.argv" "$REC/efibootmgr.argv" "$REC/ebm.n" \
          "$REC"/ebm.out.[0-9]*
    printf 'fake-boot0000' > "$EFI/efivars/Boot0000-$G"
    printf 'fake-bootorder' > "$EFI/efivars/BootOrder-$G"
}

# Is the mask in a recorded podman argv? Reads the FILE the stub wrote.
mask_in_recording() {
    local f="$1" prev="" line v
    [[ -f "$f" ]] || return 1
    while IFS= read -r line; do
        v=""
        if [[ "$prev" == "--tmpfs" ]]; then v="$line"; fi
        if [[ "$line" == --tmpfs=* ]]; then v="${line#--tmpfs=}"; fi
        v="${v%%:*}"; v="${v%/}"
        [[ "$v" == "/sys/firmware/efi/efivars" ]] && return 0
        prev="$line"
    done < "$f"
    return 1
}

# Is --generic-image in the recorded podman argv, and is it where BOOTC will
# see it? Position is the whole point: podman would swallow the same string
# appearing before the image name, and bootc would never get it. So this reads
# the FILE the stub wrote and only counts hits after the `bootc install` pair.
generic_image_in_recording() {
    local f="$1" seen_bootc=0 prev="" line
    [[ -f "$f" ]] || return 1
    while IFS= read -r line; do
        if [[ "$seen_bootc" == 0 ]]; then
            [[ "$prev" == "bootc" && "$line" == "install" ]] && seen_bootc=1
            prev="$line"; continue
        fi
        [[ "$line" == "--generic-image" ]] && return 0
    done < "$f"
    return 1
}

# The same string BEFORE `bootc install` must NOT count. Used to prove the
# helper above discriminates, rather than matching anywhere in the file.
generic_image_anywhere() { grep -qxF -- '--generic-image' "$1" 2>/dev/null; }

IMG="$WORK/target.img"

# ═════════════════════════════════════════════════════════════════════════════
sec "direction 1 — the wrapper puts the mask in the argv podman actually gets"
# ═════════════════════════════════════════════════════════════════════════════
reset_recordings
out="$("$WRAPPER" --size 1M --out "$WORK/nv1" localhost/fake:lab "$IMG" 2>&1)"; rc=$?
eq 0 "$rc" "the wrapper exits 0 on a normal lab run"
if mask_in_recording "$REC/podman.argv"; then
    ok "podman itself was handed --tmpfs /sys/firmware/efi/efivars"
else
    bad "the mask is NOT in the argv podman received: $(tr '\n' ' ' < "$REC/podman.argv" 2>/dev/null)"
fi
grep -q -- '--via-loopback' "$REC/podman.argv" \
    && ok "the run really is a loopback install" \
    || bad "no --via-loopback in the recorded argv — this suite is testing the wrong command"
# THE PREVENTION. Everything above this line is about a mask that bootc walks
# straight past; this is the assertion that corresponds to the layer that works.
if generic_image_in_recording "$REC/podman.argv"; then
    ok "bootc itself was handed --generic-image — the firmware step is skipped"
else
    bad "no --generic-image after 'bootc install' in the recorded argv: $(tr '\n' ' ' < "$REC/podman.argv" 2>/dev/null)"
fi
grep -q 'verdict: verified' <<<"$out" \
    && ok "nvram-guard reported verified around the run" \
    || bad "nvram-guard did not report verified: $out"

# ═════════════════════════════════════════════════════════════════════════════
sec "a caller cannot satisfy generic-image-present from the podman side"
# ═════════════════════════════════════════════════════════════════════════════
# `--podman-arg --generic-image` puts the literal string in the argv, but before
# the image name — podman eats it and bootc never sees it. A check that merely
# grepped the argv would call that compliant. This case exists so that the
# discrimination is proven rather than asserted in a comment. podman's stub
# accepts anything, so the wrapper still runs; what must hold is that the
# helper distinguishes the two positions.
reset_recordings
pout="$("$WRAPPER" --size 1M --podman-arg --generic-image localhost/fake:lab "$IMG" 2>&1)"; prc=$?
eq 0 "$prc" "a --podman-arg --generic-image run still launches"
generic_image_anywhere "$REC/podman.argv" \
    && ok "the string appears in the argv twice over" \
    || bad "the fixture did not land: $pout"
gcount="$(grep -cxF -- '--generic-image' "$REC/podman.argv")"
eq 2 "$gcount" "both copies are present (one podman-side, one bootc-side)"
# Now remove the wrapper's own copy from the recording and re-check: what is
# left is ONLY the podman-side string, and the helper must reject it.
grep -nxF -- '--generic-image' "$REC/podman.argv" | tail -1 | cut -d: -f1 \
    | xargs -I{} sed -i '{}d' "$REC/podman.argv"
generic_image_in_recording "$REC/podman.argv" \
    && bad "a podman-side --generic-image satisfied the bootc-side check" \
    || ok "a podman-side --generic-image does NOT satisfy the bootc-side check"

# ═════════════════════════════════════════════════════════════════════════════
sec "direction 2 — delete the mask and the named assertion goes red"
# ═════════════════════════════════════════════════════════════════════════════
# A copy, because the point is to watch the guard refuse, not to ship a broken
# wrapper. The mutation is verified to have REMOVED something: a sed that
# matched nothing would leave the wrapper intact and this case would "pass"
# for the same reason the five dead Containerfile assertions did.
MUT="$WORK/mutant"; mkdir -p "$MUT"
cp "$WRAPPER" "$MUT/bootc-install-lab"; cp "$GUARD" "$MUT/nvram-guard"
chmod +x "$MUT/bootc-install-lab" "$MUT/nvram-guard"
before_n=$(grep -c '# efivars-mask$' "$WRAPPER")
sed -i '/# efivars-mask$/d' "$MUT/bootc-install-lab"
after_n=$(grep -c '# efivars-mask$' "$MUT/bootc-install-lab")
eq 1 "$before_n" "the shipped wrapper has exactly one mask-injection line"
eq 0 "$after_n" "the mutant has none — the mutation really removed it"

reset_recordings
mout="$("$MUT/bootc-install-lab" --size 1M localhost/fake:lab "$WORK/mutant.img" 2>&1)"; mrc=$?
eq 6 "$mrc" "the mutant refuses to launch"
grep -q 'efivars-mask-present' <<<"$mout" \
    && ok "the refusal names the assertion: efivars-mask-present" \
    || bad "the mutant failed without naming efivars-mask-present: $mout"
[[ -f "$REC/podman.argv" ]] \
    && bad "the mutant still invoked podman — the check runs too late to matter" \
    || ok "podman was never invoked: the refusal happens before launch"
[[ -e "$WORK/mutant.img" ]] \
    && bad "the mutant created the target image before refusing" \
    || ok "nothing was created on disk by the refused run"

# ═════════════════════════════════════════════════════════════════════════════
sec "direction 2b — delete --generic-image and the launch is REFUSED"
# ═════════════════════════════════════════════════════════════════════════════
# THE DISCRIMINATING CASE. A loopback install without --generic-image is the
# thing that made this laptop unbootable twice, and the wrapper must refuse it
# BEFORE launching, not detect it afterwards. Proven the same way as the mask:
# delete the one tagged line from a COPY and watch `generic-image-present` go
# red while podman is never invoked. The damage is never reproduced — no bootc
# runs, no container starts, no EFI variable is read or written. The claim is
# about the argv the wrapper builds and about its refusal, and both are
# observable without firmware.
MUT2="$WORK/mutant2"; mkdir -p "$MUT2"
cp "$WRAPPER" "$MUT2/bootc-install-lab"; cp "$GUARD" "$MUT2/nvram-guard"
chmod +x "$MUT2/bootc-install-lab" "$MUT2/nvram-guard"
gbefore=$(grep -c '# generic-image$' "$WRAPPER")
sed -i '/# generic-image$/d' "$MUT2/bootc-install-lab"
gafter=$(grep -c '# generic-image$' "$MUT2/bootc-install-lab")
eq 1 "$gbefore" "the shipped wrapper has exactly one --generic-image line"
eq 0 "$gafter" "the mutant has none — the mutation really removed it"
# and it removed the RIGHT thing: the mutant must still carry the mask, or this
# case would be re-proving direction 2 under a different name.
grep -q -- '--tmpfs /sys/firmware/efi/efivars' "$MUT2/bootc-install-lab" \
    && ok "the mutant still carries the efivars mask — the two layers are independent" \
    || bad "the mutation removed the mask too; this case is not discriminating"

reset_recordings
g2out="$("$MUT2/bootc-install-lab" --size 1M localhost/fake:lab "$WORK/mutant2.img" 2>&1)"; g2rc=$?
eq 7 "$g2rc" "the mutant refuses to launch"
grep -q 'generic-image-present' <<<"$g2out" \
    && ok "the refusal names the assertion: generic-image-present" \
    || bad "the mutant failed without naming generic-image-present: $g2out"
grep -q 'efivars-mask-present' <<<"$g2out" \
    && bad "it blamed the mask, which is not what is missing" \
    || ok "it does not blame the mask — the message names the layer that failed"
[[ -f "$REC/podman.argv" ]] \
    && bad "the mutant still invoked podman — the check runs too late to matter" \
    || ok "podman was never invoked: the refusal happens before launch"
[[ -e "$WORK/mutant2.img" ]] \
    && bad "the mutant created the target image before refusing" \
    || ok "nothing was created on disk by the refused run"
grep -q 'nsenter\|host mount namespace' <<<"$g2out" \
    && ok "the refusal says why the mask would not have covered this" \
    || bad "the refusal does not explain the namespace hop: $g2out"

# A wrapper whose argv lost `bootc install` entirely must also fail, rather
# than scanning an empty range and reporting ok. That is this repository's
# signature defect and the assertion is written to fail loudly on it.
MUT3="$WORK/mutant3"; mkdir -p "$MUT3"
cp "$WRAPPER" "$MUT3/bootc-install-lab"; cp "$GUARD" "$MUT3/nvram-guard"
chmod +x "$MUT3/bootc-install-lab" "$MUT3/nvram-guard"
# Anchored to the array append, not to the bare phrase: the phrase also appears
# in the header prose, and a sed that rewrote a comment would "break" nothing
# while this case still reported a pass.
nb=$(grep -cF 'PODMAN_ARGS+=("$IMAGE" bootc install to-disk' "$MUT3/bootc-install-lab")
sed -i 's/PODMAN_ARGS+=("$IMAGE" bootc install to-disk/PODMAN_ARGS+=("$IMAGE" NOTBOOTC notinstall to-disk/' "$MUT3/bootc-install-lab"
na=$(grep -cF 'PODMAN_ARGS+=("$IMAGE" bootc install to-disk' "$MUT3/bootc-install-lab")
eq 1 "$nb" "there is exactly one bootc-install ARRAY APPEND to break"
eq 0 "$na" "the mutant has none — the mutation really removed it"
reset_recordings
n3out="$("$MUT3/bootc-install-lab" --size 1M localhost/fake:lab "$WORK/mutant3.img" 2>&1)"; n3rc=$?
eq 7 "$n3rc" "an argv with no 'bootc install' fails the assertion"
grep -q 'inspected nothing' <<<"$n3out" \
    && ok "and it says it would have inspected nothing" \
    || bad "a vacuous scan passed quietly: $n3out"

# ═════════════════════════════════════════════════════════════════════════════
sec "a caller cannot unmask it either"
# ═════════════════════════════════════════════════════════════════════════════
# A tmpfs over the leaf is undone by a bind of any ancestor, so all four are
# refused. `-v /sys:/sys` is the realistic mistake: it looks harmless.
for dst in /sys /sys/firmware /sys/firmware/efi /sys/firmware/efi/efivars; do
    reset_recordings
    uout="$("$WRAPPER" --size 1M --podman-arg -v --podman-arg "$dst:$dst" \
            localhost/fake:lab "$WORK/u.img" 2>&1)"; urc=$?
    eq 5 "$urc" "refused: -v $dst:$dst"
    grep -q 'caller-unmasks-efivars' <<<"$uout" \
        && ok "the refusal for $dst names caller-unmasks-efivars" \
        || bad "the refusal for $dst is unnamed: $uout"
    [[ -f "$REC/podman.argv" ]] && bad "podman ran despite -v $dst:$dst" || ok "podman never ran for $dst"
done
reset_recordings
uout="$("$WRAPPER" --size 1M --podman-arg "--mount type=bind,source=/sys,target=/sys/firmware/efi" \
        localhost/fake:lab "$WORK/u.img" 2>&1)"; urc=$?
eq 5 "$urc" "refused: --mount target=/sys/firmware/efi"
# The one that must NOT be refused, or the guard is unusable and gets bypassed.
reset_recordings
gout="$("$WRAPPER" --size 1M --podman-arg -v --podman-arg /sys/fs/cgroup:/sys/fs/cgroup \
        localhost/fake:lab "$IMG" 2>&1)"; grc=$?
eq 0 "$grc" "a mount that is NOT a parent of efivars is allowed through"
mask_in_recording "$REC/podman.argv" \
    && ok "and that run still carries the mask" \
    || bad "an allowed extra mount lost the mask: $gout"

# ═════════════════════════════════════════════════════════════════════════════
sec "the lab wrapper is not a path to a real disk"
# ═════════════════════════════════════════════════════════════════════════════
reset_recordings
dout="$("$WRAPPER" --dry-run localhost/fake:lab /dev/nvme0n1 2>&1)"; drc=$?
eq 5 "$drc" "a /dev target is refused"
grep -q 'target-is-a-device' <<<"$dout" && ok "named target-is-a-device" || bad "unnamed: $dout"
tout="$("$WRAPPER" --dry-run localhost/fake:lab /tmp/x.img 2>&1)"; trc=$?
eq 5 "$trc" "a /tmp target is refused"
grep -q 'target-is-in-ram' <<<"$tout" && ok "named target-is-in-ram" || bad "unnamed: $tout"
[[ -f "$REC/podman.argv" ]] && bad "podman ran on a refused target" || ok "podman never ran on a refused target"

# ═════════════════════════════════════════════════════════════════════════════
sec "nvram-guard — both of its sources, each proven to be load-bearing"
# ═════════════════════════════════════════════════════════════════════════════
# verified
reset_recordings
vout="$("$GUARD" --label t -- true 2>&1)"; vrc=$?
eq 0 "$vrc" "unchanged NVRAM → exit 0"
grep -q 'verdict: verified' <<<"$vout" && ok "verdict verified" || bad "no verified verdict: $vout"

# source 1: efivarfs digests move, the rendering does not
reset_recordings
cout="$("$GUARD" --label t -- sh -c "printf changed > '$EFI/efivars/Boot0000-$G'" 2>&1)"; crc=$?
eq 3 "$crc" "a changed efivarfs variable → exit 3"
grep -q 'nvram-changed' <<<"$cout" && ok "verdict nvram-changed from the efivarfs digests alone" \
                                   || bad "the efivarfs source missed a change: $cout"

# source 2: the rendering moves, the efivarfs digests do not
reset_recordings
cp "$REC/ebm.out.default" "$REC/ebm.out.1"
sed 's/BootOrder: 0000,0004/BootOrder: 0004,0000/' "$REC/ebm.out.default" > "$REC/ebm.out.2"
c2="$("$GUARD" --label t -- true 2>&1)"; c2rc=$?
eq 3 "$c2rc" "a changed efibootmgr rendering → exit 3"
grep -q 'nvram-changed' <<<"$c2" && ok "verdict nvram-changed from efibootmgr -v alone" \
                                 || bad "the efibootmgr source missed a change: $c2"
grep -q 'BootOrder: 0004,0000' <<<"$c2" && ok "the diff shows the entry that moved" \
                                        || bad "the failure printed no diff: $c2"

# could-not-snapshot: the command must NOT run
reset_recordings
: > "$REC/ebm.out.default"
sout="$("$GUARD" --label t -- sh -c "touch '$WORK/ran-anyway'" 2>&1)"; src=$?
eq 4 "$src" "an unreadable NVRAM → exit 4"
grep -q 'could-not-snapshot' <<<"$sout" && ok "verdict could-not-snapshot" || bad "wrong verdict: $sout"
[[ -e "$WORK/ran-anyway" ]] \
    && bad "the command ran even though NVRAM could not be measured" \
    || ok "the command did not run: an unmeasurable run is refused, not assumed safe"
grep -q 'verdict: verified' <<<"$sout" && bad "it claimed verified with an empty snapshot" \
                                       || ok "it never says verified about something it did not read"
cat > "$REC/ebm.out.default" <<'EBM'
BootCurrent: 0000
BootOrder: 0000,0004
Boot0000* APEX-OS	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\shimx64.efi
EBM

# could-not-run: no firmware at all — the command runs, and nothing is claimed
reset_recordings
nout="$(APEX_NVRAM_EFI_ROOT="$WORK/no-such-firmware" "$GUARD" --label t -- sh -c "touch '$WORK/ran-no-efi'; exit 7" 2>&1)"; nrc=$?
eq 7 "$nrc" "with no UEFI the command's own exit status is propagated"
[[ -e "$WORK/ran-no-efi" ]] && ok "the command ran" || bad "the command did not run on a non-UEFI host"
grep -q 'could-not-run' <<<"$nout" && ok "verdict could-not-run" || bad "wrong verdict: $nout"
grep -q 'verdict: verified' <<<"$nout" && bad "it claimed verified on a host with no NVRAM" \
                                       || ok "no 'verified' claim where nothing was measured"

# read-only, proven from the recorded argv
reset_recordings
"$GUARD" --label t -- true >/dev/null 2>&1
if [[ -s "$REC/efibootmgr.argv" ]]; then
    badargs="$(grep -vx -- '-v' "$REC/efibootmgr.argv" || true)"
    [[ -z "$badargs" ]] \
        && ok "every efibootmgr invocation was exactly '-v' ($(grep -c . "$REC/efibootmgr.argv") calls)" \
        || bad "the guard passed efibootmgr something other than -v: $badargs"
else
    bad "efibootmgr was never invoked — the guard measured nothing"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "no scripted caller in this repository bypasses the wrapper"
# ═════════════════════════════════════════════════════════════════════════════
# The repository has ZERO loopback callers today, so this scan passing over the
# repo proves nothing on its own. Both fixtures below exist so that the scan is
# shown to work in both directions before its verdict on the repo is believed.
#
# WHAT IT LOOKS FOR CHANGED, AND THE OLD ANSWER WAS BACKWARDS. It used to flag
# a `--via-loopback` command with no `/sys/firmware/efi/efivars` in it. That is
# the inert property: the mask does not stop bootc, which nsenters into the
# host mount namespace for the bootloader step. So the old scan would have
# passed a command that writes this machine'"'"'s NVRAM (loopback, masked, no
# --generic-image) and flagged one that cannot (loopback, unmasked, with
# --generic-image). It now flags a loopback install with no `--generic-image`,
# which is the property that decides whether the firmware step runs.
#
# Continuations are joined first: in a real caller the `--via-loopback` and the
# `--tmpfs` are on different physical lines of one command.
scan_list() {   # file paths on stdin -> "path:line" per violation
    local f
    while IFS= read -r f; do
        [[ -f "$f" ]] || continue
        # Binary files are skipped, and not quietly: awk on this repo's plymouth
        # GIF and wallpaper JPEG aborts with a glibc malloc assertion, and an
        # aborted scanner still let the verdict below print "no tracked file
        # bypasses the wrapper" — a scan that crashed on part of the tree
        # reporting a clean result is this repository's signature defect.
        grep -Iq '' "$f" 2>/dev/null || continue
        awk -v path="$f" '
            { acc = acc $0; }
            /\\[[:space:]]*$/ { sub(/\\[[:space:]]*$/, " ", acc); next }
            {
                s = acc; sub(/^[[:space:]]*/, "", s)
                # A COMMENT MUST NOT SATISFY THIS CHECK. `podman run …
                # --via-loopback …  # TODO: --tmpfs /sys/firmware/efi/efivars`
                # is the shape that would otherwise walk straight through, and
                # it is the same species as the forbid-check this repository
                # once shipped that matched the comment explaining what it
                # forbade. Trailing comments are cut before BOTH tests, so a
                # `#` inside a quoted string can only produce a FALSE POSITIVE,
                # which fails loudly and is the right direction to be wrong in.
                code = acc; sub(/[[:space:]]#.*$/, "", code)
                if (substr(s, 1, 1) != "#" && index(code, "via-loopback") > 0 \
                    && index(code, "--generic-image") == 0)
                    printf "%s:%d\n", path, NR
                acc = ""
            }
        ' "$f"
    done
}

FIX="$WORK/fixtures"; mkdir -p "$FIX"
cat > "$FIX/violating.sh" <<'FIXTURE'
#!/bin/sh
# A loopback install that would run bootc's firmware step against this machine.
# NOT a transcript of either real incident: both of those were apex-install
# handing bootc a loop DEVICE, with no `--via-loopback` anywhere in them. This
# is the shape a person types at a prompt, which is the shape the scan covers.
# Note it DOES carry the efivars tmpfs, and is dangerous anyway — that is the
# case the previous version of this scan called compliant.
sudo podman run --rm --privileged --pid=host \
    -v /var/lib/containers:/var/lib/containers \
    -v /dev:/dev \
    --tmpfs /sys/firmware/efi/efivars \
    -v /var/lab-scratch:/work \
    localhost/apex-os:daily \
    bootc install to-disk --via-loopback --wipe \
        --filesystem ext4 /work/lab.img
FIXTURE
cat > "$FIX/compliant.sh" <<'FIXTURE'
#!/bin/sh
sudo podman run --rm --privileged --pid=host \
    -v /var/lib/containers:/var/lib/containers \
    -v /dev:/dev \
    --tmpfs /sys/firmware/efi/efivars \
    -v /var/lab-scratch:/work \
    localhost/apex-os:daily \
    bootc install to-disk --via-loopback --generic-image --wipe \
        --filesystem ext4 /work/lab.img
FIXTURE

cat > "$FIX/commented.sh" <<'FIXTURE'
#!/bin/sh
sudo podman run --rm --privileged --pid=host \
    -v /dev:/dev \
    -v /var/lab-scratch:/work \
    localhost/apex-os:daily \
    bootc install to-disk --via-loopback --wipe \
        --filesystem ext4 /work/lab.img   # TODO add --generic-image
FIXTURE

vhits="$(printf '%s\n' "$FIX/violating.sh" | scan_list)"
[[ -n "$vhits" ]] && ok "the scan catches a loopback caller with no --generic-image ($vhits)" \
                  || bad "the scan MISSED a firmware-writing command shape — it inspects nothing"
chits="$(printf '%s\n' "$FIX/compliant.sh" | scan_list)"
[[ -z "$chits" ]] && ok "the scan passes a caller that skips the firmware step" \
                  || bad "the scan flags a correct caller: $chits"
khits="$(printf '%s\n' "$FIX/commented.sh" | scan_list)"
[[ -n "$khits" ]] && ok "a flag that exists only in a trailing comment does not satisfy the scan" \
                  || bad "the scan accepted a COMMENTED --generic-image — a comment is not a guard"
# The old predicate, run against the same three fixtures, to show the change is
# not cosmetic: it called the dangerous fixture compliant. If this ever stops
# holding, the two predicates have converged and one of them is wrong.
old_predicate_hits() {
    awk '{ acc = acc $0 }
         /\\[[:space:]]*$/ { sub(/\\[[:space:]]*$/, " ", acc); next }
         { s = acc; sub(/^[[:space:]]*/, "", s)
           code = acc; sub(/[[:space:]]#.*$/, "", code)
           if (substr(s, 1, 1) != "#" && index(code, "via-loopback") > 0 \
               && index(code, "/sys/firmware/efi/efivars") == 0) print NR
           acc = "" }' "$1"
}
[[ -z "$(old_predicate_hits "$FIX/violating.sh")" ]] \
    && ok "the OLD predicate passed this dangerous command — the inversion was load-bearing" \
    || bad "the old predicate also caught it; this scan change proves nothing"

# Now the repository itself. Two files are exempt and both are named here:
# the wrapper builds the argv across several array appends rather than one
# continued command, and this suite embeds the violating fixture above.
cd "$REPO" || exit 1
# An EMPTY file list would make the verdict below print "ok" while inspecting
# nothing — and `git ls-files` really can come back empty here, because CI
# checkout falls back to a tarball with no .git. So the list is captured, its
# size is asserted against a floor, and a sentinel that is KNOWN to contain a
# `bootc install` line has to be inside it before the clean verdict is
# believed. Same guard as test-boot-v2.sh's "the scan is vacuous".
mapfile -t tracked < <(git ls-files \
        | grep -vE '^(docs/|ROADMAP/)' \
        | grep -vE '\.md$' \
        | grep -vxF 'tests/lab/bootc-install-lab' \
        | grep -vxF 'tests/test-bootc-install-guard.sh')
printf '%s\n' ${tracked[@]+"${tracked[@]}"} > "$WORK/tracked.txt"
(( ${#tracked[@]} > 300 )) \
    && ok "the scan covers ${#tracked[@]} tracked files" \
    || bad "the scan is vacuous: git ls-files gave ${#tracked[@]} files (a tarball checkout has no .git)"
# Not piped into `grep -q`: a match makes grep exit early, printf takes SIGPIPE,
# and under `pipefail` the pipeline returns 141 on SUCCESS.
grep -qxF 'installer/apex-install' "$WORK/tracked.txt" \
    && ok "the sentinel installer/apex-install is inside the scanned set" \
    || bad "installer/apex-install is not in the scanned set — the file list is wrong"
hits="$(scan_list < "$WORK/tracked.txt")"
[[ -z "$hits" ]] \
    && ok "no tracked file runs a --via-loopback install without --generic-image" \
    || bad "these run a loopback install that would write this machine's NVRAM:"$'\n'"$hits"

printf '\n== test-bootc-install-guard: %d passed, %d failed ==\n' "$PASS" "$FAIL"
(( FAIL == 0 ))

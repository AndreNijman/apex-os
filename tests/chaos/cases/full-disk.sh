# ─────────────────────────────────────────────────────────────────────────────
#  full-disk — the machine records a result onto a filesystem with no space
#  left on it.
#
#  P1-062 criterion 1's "full disk". The subject is `apex qualify record`,
#  chosen because it is the shape of write this criterion is really about: a
#  small persistent document that is REPLACED rather than appended to, holding
#  state a person cannot reconstruct — which check on their machine was tried
#  and what happened. `qualify::save` writes a sibling temp file and renames,
#  and its own comment says why: "a file truncated by a crash mid-write is a
#  file somebody has to reconstruct." A full disk is that crash, arriving on
#  schedule and in daylight, so it is the cheapest honest way to find out
#  whether the atomic write is really atomic.
#
#  INJECTION   a size-limited tmpfs mounted inside `unshare --user
#              --map-root-user --mount`, filled to the last byte with a ballast
#              file. This is a REAL ENOSPC from the kernel — not a stubbed
#              write, not a permission denial dressed up as one — and it needs
#              no root and no loop device.
#  PROOF       the HARNESS, not the subject, tries to write one byte into the
#              filesystem after filling it, and must be refused. `dd` exiting 0
#              having written the ballast proves the ballast was written, not
#              that the next writer will fail; a refused `printf x` proves it.
#              The mount is also checked to BE a tmpfs at the expected path, so
#              a case whose mount silently failed and filled the developer's
#              real disk instead cannot reach the subject.
#  EXPOSURE    the baseline is the same subject on the same kind of tmpfs with
#              room on it, and it must have recorded the result. That proves
#              `apex qualify record` really writes into the filesystem this
#              case fills, rather than somewhere the fault does not reach.
#  SURVIVAL    three things, and the third is the one that was wrong.
#              1. The verb must FAIL and name ENOSPC. A record that silently
#                 did not happen is worse than an error.
#              2. The existing document must be byte-identical. This is the
#                 atomic-write claim, and it is the reason the case exists.
#              3. It must not leave litter behind.
#
#  ═══ WHAT THIS CASE FOUND ═══
#
#  (1) and (2) held: the verb exits 1 with "No space left on device (os error
#  28)" naming the path, and the existing qualification.json is byte-for-byte
#  what it was. The atomic write is real.
#
#  (3) did not. `qualify::save` removes its temp file when the RENAME fails and
#  not when the WRITE fails, so every refused record left a
#  `qualification.json.tmp` next to the document, permanently — on the
#  filesystem that has no space, which is the one place litter costs something.
#  And it was mode 0644, because the `set_permissions(0o600)` that the function
#  performs sits BETWEEN the write and the rename and the failure returns
#  before it. The comment immediately above that call says what the mode is
#  for: "between a 0644 create and a later chmod there is a window in which
#  another user on the machine can read it, and this file describes the owner's
#  hardware." On this path the window is not a window. It is the end state.
#
#  On tmpfs the leftover is empty, because tmpfs accounts in whole pages: the
#  page is there and the document fits, or the page is not and nothing is
#  written. On a filesystem that allocates sub-page — and on a database with
#  more than one machine in it, where the document is larger than one page —
#  the prefix that got written is what is left at 0644.
#
#  Fixed in `qualify::save`: the temp file is created 0600 by `OpenOptions
#  ::mode` rather than chmodded afterwards, so there is no instant at which it
#  exists at any other mode, and a failed write removes it before returning.
#  `channel::record_update` had the same omission and got the same removal.
#  Nine other writers in this tree share the temp-then-rename shape and were
#  NOT touched — a survey is not a fix, and changing code this case does not
#  exercise would be nine assertions nobody made:
#      apex/src/{host,task,blueprint}.rs
#      apex-agent-core/src/{config,checkpoint,project,request}.rs
#  A case that drives one of those is the way to close them.
#
#  ═══ WHAT THE MUTANT SHOWED ═══
#
#  Replacing the temp-and-rename with a plain `fs::write(path, &text)` — the
#  non-atomic form — turns four of these red, and it is worth knowing exactly
#  how, because it is not the obvious way. Under that mutant the write
#  SUCCEEDS on the full filesystem: truncating the existing document frees the
#  page the new one needs. So `rc` goes to 0, the ENOSPC message never appears,
#  and the document changes. The record is not corrupted — it is REPLACED,
#  under a write that had no business succeeding. The assertion below is
#  therefore worded without assuming which way it went wrong, and carries the
#  subject's exit code, because that is what tells the two apart.
# ─────────────────────────────────────────────────────────────────────────────

CASE_TITLE="a result recorded onto a filesystem with no space left"
CASE_CRITERION="1 (full disk), 3 (diagnostics, no silent corruption)"
CASE_NEEDS="apex-binary userns tmpfs-ns"

# The check the baseline records and the check the fault refuses. Two different
# rows, so "the record changed" and "the record did not change" are about
# different facts and neither can be satisfied by the other's write.
BASELINE_CHECK=sleep
FAULT_CHECK=audio

# ── the one inner script both arms run ──────────────────────────────────────
#
# Written once and given a size and a ballast flag, because two nearly-identical
# copies of a mount-and-fill script is how the control and the fault quietly
# stop being the same experiment.
#
# Everything it learns is copied OUT of the namespace into $CASE_DIR before it
# exits: the mount vanishes with the namespace, so a snapshot taken afterwards
# by the driver would see an empty directory and report, with total confidence,
# that nothing had changed.
_run_on_tmpfs() {
    local size="$1" fill="$2" check="$3" tag="$4"
    unshare --user --map-root-user --mount -- bash -s -- \
        "$APEX_BIN" "$CASE_ROOT" "$CASE_DIR" "$size" "$fill" "$check" "$tag" <<'INNER'
set -uo pipefail
bin="$1"; root="$2"; out="$3"; size="$4"; fill="$5"; check="$6"; tag="$7"
mnt="$root/mnt"

mount -t tmpfs -o "size=$size" tmpfs "$mnt" || { echo "the tmpfs would not mount" >&2; exit 3; }
# Proof the mount took, made before anything is written: without it a failed
# mount would send every write below into the developer's real filesystem and
# fill THAT. `stat -f -c %T` names the filesystem type the path is actually on.
[ "$(stat -f -c %T "$mnt")" = "tmpfs" ] || { echo "$mnt is not a tmpfs" >&2; exit 3; }

export XDG_STATE_HOME="$mnt/state" HOME="$mnt" \
       XDG_CONFIG_HOME="$mnt/config" XDG_CACHE_HOME="$mnt/cache" \
       APEX_QUALIFY_ROOT="$root/machine"
store="$mnt/state/apex/qualification.json"
mkdir -p "$mnt/state/apex" "$mnt/config" "$mnt/cache"

# A machine that has already recorded something, because the property under
# test is that an EXISTING document survives. An empty store would make "the
# document is unchanged" vacuously true.
"$bin" qualify consent grant                 > "$out/$tag.setup" 2>&1
"$bin" qualify record "$BASELINE_CHECK_SEED" --pass >> "$out/$tag.setup" 2>&1
sha256sum < "$store" | cut -d' ' -f1 > "$out/$tag.store.sha.before"
wc -c < "$store" > "$out/$tag.store.bytes.before"

if [ "$fill" = fill ]; then
    # To the last byte. dd stops at ENOSPC and says so on stderr; its exit code
    # is not the proof and is not read.
    dd if=/dev/zero of="$mnt/ballast" bs=4k >/dev/null 2>&1
    # The independent observation: can anything still be written here?
    if printf x > "$mnt/harness-probe" 2>/dev/null; then
        echo "the filesystem still accepts a write, so it is not full" > "$out/$tag.enospc"
        rm -f "$mnt/harness-probe"
    else
        echo "a one-byte write by the harness is refused" > "$out/$tag.enospc"
    fi
else
    echo "not filled — this is the control" > "$out/$tag.enospc"
fi
df -k "$mnt" | tail -1 > "$out/$tag.df"

"$bin" qualify record "$check" --pass > "$out/$tag.subject.out" 2> "$out/$tag.subject.err"
echo $? > "$out/$tag.subject.rc"

sha256sum < "$store" 2>/dev/null | cut -d' ' -f1 > "$out/$tag.store.sha.after"
# The store directory exactly as it stands, modes included, which is where the
# leftover and its permissions show up.
ls -l "$mnt/state/apex/" > "$out/$tag.storedir" 2>&1
# And the document itself, out of the namespace, so a human reading the bundle
# can see what survived rather than taking the sha's word for it.
cp "$store" "$out/$tag.store.json" 2>/dev/null || true
exit 0
INNER
}

case_setup() {
    mkdir -p "$CASE_ROOT/mnt" "$CASE_ROOT/machine"
    command -v dd >/dev/null 2>&1 || { echo "no dd, so the filesystem cannot be filled" >&2; return 1; }
    # Exported into the inner script's environment by name, because the inner
    # heredoc is quoted — it must not be expanded out here, or a change to the
    # variable would silently stop reaching it.
    export BASELINE_CHECK_SEED="$BASELINE_CHECK"
    echo "mountpoint $CASE_ROOT/mnt, fixture machine $CASE_ROOT/machine"
}

# ── the control, which is also the exposure proof ───────────────────────────
case_baseline() {
    _run_on_tmpfs 1m nofill "$FAULT_CHECK" baseline
    printf 'control (tmpfs with room): rc=%s\n' "$(cat "$CASE_DIR/baseline.subject.rc" 2>/dev/null)"
    cat "$CASE_DIR/baseline.subject.out" 2>/dev/null
}

case_inject() {
    # Nothing in the tree changes out here. The fault is a filesystem that
    # exists only inside the namespace case_observe creates, so it is made and
    # destroyed around the subject. Said out loud so a reader does not look for
    # a mutation that is not there.
    echo "the fault is a filled tmpfs, mounted around the subject in case_observe"
}

case_prove() {
    # The fault is proven in the same namespace that carries it, so the proof
    # is written by case_observe's inner script into observe.enospc — and this
    # function reads that file. Running a second, separate namespace here would
    # prove a DIFFERENT filesystem was full.
    #
    # Which means case_observe has to have run first. It has not, so the fault
    # is manufactured and proven here once, in its own namespace, purely as the
    # capability check: can this kernel, as this user, produce a real ENOSPC?
    local out
    out="$(unshare --user --map-root-user --mount -- bash -s -- "$CASE_ROOT" <<'INNER' 2>&1
set -uo pipefail
root="$1"
mount -t tmpfs -o size=64k tmpfs "$root/mnt" || { echo "the tmpfs would not mount"; exit 1; }
[ "$(stat -f -c %T "$root/mnt")" = tmpfs ] || { echo "the mountpoint is not a tmpfs"; exit 1; }
dd if=/dev/zero of="$root/mnt/ballast" bs=4k >/dev/null 2>&1
if printf x > "$root/mnt/probe" 2>/dev/null; then
    echo "a write still succeeds on a filesystem reported as full"
    exit 1
fi
echo "ENOSPC: a one-byte write into the filled tmpfs is refused by the kernel"
INNER
)" || { echo "$out"; return 1; }
    echo "$out"
    return 0
}

case_observe() {
    _run_on_tmpfs 64k fill "$FAULT_CHECK" observe
    printf 'under a full filesystem: rc=%s\n' "$(cat "$CASE_DIR/observe.subject.rc" 2>/dev/null)"
    printf 'the harness probe said : %s\n' "$(cat "$CASE_DIR/observe.enospc" 2>/dev/null)"
    printf 'df                     : %s\n' "$(cat "$CASE_DIR/observe.df" 2>/dev/null)"
    printf '\nwhat the machine said:\n'
    sed 's/^/  /' "$CASE_DIR/observe.subject.err" 2>/dev/null
    printf '\nthe store directory afterwards:\n'
    sed 's/^/  /' "$CASE_DIR/observe.storedir" 2>/dev/null
}

# ── exposure ────────────────────────────────────────────────────────────────
#
# Two halves, and both are needed. The subject must have written into a tmpfs
# of this kind when there was room (or it does not write where the fault is),
# and the filesystem the subject actually met must have been full (or it met no
# fault). The second half reads the observation the inner script made INSIDE
# the namespace the subject ran in, which is the only place that question can
# be asked.
case_prove_exposed() {
    local rc enospc
    rc="$(cat "$CASE_DIR/baseline.subject.rc" 2>/dev/null || echo missing)"
    if [[ "$rc" != 0 ]]; then
        echo "with room on the same kind of tmpfs the subject still failed (rc=$rc), so nothing shows it writes where this case injects"
        return 1
    fi
    if ! grep -qi "$FAULT_CHECK" "$CASE_DIR/baseline.store.json" 2>/dev/null; then
        echo "the control run reported success but '$FAULT_CHECK' is not in the document it wrote"
        return 1
    fi
    enospc="$(cat "$CASE_DIR/observe.enospc" 2>/dev/null || echo missing)"
    if [[ "$enospc" != *refused* ]]; then
        echo "the filesystem the subject ran on was not full: $enospc"
        return 1
    fi
    echo "control wrote '$FAULT_CHECK' into a roomy tmpfs; the subject's own tmpfs refused a one-byte harness write"
    return 0
}

case_judge() {
    # The control, asserted as well as proven so it appears in the bundle's
    # expectation list.
    expect_rc "with room on the disk the result is recorded" 0 \
        "$(cat "$CASE_DIR/baseline.subject.rc" 2>/dev/null || echo missing)"

    # 1 ── it must fail, and loudly.
    expect_rc "a full disk makes the record fail" 1 \
        "$(cat "$CASE_DIR/observe.subject.rc" 2>/dev/null || echo missing)"
    expect_says "…and the machine names the reason rather than shrugging" \
        "No space left on device" "$(cat "$CASE_DIR/observe.subject.err" 2>/dev/null)"
    expect_says "…and names the file it was writing" \
        "qualification.json" "$(cat "$CASE_DIR/observe.subject.err" 2>/dev/null)"

    # 2 ── the atomic-write claim. This is the criterion-3 clause: a failed
    #      write must not corrupt the installation, and the document a person
    #      cannot reconstruct is the installation as far as §25 is concerned.
    local before after src
    before="$(cat "$CASE_DIR/observe.store.sha.before" 2>/dev/null || echo missing-before)"
    after="$(cat "$CASE_DIR/observe.store.sha.after" 2>/dev/null || echo missing-after)"
    src="$(cat "$CASE_DIR/observe.subject.rc" 2>/dev/null || echo '?')"
    if [[ "$before" == "$after" && "$before" != missing-* ]]; then
        _chaos_pass "the existing record is byte-identical after the refused write"
    else
        # Worded without assuming which way it went wrong. Replacing the
        # temp-and-rename with a plain `fs::write` makes this red, and under
        # that mutant the write SUCCEEDS — truncating the old document frees
        # the page the new one needs — so "the refused write damaged it" would
        # have been the wrong sentence to print. The subject's exit code is on
        # the line because it is what tells the two apart.
        _chaos_fail "the record did not survive: $before -> $after (the subject exited $src)"
    fi
    # And it is not merely present-and-empty: an emptied file has a sha too.
    local bytes
    bytes="$(cat "$CASE_DIR/observe.store.bytes.before" 2>/dev/null || echo 0)"
    if [[ "${bytes:-0}" -gt 100 ]]; then
        _chaos_pass "…and it was a real document to begin with ($bytes bytes)"
    else
        _chaos_fail "the fixture's record was only ${bytes:-0} bytes, so 'unchanged' proves nothing"
    fi

    # 3 ── no litter. A temp file left on the one filesystem that has no space
    #      is a cost, and one left at 0644 beside a 0600 document describing
    #      the owner's hardware is the mode comment in `qualify::save` being
    #      true only on the path that succeeds.
    local dir
    dir="$(cat "$CASE_DIR/observe.storedir" 2>/dev/null || true)"
    if grep -q '\.tmp' <<<"$dir"; then
        _chaos_fail "a temp file was left behind on the full filesystem:"
        while IFS= read -r l; do
            [[ "$l" == *.tmp* ]] || continue
            CHAOS_EXPECT_NOTES+=("       $l")
            printf '           %s\n' "$l" >&2
        done <<<"$dir"
    else
        _chaos_pass "no temp file was left behind on the full filesystem"
    fi
}

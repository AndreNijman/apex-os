#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  tests/test-apex-chaos.sh — the chaos harness, tested.
#
#  Roadmap P1-062. `tests/chaos/` is the thing that decides whether APEX
#  survives a fault, so the question "what decides whether the decider is
#  right?" has to have an answer that is not "somebody read it."
#
#  Two halves, and they are separate for the reason the rest of this repository
#  splits suites between the `static` and `rust` jobs:
#
#    STRUCTURAL   the harness may not touch the boot path. tests/chaos/lib.sh
#                 says so in a comment and says this file asserts it; until
#                 this file existed, that comment was a description of a check
#                 that did not run — this repository's own named defect, in the
#                 harness whose entire subject is assertions that cannot fail.
#                 Needs no binary, so it can live where no path filter can skip
#                 it.
#
#    BEHAVIOURAL  the driver's verdict arms, proven by feeding it synthetic
#                 cases whose injector, whose observer and whose exposure proof
#                 each cannot run. Every one of these asserts that the harness
#                 reports could-not-inject rather than a pass, because a chaos
#                 run that reports "no corruption" having injected nothing is
#                 the single failure this whole unit exists to refuse. The
#                 synthetic cases use `true`, `false` and `echo` as subjects, so
#                 this half needs no `apex` binary either.
#
#  This file does NOT run the real cases. They need a built binary, namespaces
#  and several seconds each; `tests/chaos/run-chaos` runs them, and CI runs that
#  in the `rust` job where the toolchain is.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
CHAOS="$REPO/tests/chaos"

pass=0; fail=0
ok()   { printf '  ok   %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf '  BAD  %s\n' "$1"; [[ -n "${2:-}" ]] && printf '       %s\n' "$2"; fail=$((fail + 1)); }
sec()  { printf '\n== %s ==\n' "$1"; }
is()   { if [[ "$2" == "$3" ]]; then ok "$1 ($2)"; else bad "$1" "want '$3', got '$2'"; fi; }

TMP="$(mktemp -d -t apex-chaos-selftest-XXXXXX)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

# ═══════════════════════════════════════════════════════════════════════════
#  STRUCTURAL — the harness is held to the boot-path rule it is testing others
#  against.
# ═══════════════════════════════════════════════════════════════════════════
#
# The check is deliberately not "does the file contain the string". Two things
# make that wrong here, and both are live in this tree:
#
#   * tests/chaos/lib.sh writes a fixture file NAMED `rpm-ostree-status.json`,
#     because that is what `apex channel` reads. A substring match calls that a
#     violation, and the fix a person would then reach for is renaming the
#     fixture away from the thing it represents.
#   * every case header discusses what it may not do, in prose, using the words
#     — `power-loss-during-update.sh` says "`apex update` writes this". A
#     substring match calls that a violation too.
#
# So the check looks for the token in COMMAND POSITION: at the start of a line,
# after `;`, `&&`, `||`, or inside `$( )` or backticks, optionally behind
# `sudo`. That is the position in which a word runs a program. `sudo` is
# OPTIONAL and follows the anchor rather than being an anchor of its own —
# written the other way round, with `[[:space:]]sudo[[:space:]]` as one of the
# alternatives, `sudo rpm-ostree status` at the start of a line matched
# nothing, because there is no space before `sudo` there. That mutant is in
# the list below precisely because the first version missed it. This repository has shipped four checks
# satisfied by their own comments, and `files/scripts/check-ai-parity
# --self-test` is the pattern for not shipping a fifth: the inverse mutants are
# asserted below, not assumed.

FORBIDDEN='bootctl|rpm-ostree|efibootmgr|ostree[[:space:]]+admin'

# boot_path_hits FILE — prints every line that RUNS one of the forbidden
# commands, prefixed by its line number. Silent when the file is clean.
boot_path_hits() {
    local f="$1"
    # Strip full-line comments and trailing comments before matching, so prose
    # about the rule is never the rule being broken. A `#` inside quotes is not
    # a comment, but no line in this tree has one, and treating it as one can
    # only make the check STRICTER on a real command, never blinder.
    sed 's/[[:space:]]#.*$//; s/^[[:space:]]*#.*$//' "$f" \
    | grep -nE "(^|[;&|]+|\\\$\\(|\`)[[:space:]]*(sudo[[:space:]]+)?($FORBIDDEN)([[:space:]]|$)" \
    || true
}

# apex_update_hits FILE — `apex update` in command position. Its own token,
# because `apex` on its own is the subject of nearly every case.
apex_update_hits() {
    local f="$1"
    sed 's/[[:space:]]#.*$//; s/^[[:space:]]*#.*$//' "$f" \
    | grep -nE '(^|[;&|]+|\$\(|`)[[:space:]]*(sudo[[:space:]]+)?(apex|"\$APEX_BIN"|\$APEX_BIN)[[:space:]]+update([[:space:]]|$)' \
    || true
}

sec "the harness may not touch the boot path"
violations=""
for f in "$CHAOS"/run-chaos "$CHAOS"/lib.sh "$CHAOS"/cases/*.sh; do
    [[ -e "$f" ]] || continue
    h="$(boot_path_hits "$f")"
    [[ -n "$h" ]] && violations+="${f#"$REPO"/}: $h"$'\n'
    h="$(apex_update_hits "$f")"
    [[ -n "$h" ]] && violations+="${f#"$REPO"/}: $h"$'\n'
done
if [[ -z "$violations" ]]; then
    ok "no file under tests/chaos/ runs bootctl, rpm-ostree, ostree admin, efibootmgr or apex update"
else
    bad "the harness reaches for the boot path" "$violations"
fi

sec "…and the check is not satisfied by its own comments"
probe="$TMP/probe.sh"
# THE INVERSE MUTANTS. Each of these must NOT trip the check, and each is a
# real line from this tree or a near copy of one.
cat > "$probe" <<'PROBE'
# a comment mentioning bootctl, rpm-ostree and efibootmgr in prose
#  ... and one saying `apex update` writes this file
printf '{}' > "$root/rpm-ostree-status.json"
echo "the documented sudo apex install steam line lives in a string"
PROBE
h="$(boot_path_hits "$probe")$(apex_update_hits "$probe")"
if [[ -z "$h" ]]; then
    ok "a comment naming bootctl, and a FILENAME containing rpm-ostree, do not trip it"
else
    bad "the check fires on prose or on a filename" "$h"
fi

# THE REAL MUTANTS. Each of these must trip it, or the check above passes
# vacuously — which would be the same defect one level up.
for line in 'bootctl install' \
            'sudo rpm-ostree status' \
            '    ostree  admin unlock --hotfix' \
            'x=1 && efibootmgr -v'; do
    printf '%s\n' "$line" > "$probe"
    if [[ -n "$(boot_path_hits "$probe")" ]]; then
        ok "it fires on a real command: $line"
    else
        bad "a boot-path command was not caught" "$line"
    fi
done
for line in 'apex update' 'sudo apex update --tag edge' '"$APEX_BIN" update'; do
    printf '%s\n' "$line" > "$probe"
    if [[ -n "$(apex_update_hits "$probe")" ]]; then
        ok "it fires on a real command: $line"
    else
        bad "an apex update invocation was not caught" "$line"
    fi
done

sec "the contract every case must satisfy"
for f in "$CHAOS"/cases/*.sh; do
    [[ -e "$f" ]] || continue
    n="$(basename "$f" .sh)"
    missing=""
    for v in CASE_TITLE CASE_CRITERION CASE_NEEDS; do
        grep -qE "^$v=" "$f" || missing+=" $v"
    done
    for fn in case_setup case_inject case_prove case_observe case_judge; do
        grep -qE "^$fn\(\)" "$f" || missing+=" $fn"
    done
    if [[ -z "$missing" ]]; then ok "$n declares the whole contract"
    else bad "$n is missing:$missing" ""; fi
done

# ═══════════════════════════════════════════════════════════════════════════
#  BEHAVIOURAL — the driver's verdict arms.
# ═══════════════════════════════════════════════════════════════════════════
#
# Synthetic cases in their own --cases-dir. The subjects are `true`, `false`
# and `echo`, so nothing here needs a built binary, a namespace or a second.
# What is being tested is the driver's ordering, which is the whole argument of
# P1-062: a case that could not inject its fault is never a pass.

CASES="$TMP/cases"
mkdir -p "$CASES"

# mkcase NAME BODY — write a synthetic case. Every one sets a fixture root, so
# the subject guard is satisfied except where a test is about the guard.
mkcase() {
    local name="$1"; shift
    { printf 'CASE_TITLE="synthetic: %s"\n' "$name"
      printf 'CASE_CRITERION="self-test"\n'
      cat; } > "$CASES/$name.sh"
}

# run_case NAME [BINARY] — runs one synthetic case and leaves three things
# behind: $STATE (the verdict), $RC (the driver's exit code) and $BUNDLE.
#
# It sets variables rather than printing the state, and that is not a style
# choice: the first version returned the state on stdout and every caller wrote
# `is "..." "$(run_case x)" "..."`, which runs the function in a SUBSHELL. $RC
# and $BUNDLE were then assigned in a process that exited immediately, the
# outer $RC stayed 0 from the previous case, and eighteen assertions in this
# file went red against a driver that was behaving perfectly. A self-test whose
# own plumbing lies is worse than no self-test, so the plumbing is the first
# thing this file gets right.
RC=0; BUNDLE=""; STATE=""
run_case() {
    local name="$1"; shift
    BUNDLE="$TMP/out-$name"
    rm -rf "$BUNDLE"
    RC=0
    env APEX_BIN="${1:-/bin/true}" APEX_TRUST_ROOT="$TMP/fixture" \
        "$CHAOS/run-chaos" --out "$BUNDLE" --cases-dir "$CASES" --seed 99 "$name" \
        > "$TMP/$name.log" 2>&1 || RC=$?
    STATE="$(sed -n 's/.*"state": "\([^"]*\)".*/\1/p' "$BUNDLE/$name/verdict.json" 2>/dev/null)"
    STATE="${STATE:-<no verdict>}"
}
mkdir -p "$TMP/fixture"

sec "a prerequisite that is not met is could-not-inject, never a pass"
mkcase prereq <<'CASE'
CASE_NEEDS="apex-binary"
case_setup()   { :; }
case_inject()  { :; }
case_prove()   { return 0; }
case_observe() { echo observed; }
case_judge()   { _chaos_pass "this must never be reached"; }
CASE
run_case prereq /nonexistent/apex
is "an absent binary gives could-not-inject" "$STATE" "could-not-inject"
is "…and the run fails" "$RC" "1"
if grep -q "not built" "$BUNDLE/prereq/verdict.json"; then
    ok "…and the verdict names the reason"
else
    bad "the verdict does not say why" "$(cat "$BUNDLE/prereq/verdict.json")"
fi

sec "a fault that was not proven present is could-not-inject"
mkcase unproven <<'CASE'
CASE_NEEDS=""
case_setup()   { :; }
case_inject()  { echo "pretending to inject"; }
case_prove()   { echo "the fault is not there"; return 1; }
case_observe() { echo observed; }
case_judge()   { _chaos_pass "this must never be reached"; }
CASE
run_case unproven
is "an injector that did nothing is not a pass" "$STATE" "could-not-inject"
is "…and the run fails" "$RC" "1"
if [[ ! -s "$BUNDLE/unproven/expectations.txt" ]] || \
   ! grep -q "must never be reached" "$BUNDLE/unproven/expectations.txt" 2>/dev/null; then
    ok "…and case_judge never ran, so no expectation was recorded"
else
    bad "the judge ran after an unproven fault" "$(cat "$BUNDLE/unproven/expectations.txt")"
fi

sec "a subject that could not be executed is could-not-inject"
mkcase nosubject <<'CASE'
CASE_NEEDS=""
case_setup()   { :; }
case_inject()  { :; }
case_prove()   { return 0; }
case_observe() { /nonexistent/subject; }
case_judge()   { _chaos_pass "this must never be reached"; }
CASE
run_case nosubject
is "rc 127 from the subject is inconclusive, not red" "$STATE" "could-not-inject"

sec "a fault the subject never met is could-not-inject — the fourth arm"
mkcase unexposed <<'CASE'
CASE_NEEDS=""
case_setup()          { :; }
case_inject()         { :; }
case_prove()          { echo "the fault is really there"; return 0; }
case_observe()        { echo "the subject ran and ignored it"; }
case_prove_exposed()  { echo "the subject never touched the faulted resource"; return 1; }
case_judge()          { _chaos_fail "this must never be reached"; }
CASE
run_case unexposed
is "a real fault the subject walked past is not a failure" "$STATE" "could-not-inject"
is "…and the run still fails" "$RC" "1"
if grep -q "never met it" "$BUNDLE/unexposed/verdict.json"; then
    ok "…and the verdict says the subject never met the fault"
else
    bad "the verdict does not distinguish this from a missing fault" \
        "$(cat "$BUNDLE/unexposed/verdict.json")"
fi
if ! grep -q "must never be reached" "$BUNDLE/unexposed/expectations.txt" 2>/dev/null; then
    ok "…and no expectation was judged, which is the point of the arm"
else
    bad "the judge ran on an unexposed subject" ""
fi

sec "an expectation that failed is failed, and one that passed is survived"
mkcase red <<'CASE'
CASE_NEEDS=""
case_setup()   { :; }
case_inject()  { :; }
case_prove()   { return 0; }
case_observe() { echo observed; }
case_judge()   { _chaos_pass "one that holds"; _chaos_fail "one that does not"; }
CASE
run_case red
is "a failed expectation is a failed case" "$STATE" "failed"
is "…and the run fails" "$RC" "1"

mkcase green <<'CASE'
CASE_NEEDS=""
case_setup()   { :; }
case_inject()  { :; }
case_prove()   { return 0; }
case_observe() { echo observed; }
case_prove_exposed() { return 0; }
case_judge()   { _chaos_pass "everything holds"; }
CASE
run_case green
is "a case that injected and survived is survived" "$STATE" "survived"
is "…and the run passes" "$RC" "0"

sec "the bundle is what makes a failure reproducible"
for f in verdict.json seed env.txt inject.out observe.out expectations.txt \
         state.before state.after state.diff; do
    if [[ -e "$BUNDLE/green/$f" ]]; then ok "the bundle carries $f"
    else bad "the bundle is missing $f" ""; fi
done
if grep -q '^apex-sha256: ' "$BUNDLE/green/env.txt"; then
    ok "…and the env record fingerprints the BINARY, not only the tree"
else
    bad "env.txt does not record the binary's digest" "$(cat "$BUNDLE/green/env.txt")"
fi

sec "a recorded verdict replays"
rrc=0
env APEX_BIN=/bin/true APEX_TRUST_ROOT="$TMP/fixture" \
    "$CHAOS/run-chaos" --cases-dir "$CASES" --replay "$TMP/out-green/green" \
    > "$TMP/replay.log" 2>&1 || rrc=$?
is "replaying a survived bundle reproduces it" "$rrc" "0"
if grep -q "the verdict reproduced" "$TMP/replay.log"; then
    ok "…and says so"
else
    bad "replay did not report a reproduction" "$(tail -3 "$TMP/replay.log")"
fi

sec "a case that forgets its fixture root aborts the run"
# NOT a verdict. A case that ran the subject against the developer's live
# machine has not produced a result, it has produced an accident, so the driver
# exits 2 — the harness-is-broken code — and never writes a verdict at all.
mkcase unrooted <<'CASE'
CASE_NEEDS=""
case_setup()   { :; }
case_inject()  { :; }
case_prove()   { return 0; }
case_observe() { env -u APEX_TRUST_ROOT "$APEX_BIN" status-of-the-real-machine; }
case_judge()   { _chaos_pass "this must never be reached"; }
CASE
grc=0
GB="$TMP/out-unrooted"
env APEX_BIN=/bin/true APEX_TRUST_ROOT="$TMP/fixture" \
    "$CHAOS/run-chaos" --out "$GB" --cases-dir "$CASES" --seed 99 unrooted \
    > "$TMP/unrooted.log" 2>&1 || grc=$?
is "an unrooted subject invocation is exit 2, not a verdict" "$grc" "2"
if grep -qP 'observe\tstatus-of-the-real-machine' "$GB/unrooted-subject-invocations" 2>/dev/null; then
    ok "…and the marker names the phase and the argv"
else
    bad "the guard marker does not name what was run" \
        "$(cat "$GB/unrooted-subject-invocations" 2>/dev/null)"
fi

sec "a malformed case is the harness's problem, not a verdict"
printf 'CASE_TITLE=""\n' > "$CASES/broken.sh"
brc=0
env APEX_BIN=/bin/true APEX_TRUST_ROOT="$TMP/fixture" \
    "$CHAOS/run-chaos" --out "$TMP/out-broken" --cases-dir "$CASES" --seed 99 broken \
    > "$TMP/broken.log" 2>&1 || brc=$?
is "a case with no title exits 2" "$brc" "2"
rm -f "$CASES/broken.sh"

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[[ "$fail" -eq 0 ]]

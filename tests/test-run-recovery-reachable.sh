#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-run-recovery-reachable.sh — the mutation harness for
#  files/scripts/check-run-recovery-reachable.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  Round 37: every `core` image build died at
#
#      RUN set -eux; … akmods --force --kernels "${KVER}" --kmod "${k}"; …
#          || { echo "FATAL: …"; sed -n '1,80p' …/*.failed.log; exit 1; }
#
#  and the build log contained no compiler error, because the `sed` that dumps
#  akmods' failure log is unreachable: `akmods` is an unguarded simple command
#  under `set -e`, so its failure terminates the RUN before the recovery block
#  is ever read. Two roadmap rounds were spent on a failure nobody could read.
#
#  The checker is the gate. THIS file is the gate on the checker, because a
#  checker that answers "OK" to everything is this repository's dominant CI
#  defect (see the memory note "a gate that runs and inspects nothing").
#
#  ── What is asserted ────────────────────────────────────────────────────────
#  Every case is a synthetic Containerfile run through the real checker as a
#  process, and cases exist on BOTH sides:
#
#    * a checker that always exits 1 fails every GREEN case below — including
#      the one that matters most, `/var/cache/akmods/...` appearing as a PATH
#      in a `sed` argument, which a naive `grep -q akmods` would flag;
#    * a checker that always exits 0 fails every RED case, including the exact
#      shape that shipped.
#
#  Neither direction can pass by accident.
#
#  Nothing is built, nothing is pulled, no network, no root. Runs anywhere.
#
#  PASS = every case's exit code and message are what they should be.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

CHECKER=../files/scripts/check-run-recovery-reachable
[ -f "$CHECKER" ] || { echo "cannot find $CHECKER"; exit 2; }
CHECKER=$(cd "$(dirname "$CHECKER")" && pwd)/$(basename "$CHECKER")
REPO=$(cd .. && pwd)

WORK=$(mktemp -d /var/lab-scratch/apex-run-recovery.XXXXXX 2>/dev/null \
       || mktemp -d)
trap 'rm -rf "${WORK:?}"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %-68s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-68s %s\n' "$1" "$2"; fail=$((fail+1)); }

OUT=""
RC=0
# run_case <name> — writes a Containerfile from stdin and runs the checker.
run_case() {
    local root="$WORK/$1"
    rm -rf "$root"; mkdir -p "$root"
    cat > "$root/Containerfile"
    OUT=$(python3 "$CHECKER" "$root/Containerfile" 2>&1); RC=$?
}

expect_rc() {
    if [ "$RC" = "$2" ]; then ok "$1"
    else bad "$1" "expected exit $2, got $RC: $(head -3 <<<"$OUT" | tr '\n' ' ')"; fi
}
expect_says() {
    if grep -qF -- "$2" <<<"$OUT"; then ok "$1"
    else bad "$1" "expected $(printf '%q' "$2") in: $(tr '\n' ' ' <<<"$OUT")"; fi
}
expect_silent() {
    if grep -qF -- "$2" <<<"$OUT"; then bad "$1" "found $(printf '%q' "$2") and should not have"
    else ok "$1"; fi
}

echo "── RED: the defect the checker exists to catch ──"

# The shipped one, verbatim in shape, loop and all.
run_case shipped-shape <<'CF'
FROM fedora:43
RUN set -eux; \
    KVER="$(cat /usr/lib/apex-kver)"; \
    for k in nvidia xone; do \
        akmods --force --kernels "${KVER}" --kmod "${k}"; \
        RPM="$(ls -1 /var/cache/akmods/${k}/kmod-${k}-*.rpm 2>/dev/null | head -1)"; \
        test -n "${RPM}" && test -f "${RPM}" \
            || { echo "FATAL: ${k} akmod produced no kmod RPM"; \
                 sed -n '1,80p' /var/cache/akmods/${k}/*.failed.log 2>/dev/null; exit 1; }; \
    done
CF
expect_rc   "the shape that shipped is refused" 1
expect_says "and it names the unguarded command" "akmods --force --kernels"
expect_says "and it says the recovery is dead code" "dead code"

# The rule stands on its own: no recovery block at all is still unreadable.
run_case no-recovery-block <<'CF'
FROM fedora:43
RUN set -eux; akmods --force --kernels 7.2.6 --kmod nvidia; echo done
CF
expect_rc   "an unguarded akmods with no recovery block at all is refused" 1
expect_silent "and it does not claim a dead recovery that is not there" "dead code"

run_case full-path <<'CF'
FROM fedora:43
RUN set -eux; /usr/sbin/akmods --force --kernels 7.2.6 --kmod nvidia
CF
expect_rc   "an absolute path to the driver is still the driver" 1

run_case env-prefix <<'CF'
FROM fedora:43
RUN set -eux; KVER=7.2.6 LC_ALL=C akmods --force --kmod nvidia
CF
expect_rc   "leading environment assignments do not hide the command" 1

run_case and-list-is-not-a-guard <<'CF'
FROM fedora:43
RUN set -eux; akmods --force --kmod nvidia && echo "built"
CF
expect_rc   "\`cmd && next\` is NOT a guard — the AND-list still fails" 1

run_case inverted <<'CF'
FROM fedora:43
RUN set -eux; ! akmods --force --kmod nvidia; echo "carried on regardless"
CF
expect_rc   "a bare inverted call is refused too" 1
expect_says "and it says why an inverted command is worse, not better" "errexit does not apply to an inverted command"

run_case other-drivers <<'CF'
FROM fedora:43
RUN set -eux; dkms build -m nvidia -v 580.178.04 -k 7.2.6
RUN set -euo pipefail; rpmbuild --rebuild --quiet /tmp/x.src.rpm
RUN set -eux; akmodsbuild --quiet --kernels 7.2.6 /usr/src/akmods/nvidia-kmod.latest
CF
expect_rc   "dkms, rpmbuild and akmodsbuild are covered too" 1
expect_says "and all three are reported, not just the first" "akmodsbuild --quiet"

echo
echo "── GREEN: the shapes that are actually safe ──"

run_case if-not-guard <<'CF'
FROM fedora:43
RUN set -eux; \
    if ! akmods --force --kernels 7.2.6 --kmod nvidia; then \
        cat /var/cache/akmods/nvidia/.last.log; exit 1; \
    fi
CF
expect_rc   "\`if ! akmods …; then dump; fi\` passes" 0

run_case rc-capture <<'CF'
FROM fedora:43
RUN set -eux; rc=0; akmods --force --kmod nvidia || rc=$?; echo "rc=${rc}"
CF
expect_rc   "\`akmods … || rc=\$?\` passes" 0

run_case and-then-or <<'CF'
FROM fedora:43
RUN set -eux; akmods --force --kmod nvidia && echo ok || { cat /var/log/akmods/akmods.log; exit 1; }
CF
expect_rc   "\`a && b || c\` passes — c catches a's failure" 0

# THE CONTROL FOR A NAIVE CHECKER. `grep -q akmods` over this file finds four
# hits and none of them is a command. Without this case the checker could be
# "does the word akmods appear", which flags every correct Containerfile in
# the tree and would be ripped out within a day.
run_case akmods-as-a-path <<'CF'
FROM fedora:43
RUN set -eux; \
    mkdir -p /var/cache/akmods/nvidia; \
    sed -n '1,80p' /var/cache/akmods/nvidia/x.failed.log; \
    echo "see /var/log/akmods/akmods.log for details"; \
    find /var/cache/akmods -type f
CF
expect_rc   "the word akmods in a PATH is not a call" 0
expect_says "and the file is reported as really read" "errexit RUN(s)"

run_case no-errexit <<'CF'
FROM fedora:43
RUN akmods --force --kmod nvidia; cat /var/cache/akmods/nvidia/.last.log
CF
expect_rc   "a RUN without errexit is out of scope — the dump does run" 0

run_case while-condition <<'CF'
FROM fedora:43
RUN set -eux; n=0; while ! akmods --force --kmod nvidia; do n=$((n+1)); [ "$n" -lt 3 ] || exit 1; done
CF
expect_rc   "a call in a while-condition passes" 0

run_case comment-and-argument <<'CF'
FROM fedora:43
# akmods --force --kmod nvidia would be wrong here; see the note above.
RUN set -eux; echo "akmods --force --kmod nvidia"; true
CF
expect_rc   "a comment and a quoted argument are not calls" 0

echo
echo "── an input that is named but absent is a failure, not a skip ──"

OUT=$(python3 "$CHECKER" "$WORK/no-such-containerfile" 2>&1); RC=$?
expect_rc   "a Containerfile named but absent is refused" 1
expect_says "and it says so" "MISSING"

echo
echo "── and the shipped Containerfiles, which must be clean ──"

CFS=()
for f in Containerfile.base Containerfile.core Containerfile.kernel \
         Containerfile.apex Containerfile.release; do
    [ -f "$REPO/$f" ] && CFS+=("$f")
done
OUT=$(cd "$REPO" && python3 "$CHECKER" "${CFS[@]}" 2>&1); RC=$?
expect_rc   "the shipped Containerfiles pass" 0
for f in "${CFS[@]}"; do
    expect_says "$f reports a RUN count, so it was really read" "$f: OK ["
done

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1

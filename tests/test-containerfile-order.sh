#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-containerfile-order.sh — the mutation harness for
#  files/scripts/check-containerfile-order.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  That checker is the gate on a class of defect that has cost this project
#  five days of image builds: a Containerfile assertion that cannot possibly
#  pass, discovered hours into a build. `tests/test-apex-greet-sessions.sh`
#  runs it against the real Containerfiles, which asserts that THE IMAGE is in
#  order; nothing anywhere asserted that THE CHECKER still works. A checker
#  that answers "layer order OK" to everything is this repository's dominant CI
#  defect — a gate that runs and inspects nothing — sitting on top of the gate
#  that catches it.
#
#  The occasion was a false positive. On run 34727364060 the checker read
#
#      grep -q '^ExecStart=/usr/bin/apex lid watch$' \
#          /usr/lib/systemd/system/apex-lid.service
#
#  as "RUN reads /usr/bin/apex", which the COPY at the bottom of the file
#  provides, and failed the build's static job — taking the twenty static gates
#  below it off every run, because a failed step skips everything under it.
#  The fix strips a quoted PATTERN operand and keeps FILE operands. Both halves
#  are asserted here, because stripping too much would silently retire the
#  commonest real shape of the defect (`RUN grep -q x /path/copied/later`).
#
#  ── What is asserted ────────────────────────────────────────────────────────
#  Every case is a synthetic Containerfile in a throwaway repo root, run
#  through the real checker as a process. Each case states the exit code it
#  expects, and cases exist on BOTH sides: a checker that always said 0 fails
#  the read-before-COPY cases, and one that always said 1 fails the
#  pattern-operand cases. Neither direction can pass by accident.
#
#  Nothing is built, nothing is pulled, no network, no root. Runs anywhere.
#
#  PASS = every case's exit code and message are what they should be.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2

CHECKER=../files/scripts/check-containerfile-order
[ -f "$CHECKER" ] || { echo "cannot find $CHECKER"; exit 2; }
CHECKER=$(cd "$(dirname "$CHECKER")" && pwd)/$(basename "$CHECKER")
REPO=$(cd .. && pwd)

WORK=$(mktemp -d /tmp/apex-cf-order.XXXXXX)
trap 'rm -rf "${WORK:?}"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %-64s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-64s %s\n' "$1" "$2"; fail=$((fail+1)); }

OUT=""
# run_case <name> — writes a Containerfile from stdin into a fresh root and
# runs the checker on it. Leaves the output in $OUT and the status in $RC.
RC=0
run_case() {
    local root="$WORK/$1"
    rm -rf "$root"
    mkdir -p "$root/files/desktop/labwc"
    printf '<labwc_config/>\n' > "$root/files/desktop/labwc/rc.xml"
    printf 'menu\n'            > "$root/files/desktop/labwc/menu.xml"
    cat > "$root/Containerfile"
    OUT=$(python3 "$CHECKER" "$root/Containerfile" 2>&1); RC=$?
}

# expect_rc <name> <want> — the exit code, which is the only thing CI reads.
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

echo "── the defect the checker exists to catch, still caught ──"

# The historical one, verbatim in shape: the context-menu assertions read
# rc.xml about 240 lines above the COPY that provides it.
run_case read-before-copy <<'CF'
FROM fedora:43
RUN set -eux; test -s /usr/share/apex/labwc/rc.xml
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc   "a RUN that reads a path above its COPY is refused" 1
expect_says "and it names the path and both line numbers" "RUN reads /usr/share/apex/labwc/rc.xml"

# THE CONTROL FOR THE FIX. `grep` now has its pattern stripped; its FILE
# operands must still be read. Without this case the fix could have been
# "ignore every path on a grep line", which retires the commonest real shape of
# the defect while still passing every other case in this file.
run_case grep-file-operand <<'CF'
FROM fedora:43
RUN set -eux; grep -q '<labwc_config' /usr/share/apex/labwc/rc.xml
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc   "grep's FILE operand is still a read" 1
expect_says "and it names the file, not the pattern" "RUN reads /usr/share/apex/labwc/rc.xml"

# Two operands, one provided above and one below: the checker must not stop at
# the first path it clears.
run_case second-operand <<'CF'
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/share/apex/labwc/menu.xml
RUN set -eux; grep -q 'menu' /usr/share/apex/labwc/menu.xml; test -s /usr/share/apex/labwc/rc.xml
COPY files/desktop/labwc/rc.xml /usr/share/apex/labwc/rc.xml
CF
expect_rc   "a later operand on the same RUN is still checked" 1
expect_says "and the message is about rc.xml" "RUN reads /usr/share/apex/labwc/rc.xml"

echo
echo "── a pattern is matched text, not a file that was opened ──"

# The live false positive, reduced. The unit file IS read and IS provided
# above; /usr/share/apex/labwc/rc.xml appears only inside the quoted pattern.
run_case pattern-operand <<'CF'
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/lib/systemd/system/apex-lid.service
RUN set -eux; grep -q '^ExecStart=/usr/share/apex/labwc/rc.xml watch$' /usr/lib/systemd/system/apex-lid.service
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc     "a path that only appears in a grep pattern is not a read" 0
expect_silent "and nothing is reported against it" "ordering problem"

run_case pattern-double-quoted <<'CF'
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/lib/systemd/system/apex-lid.service
RUN set -eux; grep -qF "/usr/share/apex/labwc/rc.xml" /usr/lib/systemd/system/apex-lid.service
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc "a double-quoted pattern is a pattern too" 0

run_case pattern-dash-e <<'CF'
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/lib/systemd/system/apex-lid.service
RUN set -eux; grep -q -e '/usr/share/apex/labwc/rc.xml' /usr/lib/systemd/system/apex-lid.service
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc "flags between the command and the pattern are skipped" 0

run_case pattern-sed <<'CF'
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/lib/systemd/system/apex-lid.service
RUN set -eux; sed -i 's|/usr/share/apex/labwc/rc.xml|x|' /usr/lib/systemd/system/apex-lid.service
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc "sed's script is a pattern as well" 0

# The idiom the checker already carved out, kept under test so the two strips
# cannot uncover each other.
run_case prose <<'CF'
FROM fedora:43
RUN set -eux; : 'this stanza must NOT touch /usr/share/apex/labwc/rc.xml yet'; true
COPY files/desktop/labwc/ /usr/share/apex/labwc/
CF
expect_rc 'a path named only in the : prose idiom is not a read' 0

echo
echo "── the cross-stage shape the live file actually has ──"

# An earlier stage says what it produces with `install -D`, a COPY --from maps
# it, and a RUN above that COPY uses the binary. This is the only way the
# checker can see across a stage boundary, and it is exactly the arrangement
# Containerfile.base uses for /usr/bin/apex.
run_case cross-stage-read <<'CF'
FROM fedora:43 AS apex-builder
RUN set -eux; install -Dm0755 /bin/true /out/usr/bin/apex
FROM fedora:43
RUN set -eux; /usr/bin/apex lid --help
COPY --from=apex-builder /out/usr/bin/ /usr/bin/
CF
expect_rc   "a cross-stage binary used above its COPY is refused" 1
expect_says "and it names the binary" "RUN reads /usr/bin/apex"

# The same file, with the only mention of the binary inside a grep pattern.
# This IS Containerfile.base:1644, and it must pass.
run_case cross-stage-pattern <<'CF'
FROM fedora:43 AS apex-builder
RUN set -eux; install -Dm0755 /bin/true /out/usr/bin/apex
FROM fedora:43
COPY files/desktop/labwc/menu.xml /usr/lib/systemd/system/apex-lid.service
RUN set -eux; grep -q '^ExecStart=/usr/bin/apex lid watch$' /usr/lib/systemd/system/apex-lid.service
COPY --from=apex-builder /out/usr/bin/ /usr/bin/
CF
expect_rc     "the live Containerfile.base:1644 shape passes" 0
expect_silent "and /usr/bin/apex is not reported" "/usr/bin/apex"

echo
echo "── the other half of the checker, unchanged and still armed ──"

run_case volatile-early <<'CF'
FROM fedora:43 AS apex-builder
RUN set -eux; install -Dm0755 /bin/true /out/usr/bin/apex
FROM fedora:43
COPY --from=apex-builder /out/usr/bin/ /usr/bin/
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
RUN set -eux; true
CF
expect_rc   "volatile content near the top is refused" 1
expect_says "and it says what it costs" "rebuilds AND re-pushes"

# A named input that is not on disk must be a failure, never a skip: CI passes
# these paths explicitly, so a rename would otherwise stop validating a whole
# image variant while still reporting green.
OUT=$(python3 "$CHECKER" "$WORK/no-such-containerfile" 2>&1); RC=$?
expect_rc   "a Containerfile named but absent is a failure, not a skip" 1
expect_says "and it says so" "MISSING"

echo
echo "── and the three real ones, which must be in order ──"

OUT=$(cd "$REPO" && python3 "$CHECKER" Containerfile.base Containerfile.core Containerfile.apex 2>&1); RC=$?
expect_rc   "the shipped Containerfiles pass" 0
for f in Containerfile.base Containerfile.core Containerfile.apex; do
    expect_says "$f reports a layer count, so it was really read" "$f: layer order OK ["
done

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1

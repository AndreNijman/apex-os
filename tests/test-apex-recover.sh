#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-recover.sh — assertions for roadmap §19: the recovery surface, the
#  scoped factory reset, and disposable environments.
#
#  Two halves, and the split is deliberate:
#
#    * bare        the shipped disposable engine
#                  (files/system/libexec/apex-disposable) and the structural
#                  rules that must hold in the shipped file. Needs no Rust
#                  toolchain, so it runs in pr-validation's `static` job, which
#                  has NO path filter and can therefore never be skipped.
#    * --with-binary
#                  `apex recover` and `apex doctor --json` against fixture
#                  trees, driving the real built binary. Runs in the `rust`
#                  job, where the toolchain is.
#
#  This is `test-boot-v2.sh`'s shape, and it is used for the same reason that
#  file gives: the `rust` selector is ^(apexd/|config/|tests/) and the `engine`
#  selector is ^(files/|tests/), so a PR touching ONLY
#  files/system/libexec/apex-disposable sets engine=true and rust=false. A
#  binary-driven suite living only in `rust` would be skipped by exactly that
#  PR — and a skipped job counts as success. This file records three such
#  instances; this is not a fourth.
#
#  ── THIS SUITE MUST BE INCAPABLE OF DESTROYING THE MACHINE IT RUNS ON ──────
#  It exercises a factory reset and a recursive delete. Four independent things
#  make that safe, and each of them is itself asserted:
#
#    1. Every destructive path runs against a temp tree. $HOME, $XDG_*, and
#       APEX_DISPOSABLE_ROOT are all redirected into $WORK, and the suite
#       HARD-EXITS if any of them resolves outside it. A skip would not do:
#       a run that quietly used the real directories is the accident the guard
#       exists for.
#    2. A PATH tripwire. Fake `bootctl`, `efibootmgr`, `ostree`, `bootc`,
#       `podman`, `distrobox`, `systemctl`, `pkexec`, `busctl`, `sudo` and
#       `flatpak` record every invocation, and the run FAILS if any of them is
#       called. `pkexec`, `busctl` and `sudo` are in that list specifically so
#       that "§19 never raises an authentication prompt" is an assertion rather
#       than a claim — an earlier suite in this repository raised a burst of
#       polkit prompts on the developer's desktop and nearly changed his
#       scheduler.
#    3. CANARIES OUTSIDE THE FIXTURE. The tripwire proves nothing about the
#       Rust half: `std::fs::remove_dir_all` spawns no `rm`, so a fake `rm` on
#       PATH would never fire. So files are planted outside every fixture root
#       — including one a symlink points at — and asserted present at the end.
#    4. A final check that the developer's own ~/.config/apex-shell,
#       ~/.config/apex and ~/.local/state/apex are byte-identical to what they
#       were before the run.
#
#  PASS = every verb behaves, every refusal refuses WITH THE REASON UNDER TEST,
#         no external command was spawned, and nothing outside $WORK moved.
#
#  Run from anywhere:  ./tests/test-apex-recover.sh [--with-binary]
# ─────────────────────────────────────────────────────────────────────────────
# `set +e`: this suite COUNTS failures rather than aborting, and many cases run
# commands that exit non-zero on purpose. GitHub Actions invokes a script as
# `bash -e {0}`, and under `-e` the first such command ends the run silently
# instead of reporting anything.
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2
REPO=$(cd .. && pwd)

WITH_BINARY=0
[ "${1:-}" = "--with-binary" ] && WITH_BINARY=1

ENGINE="$REPO/files/system/libexec/apex-disposable"
[ -f "$ENGINE" ] || { echo "FATAL: cannot find $ENGINE" >&2; exit 2; }

pass=0; fail=0
ok()  { printf 'PASS  %-64s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-64s %s\n' "$1" "${2:-}"; fail=$((fail+1)); }
sec() { printf '\n── %s ──\n' "$1"; }
has() {
    local name=$1 want=$2 hay=$3
    if grep -qF -- "$want" <<<"$hay"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want") in: $(head -4 <<<"$hay" | tr '\n' '|')"; fi
}
hasnt() {
    local name=$1 unwanted=$2 hay=$3
    if grep -qF -- "$unwanted" <<<"$hay"; then
        bad "$name" "found $(printf '%q' "$unwanted") and should not have"
    else ok "$name"; fi
}
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$want"), got $(printf '%q' "$got")"; fi
}

# A missing prerequisite is a FAILURE, never a skip. A suite that reports
# "0 passed, 0 failed" and a green tick has asserted precisely nothing, which
# has happened in this repository before.
command -v python3 >/dev/null 2>&1 || {
    echo "FATAL: python3 is required to validate the JSON output" >&2; exit 2; }
command -v realpath >/dev/null 2>&1 || {
    echo "FATAL: realpath is required; the engine's teardown depends on it" >&2; exit 2; }
# Deliberately NOT `diff`: it is diffutils, it is absent from this project's
# `apex-rust` container, and the final comparison is done in the shell instead.

# ── this suite cannot run as root, and refuses rather than half-running ─────
# `apex disposable run` and `apex recover reset` both refuse root BY DESIGN —
# a root capsule is stored under /var/lib/containers and shared by every
# account, and root's home is not the user's, so a root reset would clear the
# wrong account while reporting success. A suite running as root would
# therefore exercise those refusals on every case that matters and report a
# green run having tested nothing, which is the vacuous shape this repository
# has been bitten by. Measured: under root in `apex-rust`, 18 cases failed and
# every one of them failed because the program correctly refused.
#
# A refusal, not a skip, and not a privilege drop: dropping would leave the
# --with-binary half unable to build. Both GitHub jobs that run this file
# (`static` and `rust`) run as an ordinary user, so this never fires in CI. It
# fires when someone runs it in a root container — exactly the case where the
# behaviour change would otherwise be invisible.
if [ "$(id -u)" = 0 ]; then
    {
        echo "FATAL: this suite must not run as root."
        echo "  \`apex disposable run\` and \`apex recover reset\` refuse root by design,"
        echo "  so every case that drives them would assert the refusal instead of the"
        echo "  behaviour, and the run would be green having tested nothing."
        echo "  Run it as an ordinary user; both CI jobs already do."
        echo "  In a container:  podman run --user 1000 -e HOME=/tmp/h …"
    } >&2
    exit 2
fi

WORK=$(mktemp -d /tmp/apex-recover-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

# ── the developer's own state, captured before anything runs ────────────────
# Compared at the end. `apex recover reset` deletes files under ~/.config and
# ~/.local/state; if it ever resolved its paths from the passwd database
# instead of $HOME, this is what catches it — and the cost of not catching it
# is the developer's desktop configuration.
REAL_STATE_SUM="$WORK/real-state-before"
# The real home, captured NOW. $HOME is redirected into $WORK a few lines
# below, so a helper that read $HOME at the end would be comparing the fixture
# with itself and reporting a pass no matter what the suite had done.
REAL_HOME="$HOME"
real_state_sum() {
    { for d in "$REAL_HOME/.config/apex-shell" "$REAL_HOME/.config/apex" \
               "$REAL_HOME/.local/state/apex" "$REAL_HOME/.config/hypr"; do
          [ -e "$d" ] && find "$d" -printf '%p %s\n' 2>/dev/null
      done; } | LC_ALL=C sort
}
real_state_sum > "$REAL_STATE_SUM"

# ── canaries: what must still exist when this is over ───────────────────────
# Placed OUTSIDE every fixture root, including one that a symlink inside a
# fixture points at. The PATH tripwire below cannot see a Rust
# `remove_dir_all`, so these are what make "the reset did not escape" a
# measurement rather than a hope.
# A sibling of $WORK, made with its own mktemp rather than spelled as
# "$WORK/../…": `rm -rf "$WORK" "$CANARY"` removes $WORK first, and the second
# path then no longer resolves through the parent that just vanished, so -f
# swallows the ENOENT and the canary is left behind. Nine of them accumulated
# in /tmp before that was noticed.
CANARY=$(mktemp -d /tmp/apex-recover-canary.XXXXXX)
mkdir -p "$CANARY/precious"
printf 'do not delete\n' > "$CANARY/precious/data"
printf 'do not delete\n' > "$CANARY/loose-file"
CANARY_SUM=$(LC_ALL=C find "$CANARY" -printf '%p %s\n' | sort)
# The canary directory is outside $WORK, so it is removed by its own trap.
trap 'rm -rf "$WORK" "$CANARY"' EXIT

# ── the tripwire ────────────────────────────────────────────────────────────
SPAWN_LOG="$WORK/spawned.log"
: > "$SPAWN_LOG"
FAKEBIN="$WORK/fakebin"
mkdir -p "$FAKEBIN"
for tool in bootctl bootupctl efibootmgr ostree bootc podman distrobox \
            systemctl pkexec busctl sudo flatpak rpm-ostree fwupdmgr; do
    cat > "$FAKEBIN/$tool" <<EOF
#!/usr/bin/env bash
echo "$tool \$*" >> "$SPAWN_LOG"
exit 0
EOF
    chmod +x "$FAKEBIN/$tool"
done
export PATH="$FAKEBIN:$PATH"

# ── the sandbox every case runs in ──────────────────────────────────────────
FAKEHOME="$WORK/home"
DISP_ROOT="$WORK/disposable"
mkdir -p "$FAKEHOME" "$DISP_ROOT"
export HOME="$FAKEHOME"
export XDG_CONFIG_HOME="$FAKEHOME/.config"
export XDG_STATE_HOME="$FAKEHOME/.local/state"
export XDG_DATA_HOME="$FAKEHOME/.local/share"
export XDG_CACHE_HOME="$FAKEHOME/.cache"
export APEX_DISPOSABLE_ROOT="$DISP_ROOT"

sec "the suite must not be able to reach anything real"
# HARD EXIT, never a skip. Everything below deletes.
for guard in "$FAKEHOME:HOME" "$DISP_ROOT:APEX_DISPOSABLE_ROOT"; do
    p=${guard%%:*}; n=${guard##*:}
    case "$p" in
        "$WORK"/*) ok "$n is inside the temp tree" ;;
        *) echo "FATAL: $n=$p is outside $WORK" >&2; exit 2 ;;
    esac
done
# And resolved, not merely spelled: a symlinked $TMPDIR would make the string
# check above pass while the real writes landed elsewhere.
if [ "$(realpath -e "$FAKEHOME")" = "$(realpath -e "$WORK")/home" ]; then
    ok "HOME resolves inside the temp tree, not merely starts with its name"
else
    echo "FATAL: HOME resolves to $(realpath -e "$FAKEHOME")" >&2; exit 2
fi

# ── the stub capsule engine ─────────────────────────────────────────────────
# apex-disposable drives /usr/libexec/apex-env. Faking it is what lets this
# suite assert the exact argv of a verb that otherwise pulls hundreds of
# megabytes and makes a container — the same technique test-apex-env.sh uses
# for distrobox, and for the same reason: a previous suite in this repository
# reached the host and changed the developer's CPU scheduler.
CALLS="$WORK/env-calls"
STUB="$WORK/stub-apex-env"
cat > "$STUB" <<EOF
#!/usr/bin/env bash
{ printf 'env'; printf ' <%s>' "\$@"; printf '\n'; } >> "$CALLS"
# The one side effect a real capsule has that this suite reads back: a command
# run inside can leave something in ~/out. Without it every copy-out assertion
# would pass vacuously against an empty directory.
if [ -n "\${STUB_PRODUCES:-}" ]; then
    case "\$1" in
        exec|enter)
            mkdir -p "${DISP_ROOT}/\$2/home/out"
            printf 'produced\n' > "${DISP_ROOT}/\$2/home/out/\${STUB_PRODUCES}" ;;
    esac
fi
# Two separate knobs, because they are two different failures and conflating
# them made the exit-status case pass for the wrong reason: with one variable,
# `create` failed first and the run died at 1 before the inner command ran.
case "\$1" in
    create) exit "\${STUB_CREATE_EXIT:-0}" ;;
    exec|enter) exit "\${STUB_EXIT:-0}" ;;
esac
exit 0
EOF
chmod +x "$STUB"
export APEX_DISPOSABLE_ENV_ENGINE="$STUB"

disp() { "$ENGINE" "$@" 2>&1; }
resetcalls() { : > "$CALLS"; }

# ── prove the harness is armed ──────────────────────────────────────────────
# Without this every assertion below could be passing because the engine never
# ran the capsule engine at all. This repository has shipped a check that was
# satisfied by its own comments; a tripwire that is never armed is the same
# failure.
sec "the harness itself"
resetcalls
disp run --name armed -- true >/dev/null
if [ -s "$CALLS" ]; then
    ok "the stub capsule engine is the one being driven"
else
    bad "the stub capsule engine is the one being driven" \
        "nothing was recorded — every argv assertion below would be vacuous"
fi
has "…and it was asked to CREATE a capsule, so the mode really is a capsule" \
    "<create>" "$(cat "$CALLS")"
if [ ! -s "$SPAWN_LOG" ]; then
    ok "no bootctl/efibootmgr/ostree/podman/pkexec/sudo was spawned so far"
else
    bad "no external command was spawned" "$(cat "$SPAWN_LOG")"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the shipped engine's structural rules"
# Greps anchored to EXECUTABLE lines (^[^#]*), because this file's comments
# discuss every one of these checks at length. A check its own comments can
# satisfy proves nothing, and this repository has shipped five of them.
DANGER_FREE=1
check_exec_line() {
    local label=$1 pattern=$2
    if grep -qE "^[^#]*$pattern" "$ENGINE"; then ok "$label"
    else bad "$label" "no executable line matches /$pattern/"; fi
}
check_exec_line "teardown validates the name against the disp- allowlist" \
    '\[\[ "\$1" =~ \^disp-\[a-z0-9\]\{1,24\}\$ \]\]'
check_exec_line "teardown refuses a symlink at the final component" \
    '\[ -L "\$dir" \]'
check_exec_line "teardown resolves the path with realpath -e" \
    'realpath -e -- "\$dir"'
check_exec_line "teardown asserts the resolved path EQUALS <root>/<name>" \
    '\[ "\$real" = "\$\{root_real\}/\$\{name\}" \]'
# The suite refuses to run as root (see the top of this file), so it cannot
# exercise this one directly. Asserted structurally here, and behaviourally in
# Containerfile.base, which runs `apex recover reset` AS root during the build
# and checks both the exit status and the reason.
check_exec_line "run refuses to be executed as root" 'require_rootless'
check_exec_line "the capsule is created with its own --home" '--home='
check_exec_line "and with no device access" '--gpu=none'

# The two controls that make the greps above mean something. Without them the
# check could be reporting "matched" because the pattern matches anything, or
# "clean" because it matches nothing at all.
CTRL="$WORK/control"
printf '#!/bin/sh\n# a comment mentioning [ -L "$dir" ] on purpose\necho hi\n' > "$CTRL"
if grep -qE '^[^#]*\[ -L "\$dir" \]' "$CTRL"; then
    bad "inverse control: a comment does not satisfy an executable-line grep" \
        "it did — every structural check above is worthless"
else
    ok "inverse control: a comment does not satisfy an executable-line grep"
fi
printf '#!/bin/sh\n[ -L "$dir" ] && exit 1\n' > "$CTRL"
if grep -qE '^[^#]*\[ -L "\$dir" \]' "$CTRL"; then
    ok "forward control: a real executable line does satisfy it"
else
    bad "forward control: a real executable line does satisfy it" \
        "it did not — the pattern matches nothing, so the check is vacuous"
fi

# AGENTS.md boot-path rule 1, for this file specifically. test-boot-v2.sh scans
# every shipped helper for the same thing, so this is belt and braces — but it
# is the check that would fire first if a "recovery boot entry" were ever
# scripted here, which is precisely the thing §19 tempts an implementer into.
if grep -nE '^[^#]*\b(bootctl[[:space:]]+(install|update)|bootupctl|grub2-install|grub2-mkconfig|efibootmgr[[:space:]]+-[cBo])' "$ENGINE" >/dev/null 2>&1; then
    bad "the disposable engine touches no boot path" "it runs a boot-path command"
    DANGER_FREE=0
fi
[ "$DANGER_FREE" = 1 ] && ok "the disposable engine touches no boot path"

# ═════════════════════════════════════════════════════════════════════════════
sec "the copy boundary is default-deny at both ends"
resetcalls
out=$(disp plan)
has "plan names the host mount that is NOT isolated" "/run/host" "$out"
has "plan refuses to call itself a security boundary" "not a" "$out"
has "…and says which mechanism is one" "apex agent" "$out"
has "nothing is copied in by default" "(nothing)" "$out"
has "nothing leaves by default" "nothing. Without --copy-out" "$out"
has "plan says what teardown deletes" "DELETED WHEN IT CLOSES" "$out"
if [ ! -s "$CALLS" ]; then
    ok "plan creates nothing: the capsule engine is never called"
else
    bad "plan creates nothing" "$(cat "$CALLS")"
fi
if [ -z "$(ls -A "$DISP_ROOT")" ]; then
    ok "plan writes nothing under the disposable root"
else
    bad "plan writes nothing under the disposable root" "$(ls -A "$DISP_ROOT")"
fi

# plan and run must describe the SAME environment, or the boundary report is
# describing something other than what happens.
mkdir -p "$WORK/src"; printf 'payload\n' > "$WORK/src/file.txt"
planout=$(disp plan --name twins --copy-in "$WORK/src/file.txt" --copy-out "$WORK/results")
runout=$(disp run --name twins --copy-in "$WORK/src/file.txt" --copy-out "$WORK/results" -- true)
# Compare only the boundary block, which both print verbatim.
pb=$(sed -n '/^disposable environment/,/^  including/p' <<<"$planout")
rb=$(sed -n '/^disposable environment/,/^  including/p' <<<"$runout")
if [ -n "$pb" ] && [ "$pb" = "$rb" ]; then
    ok "plan's boundary report is byte-identical to run's"
else
    bad "plan's boundary report is byte-identical to run's" "they differ"
fi

sec "copy-in is a copy, so the host original cannot be changed from inside"
resetcalls
printf 'original\n' > "$WORK/src/file.txt"
disp run --name copies --copy-in "$WORK/src/file.txt" -- true >/dev/null
is "the host original is untouched by a run" "original" "$(cat "$WORK/src/file.txt")"
out=$(disp plan --copy-in "$WORK/src")
has "a directory can be copied in too" "-> ~/in/src" "$out"

# Refusals, each asserted for the REASON under test rather than for a non-zero
# exit: "it refused" has been true in this repository while "it refused for the
# reason under test" was false.
out=$(disp plan --copy-in "relative/path"); rc=$?
is "a relative --copy-in exits non-zero" "1" "$rc"
has "…and says it needs an absolute path" "absolute path" "$out"

out=$(disp plan --copy-in "$WORK/does-not-exist"); rc=$?
is "a --copy-in that does not exist exits non-zero" "1" "$rc"
has "…and says the path must exist" "exists" "$out"

out=$(disp plan --copy-out "relative"); rc=$?
is "a relative --copy-out exits non-zero" "1" "$rc"
out=$(disp plan --copy-out "$DISP_ROOT/inside"); rc=$?
is "a --copy-out inside the disposable root exits non-zero" "1" "$rc"
has "…and says why: teardown would delete it" "deleted at teardown" "$out"

out=$(disp plan --git "git@github.com:o/r"); rc=$?
is "an ssh git remote exits non-zero" "1" "$rc"
has "…and says https is required" "https://" "$out"
out=$(disp plan --git "https://example.com/a b"); rc=$?
is "a git URL with whitespace exits non-zero" "1" "$rc"

sec "copy-out"
resetcalls
rm -rf "$WORK/results"
STUB_PRODUCES=result.txt disp run --name outing --copy-out "$WORK/results" -- true >/dev/null
is "what the environment wrote to ~/out reaches the destination" \
    "produced" "$(cat "$WORK/results/result.txt" 2>/dev/null)"
# A second run must not silently overwrite it.
printf 'mine\n' > "$WORK/results/result.txt"
out=$(STUB_PRODUCES=result.txt disp run --name outing2 --copy-out "$WORK/results" -- true)
is "an existing file at the destination is NOT overwritten" "mine" \
    "$(cat "$WORK/results/result.txt")"
has "…and the refusal names the collision" "refusing to overwrite" "$out"
has "…and says nothing was copied, not that some was" "nothing was copied out" "$out"
# --force is the explicit opt-in.
STUB_PRODUCES=result.txt disp run --name outing3 --copy-out "$WORK/results" --force -- true >/dev/null
is "--force overwrites, and only then" "produced" "$(cat "$WORK/results/result.txt")"

sec "delete the entire environment when closed"
resetcalls
disp run --name gone --copy-in "$WORK/src/file.txt" -- true >/dev/null
if [ ! -e "$DISP_ROOT/disp-gone" ]; then
    ok "the environment's whole directory is gone after the run"
else
    bad "the environment's whole directory is gone after the run" \
        "$(ls -A "$DISP_ROOT/disp-gone")"
fi
has "…and the container was removed through the capsule engine" \
    "<rm> <disp-gone>" "$(cat "$CALLS")"
# The teardown must happen even when the thing inside fails: an environment
# that leaks on failure is one that accumulates exactly the runs people abandon.
resetcalls
STUB_EXIT=7 disp run --name failing -- false >/dev/null
if [ ! -e "$DISP_ROOT/disp-failing" ]; then
    ok "teardown happens even when the command inside fails"
else
    bad "teardown happens even when the command inside fails" "directory survived"
fi
# And the exit status propagates, so `apex disposable run -- make test` is
# usable as a check.
STUB_EXIT=7 "$ENGINE" run --name status -- false >/dev/null 2>&1
is "the inner command's exit status becomes the run's" "7" "$?"
# Creation failure must not leave a directory behind either.
resetcalls
STUB_CREATE_EXIT=1 disp run --name cfail -- true >/dev/null
if [ ! -e "$DISP_ROOT/disp-cfail" ]; then
    ok "a failed creation still tears the directory down"
else
    bad "a failed creation still tears the directory down" "directory survived"
fi

sec "teardown cannot remove anything but a disposable environment"
# THE assertions this file exists for. Each one must HARD-EXIT (2) with nothing
# removed, and the thing it could have removed is checked afterwards.
ln -sfn "$CANARY/precious" "$DISP_ROOT/disp-evil"
out=$(disp rm disp-evil); rc=$?
is "a symlinked environment directory exits 2, not 1" "2" "$rc"
has "…and refuses for the symlink reason specifically" "is a symlink" "$out"
has "…and says nothing was removed" "REFUSING TO REMOVE ANYTHING" "$out"
if [ -f "$CANARY/precious/data" ]; then
    ok "…and what the symlink pointed at is untouched"
else
    bad "…and what the symlink pointed at is untouched" "THE CANARY IS GONE"
fi
rm -f "$DISP_ROOT/disp-evil"

for bogus in "../../../etc" "x/../.." "disp-../evil" "disp-UPPER" "" "." ".." \
             "disp-$(printf 'a%.0s' {1..40})"; do
    out=$("$ENGINE" rm "$bogus" 2>&1); rc=$?
    if [ "$rc" -ne 0 ] && grep -q "not a disposable environment name" <<<"$out"; then
        ok "a name like '${bogus:0:24}' is refused as a name, before any path is built"
    else
        bad "a name like '${bogus:0:24}' is refused" "rc=$rc out=$(head -1 <<<"$out")"
    fi
done

# A symlinked ANCESTOR: the final component is a real directory, so the -L
# check does not fire and the realpath equality is what has to catch it.
mkdir -p "$WORK/elsewhere/disp-ancestor"
printf 'keep\n' > "$WORK/elsewhere/disp-ancestor/data"
ALT_ROOT="$WORK/alt-root"
ln -sfn "$WORK/elsewhere" "$ALT_ROOT"
out=$(APEX_DISPOSABLE_ROOT="$WORK/alt-root" "$ENGINE" rm disp-ancestor 2>&1); rc=$?
# A symlinked ROOT is legitimate — both sides resolve — so this must SUCCEED
# and remove it. The assertion is that it resolved rather than concatenated.
if [ "$rc" -eq 0 ] && [ ! -e "$WORK/elsewhere/disp-ancestor" ]; then
    ok "a symlinked disposable ROOT is resolved and its own environment removed"
else
    bad "a symlinked disposable ROOT is resolved" "rc=$rc out=$(head -2 <<<"$out")"
fi

sec "list and purge"
resetcalls
mkdir -p "$DISP_ROOT/disp-leaked/home" "$DISP_ROOT/not-ours"
printf 'keep\n' > "$DISP_ROOT/not-ours/data"
out=$(disp list)
has "a leaked environment is listed" "disp-leaked" "$out"
has "…and named as something purge will remove" "purge" "$out"
has "a directory that is not ours is reported as untouchable" "not-ours" "$out"
out=$(disp list --json)
if python3 -c "
import json,sys
d=json.loads(sys.argv[1])
names=[e['name'] for e in d]
assert 'disp-leaked' in names, names
assert 'not-ours' not in names, 'a stranger appeared in the machine-readable list'
" "$out" 2>/dev/null; then
    ok "list --json carries ours and never a stranger"
else
    bad "list --json carries ours and never a stranger" "$out"
fi
out=$(disp purge)
if [ ! -e "$DISP_ROOT/disp-leaked" ]; then
    ok "purge removes a leaked environment"
else
    bad "purge removes a leaked environment" "it survived"
fi
if [ -f "$DISP_ROOT/not-ours/data" ]; then
    ok "purge leaves a directory that is not a disposable environment alone"
else
    bad "purge leaves a directory that is not ours alone" "IT WAS DELETED"
fi
has "…and says so rather than silently skipping it" "left alone" "$out"
rm -rf "$DISP_ROOT/not-ours"

sec "the capsule argv"
resetcalls
disp run --name argv --image docker.io/library/ubuntu:24.04 -- true >/dev/null
argv=$(grep '<create>' "$CALLS")
has "create names the environment" "<disp-argv>" "$argv"
has "create passes the requested image" "<--image=docker.io/library/ubuntu:24.04>" "$argv"
has "create gives it its OWN home, which is what makes it disposable" \
    "<--home=$DISP_ROOT/disp-argv/home>" "$argv"
has "create gives it no device access" "<--gpu=none>" "$argv"
hasnt "a disposable capsule is never given a GPU" "--gpu=nvidia" "$argv"

resetcalls
disp run --name gitrun --git https://example.com/o/r.git -- true >/dev/null
gitcall=$(grep 'clone' "$CALLS")
has "the clone runs INSIDE the environment, not on the host" "<exec> <disp-gitrun>" "$gitcall"
has "…into ~/in" "<$DISP_ROOT/disp-gitrun/home/in>" "$gitcall"
has "…with -- before the URL, so a URL can never be read as an option" \
    "<--> <https://example.com/o/r.git>" "$gitcall"

sec "no authentication prompt, no external command"
if [ ! -s "$SPAWN_LOG" ]; then
    ok "no bootctl, efibootmgr, ostree, bootc, podman, pkexec, busctl or sudo ran"
else
    bad "no external command ran" "$(sort -u "$SPAWN_LOG" | tr '\n' '|')"
fi

# ═════════════════════════════════════════════════════════════════════════════
# The binary half. Everything above needed no toolchain and runs in `static`.
# ═════════════════════════════════════════════════════════════════════════════
if [ "$WITH_BINARY" = 1 ]; then

APEX_BIN=${APEX_BIN:-$REPO/apexd/target/debug/apex}
if [ ! -x "$APEX_BIN" ]; then
    echo "building the apex binary (not found at $APEX_BIN)…"
    ( cd "$REPO/apexd" && cargo build --locked --bin apex ) || {
        echo "FATAL: could not build the apex binary" >&2; exit 2; }
fi
[ -x "$APEX_BIN" ] || { echo "FATAL: no apex binary at $APEX_BIN" >&2; exit 2; }
apex() { "$APEX_BIN" "$@" 2>&1; }

# ── fixtures ────────────────────────────────────────────────────────────────
# Whole machines, presented as trees. The interesting states — no rollback
# target, /usr mounted read-write, an extension built for the previous release,
# no GPU driver — are ones a healthy machine does not have, so reasoning about
# them is the alternative and it is a much worse one.
mkfixture() {
    local r=$1 kind=$2
    mkdir -p "$r/proc" "$r/run" "$r/etc" "$r/usr/share/apex-shell" \
             "$r/sys/firmware/efi/efivars" "$r/sys/bus/pci/devices/0000:03:00.0" \
             "$r/ostree/deploy/apex/deploy" "$r/usr/lib/systemd/system" \
             "$r/usr/libexec" "$r/var/lib/apex/pkg" "$r/proc/net"
    local csum=8f14e45fceea167a5a36dedd4bea2543f14e45fceea167a5a36dedd4bea25431
    printf 'BOOT_IMAGE=/vmlinuz root=UUID=x ostree=/ostree/boot.1/apex/%s/0 rw quiet\n' \
        "$csum" > "$r/proc/cmdline"
    printf 'NAME="APEX-OS"\nVERSION_ID=43\nVARIANT_ID=gaming\n' > "$r/etc/os-release"
    printf 'shell\n' > "$r/usr/share/apex-shell/shell.qml"
    : > "$r/usr/lib/systemd/system/rescue.target"
    printf '#!/bin/sh\nexit 0\n' > "$r/usr/libexec/apex-shell-firstrun"
    chmod +x "$r/usr/libexec/apex-shell-firstrun"
    printf '0x030000\n' > "$r/sys/bus/pci/devices/0000:03:00.0/class"
    printf '0x1002\n'   > "$r/sys/bus/pci/devices/0000:03:00.0/vendor"
    printf '0x1636\n'   > "$r/sys/bus/pci/devices/0000:03:00.0/device"
    # 4 attribute bytes then the payload, as efivarfs presents it.
    printf '\x06\x00\x00\x00\x01' \
        > "$r/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
    if [ "$kind" = healthy ]; then
        : > "$r/run/ostree-booted"
        mkdir -p "$r/ostree/deploy/apex/deploy/${csum}.0" \
                 "$r/ostree/deploy/apex/deploy/aaaa.0"
        printf 'overlay / overlay ro,relatime 0 0\nnone /usr overlay ro,relatime 0 0\n' \
            > "$r/proc/mounts"
        printf 'amdgpu 1 0 - Live 0x0\ndrm 1 0 - Live 0x0\n' > "$r/proc/modules"
        printf 'Iface\tDestination\tGateway\nwlan0\t00000000\t0101A8C0\t0003\n' \
            > "$r/proc/net/route"
    else
        : > "$r/run/ostree-booted"
        mkdir -p "$r/ostree/deploy/apex/deploy/${csum}.0"
        printf 'overlay / overlay ro 0 0\nnone /usr overlay rw,relatime 0 0\n' \
            > "$r/proc/mounts"
        printf 'drm 1 0 - Live 0x0\n' > "$r/proc/modules"
        printf 'Iface\tDestination\tGateway\n' > "$r/proc/net/route"
        printf '\x06\x00\x00\x00\x00' \
            > "$r/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
        printf '{"os_version_id":"42","resolved":["a","b","c"],"requested":["a"],"unsigned_accepted":[]}\n' \
            > "$r/var/lib/apex/pkg/state.json"
    fi
}
HEALTHY="$WORK/fx-healthy"; mkfixture "$HEALTHY" healthy
DEGRADED="$WORK/fx-degraded"; mkfixture "$DEGRADED" degraded

sec "apex recover status — a healthy machine"
mkdir -p "$FAKEHOME/.config/apex-shell"
out=$(APEX_RECOVER_ROOT="$HEALTHY" apex recover status); rc=$?
is "a healthy machine exits 0, so it is usable as a check" "0" "$rc"
for row in "Current deployment" "Previous deployment" "Secure Boot" "Filesystem" \
           "GPU driver" "APEX Shell" "Network" "Package extensions"; do
    has "the surface carries §19's row '$row'" "$row" "$out"
done
has "the booted deployment is named by its ostree checksum" "8f14e45fceea" "$out"
has "a rollback target is reported as available" "2 deployments present" "$out"
has "…and names the command a button runs" "sudo apex rollback" "$out"
has "/usr read-only is reported as verified" "read-only" "$out"
has "the GPU's loaded module is named" "amdgpu" "$out"
has "§19's four actions are listed" "Repair automatically" "$out"
has "…including hardware diagnostics" "apex doctor" "$out"
has "…and the factory reset, as a dry run" "apex recover reset" "$out"
has "recovery routes are reported" "Recovery routes" "$out"

sec "apex recover status — the states a healthy machine does not have"
out=$(APEX_RECOVER_ROOT="$DEGRADED" apex recover status); rc=$?
is "a machine needing attention exits non-zero" "1" "$rc"
has "one deployment means nothing to roll back to" "nothing to roll back to" "$out"
has "a writable /usr is reported as drift" "READ-WRITE" "$out"
has "Secure Boot disabled is reported as attention, not as fine" "disabled" "$out"
has "a missing GPU module is named" "no kernel module loaded" "$out"
has "no default route is reported" "no default route" "$out"
has "an extension built for another release names the rebuild" "pkg rebuild" "$out"
has "…and says which release it was built for" "built for OS 42" "$out"

sec "apex recover status --json"
out=$(APEX_RECOVER_ROOT="$HEALTHY" apex recover status --json)
if python3 -c "
import json,sys
d=json.loads(sys.argv[1])
want=['current-deployment','previous-deployment','secure-boot','filesystem',
      'gpu-driver','apex-shell','network','package-extensions']
got=[r['id'] for r in d['rows']]
assert got == want, f'{got} != {want}'
for r in d['rows']:
    assert r['state'] in ('verified','available','attention','unavailable'), r
    assert r['detail'], r
assert d['bootloader'] == 'grub', d['bootloader']
assert any(a['id']=='bootPrevious' for a in d['actions'])
assert any(a['id']=='factoryReset' for a in d['actions'])
ids=[r['id'] for r in d['routes']]
assert 'recovery-boot-entry' in ids
entry=[r for r in d['routes'] if r['id']=='recovery-boot-entry'][0]
assert entry['available'] is False, 'APEX ships no recovery boot entry and must say so'
media=[r for r in d['routes'] if r['id']=='installer-media'][0]
assert media['available'] is None, 'unknowable must be null, never false'
assert [s['id'] for s in d['resetScopes']] == ['desktop','user']
" "$out" 2>&1; then
    ok "status --json carries all eight rows, the actions and the routes"
else
    bad "status --json carries all eight rows, the actions and the routes" \
        "$(python3 -c "
import json,sys
try: json.loads(sys.argv[1])
except Exception as e: print(e)
" "$out")"
fi
# The route conditionality that matters: on GRUB the rescue route exists.
if python3 -c "
import json,sys
d=json.loads(sys.argv[1])
r=[x for x in d['routes'] if x['id']=='rescue-target'][0]
assert r['available'] is True, r
assert 'grub' in r['how'], r['how']
" "$out" 2>/dev/null; then
    ok "on GRUB, editing the command line at the menu is offered as a route"
else
    bad "on GRUB the rescue route is offered" "$out"
fi
# And on a UKI it must NOT be, because a UKI's command line is inside the
# signed image. A uniform claim would be a false one on exactly the machines
# that are hardest to get into.
UKI="$WORK/fx-uki"; mkfixture "$UKI" healthy
G=4a67b082-0a4c-41cf-b6c7-440b29bb8c4f
printf '\x06\x00\x00\x00s\x00y\x00s\x00t\x00e\x00m\x00d\x00-\x00b\x00o\x00o\x00t\x00' \
    > "$UKI/sys/firmware/efi/efivars/LoaderInfo-$G"
printf '\x06\x00\x00\x00s\x00d\x00-\x00s\x00t\x00u\x00b\x00' \
    > "$UKI/sys/firmware/efi/efivars/StubInfo-$G"
out=$(APEX_RECOVER_ROOT="$UKI" apex recover status --json)
if python3 -c "
import json,sys
d=json.loads(sys.argv[1])
assert d['bootloader']=='systemd-boot', d['bootloader']
r=[x for x in d['routes'] if x['id']=='rescue-target'][0]
assert r['available'] is False, r
assert 'Unified Kernel Image' in r['how'], r['how']
" "$out" 2>/dev/null; then
    ok "on a signed UKI, the rescue route is reported ABSENT with the reason"
else
    bad "on a signed UKI the rescue route is absent" "$out"
fi

sec "apex doctor --json"
out=$(apex doctor --json)
if python3 -c "
import json,sys
d=json.loads(sys.argv[1])
assert isinstance(d['checks'], list) and d['checks'], 'no checks'
assert d['passed'] + d['warned'] == d['total'], d
for c in d['checks']:
    assert isinstance(c['ok'], bool) and isinstance(c['check'], str) and c['check']
" "$out" 2>/dev/null; then
    ok "doctor --json is valid JSON with every check and the counts"
else
    bad "doctor --json is valid JSON" "$(head -c 300 <<<"$out")"
fi
text=$(apex doctor)
tcount=$(grep -c '^\[\(PASS\|WARN\)\]' <<<"$text")
jcount=$(python3 -c "import json,sys; print(len(json.loads(sys.argv[1])['checks']))" "$out")
is "the JSON and the text report exactly the same number of checks" "$tcount" "$jcount"

sec "apex recover repair"
out=$(APEX_RECOVER_ROOT="$HEALTHY" apex recover repair)
has "repair is a DRY RUN unless told otherwise" "DRY RUN" "$out"
has "…and says which privilege domain it converges" "privilege domain" "$out"
# The desktop is provisioned in $FAKEHOME, and the fixture's package state is
# absent, so a healthy machine has nothing to repair.
has "a healthy machine has nothing to repair" "Nothing to repair" "$out"
out=$(APEX_RECOVER_ROOT="$DEGRADED" apex recover repair)
has "a stale package extension is offered as a repair" "rebuild-package-extension" "$out"
has "…with the command it would run" "apex-pkg" "$out"
has "…and why it is safe to run unattended" "safe" "$out"
has "…and it is NOT run, because it belongs to the other domain" \
    "belong to the other privilege domain" "$out"
hasnt "repair never proposes a rollback" "apex rollback" "$out"
hasnt "repair never proposes a reset" "recover reset" "$out"
# An unprovisioned account is the user-domain repair.
NOSHELL="$WORK/home-noshell"; mkdir -p "$NOSHELL"
out=$(HOME="$NOSHELL" APEX_RECOVER_ROOT="$HEALTHY" apex recover repair)
has "an unprovisioned desktop is offered as a user-domain repair" \
    "reprovision-desktop" "$out"

sec "apex recover reset — the dry run is the default"
RH="$WORK/home-reset"
seed_reset_home() {
    rm -rf "$RH"
    mkdir -p "$RH/.config/apex-shell/plugins/keep" "$RH/.config/apex" \
             "$RH/.config/hypr" "$RH/.cache/apex-shell" \
             "$RH/.local/state/apex" "$RH/.local/share/apex/env" "$RH/.ssh" \
             "$RH/Documents"
    printf '{"a":1}\n'   > "$RH/.config/apex-shell/input.json"
    printf '{"b":2}\n'   > "$RH/.config/apex-shell/display.json"
    printf 'plugin\n'    > "$RH/.config/apex-shell/plugins/keep/manifest.json"
    printf 'bp\n'        > "$RH/.config/apex/blueprint.toml"
    printf 'games\n'     > "$RH/.config/apex/games.toml"
    printf 'main\n'      > "$RH/.config/hypr/hyprland.conf"
    printf 'idle\n'      > "$RH/.config/hypr/hypridle.conf"
    printf 'generated\n' > "$RH/.config/hypr/apex-input.conf"
    printf 'generated\n' > "$RH/.config/hypr/apex-display.conf"
    printf 'cached\n'    > "$RH/.cache/apex-shell/colors.json"
    printf 'state\n'     > "$RH/.local/state/apex/blueprint-state.toml"
    printf 'capsule\n'   > "$RH/.local/share/apex/env/fedora.json"
    printf 'KEY\n'       > "$RH/.ssh/id_ed25519"
    printf 'thesis\n'    > "$RH/Documents/thesis.txt"
}
seed_reset_home
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop); rc=$?
is "a dry run exits 0" "0" "$rc"
has "…and says it is one" "DRY RUN. Nothing has been changed." "$out"
has "…and enumerates what would be removed" "WILL BE REMOVED" "$out"
has "…and what would be emptied rather than removed" "WILL BE EMPTIED" "$out"
has "…and what is preserved" "PRESERVED:" "$out"
has "…naming credentials explicitly" ".ssh" "$out"
has "…and the exact command that performs it" "--commit --confirm desktop:" "$out"
if [ -f "$RH/.config/apex-shell/input.json" ] && [ -f "$RH/.cache/apex-shell/colors.json" ]; then
    ok "a dry run removes nothing at all"
else
    bad "a dry run removes nothing at all" "SOMETHING WAS DELETED"
fi

sec "apex recover reset — every way it must refuse"
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope system); rc=$?
is "an unknown scope exits 2" "2" "$rc"
has "…and names the scopes that exist" "desktop" "$out"

out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop --commit); rc=$?
is "--commit with no --confirm exits 2" "2" "$rc"
has "…for the reason under test, not merely non-zero" "--commit needs --confirm" "$out"
is "…and deletes nothing" "{\"a\":1}" "$(cat "$RH/.config/apex-shell/input.json")"

out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" \
      apex recover reset --scope desktop --commit --confirm "desktop:99:deadbeef"); rc=$?
is "a wrong --confirm exits 2" "2" "$rc"
has "…names both the token given and the token expected" "expected :" "$out"
has "…and says nothing was changed" "Nothing has been changed" "$out"
is "…and deletes nothing" "{\"a\":1}" "$(cat "$RH/.config/apex-shell/input.json")"

# The token must be bound to the PLAN, not to the scope: a token read from one
# machine state must be refused after that state changes.
tok=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop \
      | grep -o 'desktop:[0-9]*:[0-9a-f]*' | tail -1)
if [ -n "$tok" ]; then ok "the dry run prints a plan-bound token"
else bad "the dry run prints a plan-bound token" "none found"; fi
printf 'new\n' > "$RH/.config/apex-shell/ApexShellKeybinds.conf"
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" \
      apex recover reset --scope desktop --commit --confirm "$tok"); rc=$?
is "a token from before the machine changed is refused" "2" "$rc"
has "…because the plan differs, and it says so" "machine changed" "$out"
is "…and deletes nothing" "{\"a\":1}" "$(cat "$RH/.config/apex-shell/input.json")"

# Running as root is refused. Not run as root here — this suite never
# escalates — so the refusal is asserted through the shipped message instead,
# and the build asserts the behaviour itself (Containerfile.base runs it as
# root and checks both the status and the reason).
if grep -q "must not run as root" "$REPO/apexd/apex/src/recover.rs"; then
    ok "reset refuses root, and the message says why"
else
    bad "reset refuses root" "no such refusal in the source"
fi

sec "apex recover reset --commit, against a fake home"
seed_reset_home
tok=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop \
      | grep -o 'desktop:[0-9]*:[0-9a-f]*' | tail -1)
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" \
      apex recover reset --scope desktop --commit --confirm "$tok"); rc=$?
is "a matching confirmation performs it" "0" "$rc"
if [ ! -f "$RH/.config/apex-shell/input.json" ]; then
    ok "the settings it named are gone"
else bad "the settings it named are gone" "input.json survived"; fi
if [ ! -d "$RH/.cache/apex-shell" ]; then
    ok "the cache directory it named is gone"
else bad "the cache directory is gone" "it survived"; fi
# The heart of the data boundary. Each of these is a promise the plan printed.
for keep in ".ssh/id_ed25519" "Documents/thesis.txt" ".config/apex/blueprint.toml" \
            ".config/apex/games.toml" ".config/hypr/hyprland.conf" \
            ".config/hypr/hypridle.conf" ".config/apex-shell/plugins/keep/manifest.json" \
            ".local/share/apex/env/fedora.json" ".local/state/apex/blueprint-state.toml"; do
    if [ -e "$RH/$keep" ]; then ok "desktop scope preserved ~/$keep"
    else bad "desktop scope preserved ~/$keep" "IT WAS DELETED"; fi
done
# Truncated, never removed: hyprland.conf sources these and a missing source is
# a fatal config error, so a delete here takes the whole session's config down.
for gen in ".config/hypr/apex-input.conf" ".config/hypr/apex-display.conf"; do
    if [ -f "$RH/$gen" ] && [ ! -s "$RH/$gen" ]; then
        ok "the generated $gen was emptied, not removed"
    else
        bad "the generated $gen was emptied, not removed" \
            "exists=$([ -f "$RH/$gen" ] && echo y || echo n) size=$(stat -c %s "$RH/$gen" 2>/dev/null)"
    fi
done
has "the run says what it preserved, re-checked after the fact" "preserved" "$out"
# The backup is what makes a mistaken reset recoverable.
bdir=$(find "$RH" -maxdepth 1 -name 'apex-reset-backup-*' -type d | head -1)
if [ -n "$bdir" ] && [ -f "$bdir/.config/apex-shell/input.json" ]; then
    ok "what was removed was copied to a backup directory first"
else
    bad "what was removed was copied to a backup first" "no backup at $bdir"
fi
if [ -n "$bdir" ] && [ ! -e "$bdir/.cache/apex-shell" ]; then
    ok "…except the cache, which is regenerable and can be large"
else
    bad "the cache is not backed up" "it was"
fi

sec "apex recover reset --scope user"
seed_reset_home
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope user)
has "user scope names the blueprint as a loss" "blueprint.toml" "$out"
has "…and says how to keep it first" "apex sync export" "$out"
has "…and still preserves credentials" ".ssh" "$out"
has "…and still preserves capsule records" "apex env rm" "$out"
tok=$(grep -o 'user:[0-9]*:[0-9a-f]*' <<<"$out" | tail -1)
HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" \
    apex recover reset --scope user --commit --confirm "$tok" >/dev/null
if [ ! -f "$RH/.config/apex/blueprint.toml" ]; then
    ok "user scope really does remove the blueprint"
else bad "user scope removes the blueprint" "it survived"; fi
for keep in ".ssh/id_ed25519" "Documents/thesis.txt" ".config/hypr/hyprland.conf" \
            ".local/share/apex/env/fedora.json" \
            ".config/apex-shell/plugins/keep/manifest.json"; do
    if [ -e "$RH/$keep" ]; then ok "user scope still preserved ~/$keep"
    else bad "user scope still preserved ~/$keep" "IT WAS DELETED"; fi
done

sec "apex recover reset — the guards on \$HOME itself"
out=$(HOME=/ APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop); rc=$?
is "HOME=/ is refused" "1" "$rc"
has "…for the reason under test" "shared parent" "$out"
out=$(HOME="$WORK/no-such-home" APEX_RECOVER_ROOT="$HEALTHY" \
      apex recover reset --scope desktop); rc=$?
is "a HOME that does not exist is refused" "1" "$rc"
# `unset` in a subshell, NOT `env -u`: `apex` here is a shell function, and
# `env` cannot run one — it would resolve some other `apex` on PATH or fail
# with 127. The first version of this case did exactly that and reported "it
# refused" while the thing under test never ran. Both the status and the
# reason are asserted now.
out=$(unset HOME; APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop); rc=$?
is "an unset HOME is refused" "1" "$rc"
has "…for the reason under test" "\$HOME is unset" "$out"

sec "the reset refuses when it could not put back what it removes"
NOPROV="$WORK/fx-noprov"; mkfixture "$NOPROV" healthy
rm -f "$NOPROV/usr/libexec/apex-shell-firstrun"
seed_reset_home
tok=$(HOME="$RH" APEX_RECOVER_ROOT="$NOPROV" apex recover reset --scope desktop \
      | grep -o 'desktop:[0-9]*:[0-9a-f]*' | tail -1)
out=$(HOME="$RH" APEX_RECOVER_ROOT="$NOPROV" \
      apex recover reset --scope desktop --commit --confirm "$tok"); rc=$?
is "a missing provisioner refuses the whole reset" "1" "$rc"
has "…for the reason under test" "could not put back" "$out"
is "…and deletes nothing" "{\"a\":1}" "$(cat "$RH/.config/apex-shell/input.json")"
has "…and names the escape hatch for someone who wants only the deletion" \
    "--no-reprovision" "$out"

sec "a symlinked target is refused, and what it pointed at survives"
seed_reset_home
rm -rf "$RH/.cache/apex-shell"
ln -sfn "$CANARY/precious" "$RH/.cache/apex-shell"
tok=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" apex recover reset --scope desktop \
      | grep -o 'desktop:[0-9]*:[0-9a-f]*' | tail -1)
out=$(HOME="$RH" APEX_RECOVER_ROOT="$HEALTHY" \
      apex recover reset --scope desktop --commit --confirm "$tok"); rc=$?
is "a symlinked reset target refuses the whole reset" "1" "$rc"
has "…for the symlink reason specifically" "is a symlink" "$out"
has "…and says nothing was changed" "nothing has been changed" "$out"
if [ -f "$CANARY/precious/data" ]; then
    ok "…and what the symlink pointed at is untouched"
else
    bad "…and what the symlink pointed at is untouched" "THE CANARY IS GONE"
fi
is "…and the other targets are untouched too, because it is all-or-nothing" \
    "{\"a\":1}" "$(cat "$RH/.config/apex-shell/input.json")"

fi  # WITH_BINARY

# ═════════════════════════════════════════════════════════════════════════════
sec "the machine running the tests"
if [ ! -s "$SPAWN_LOG" ]; then
    ok "no external command was spawned by any case in this file"
else
    bad "no external command was spawned" "$(sort -u "$SPAWN_LOG" | tr '\n' '|')"
fi
now=$(LC_ALL=C find "$CANARY" -printf '%p %s\n' | sort)
if [ "$now" = "$CANARY_SUM" ]; then
    ok "the canaries outside every fixture are byte-identical"
else
    bad "the canaries outside every fixture are byte-identical" "THEY CHANGED"
fi
real_state_sum > "$WORK/real-state-after"
# Compared in the shell, not with diff(1). `diff` is diffutils and is NOT
# installed in this project's `apex-rust` container: it exited 127, `diff -q`
# reported "differ" for a missing tool rather than for a difference, and the
# check went red on two byte-identical empty files. A dependency that turns a
# clean run into a false alarm is worse than no check.
before_state=$(cat "$REAL_STATE_SUM")
after_state=$(cat "$WORK/real-state-after")
if [ "$before_state" = "$after_state" ]; then
    ok "the developer's own ~/.config/apex, apex-shell, hypr and state are untouched"
else
    bad "the developer's own configuration is untouched" \
        "IT CHANGED — before:$(head -c 120 <<<"$before_state" | tr '\n' '|') after:$(head -c 120 <<<"$after_state" | tr '\n' '|')"
fi

printf '\napex recover / disposable: %d passed, %d failed%s\n' \
    "$pass" "$fail" "$([ "$WITH_BINARY" = 1 ] && echo ' (with binary)' || echo ' (structural only)')"
[ "$fail" -eq 0 ] || exit 1

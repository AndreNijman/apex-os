#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-build-local-shell-ref.sh — build-local.sh pins the apex-shell branch
#  that MATCHES the apex-os branch being built, and says which one it used.
#
#  WHY. build-local.sh asked the remote for `refs/heads/main` unconditionally,
#  while build-image.yml and pr-validation.yml had both already been changed to
#  pin the matching branch — the third appearance of one defect. The cost is not
#  theoretical: apex-os roadmap/v2.2 + apex-shell main dies in
#  check-labwc-keybinds on W-A-s (screen reader) and W-A-v (voice), 157 steps
#  into Containerfile.base, roughly an hour in. A local build of the integration
#  branch could never pass.
#
#  Underneath it was a second defect. The stated property — "cannot reach the
#  remote is FATAL, never a silent fallback" — was unreachable code:
#
#      SHELL_REF="$(git ls-remote … 2>/dev/null | awk '{print $1}')"
#      [ -n "$SHELL_REF" ] || { echo "FATAL: …"; exit 1; }
#
#  under `set -euo pipefail` the assignment carries ls-remote's status (128)
#  through pipefail, errexit kills the script on that line, and `2>/dev/null`
#  has already discarded git's own message. Measured: exit 128, silent. The
#  FATAL could only ever print if the remote answered successfully AND had no
#  `main`, which does not happen. So the resolution is now a return-code
#  decision and this suite drives all three codes.
#
#  HOW IT RUNS build-local.sh WITHOUT BUILDING ANYTHING. build-local.sh resolves
#  the shell ref before it defines any build function and long before it calls
#  podman, so an UNKNOWN TARGET runs the whole resolution and then exits 2 at
#  dispatch without touching podman or sudo:
#
#      ./build-local.sh --allow-unsigned bogus-target
#
#  Every passing case therefore asserts BOTH the `== shell ==` lines on stdout
#  AND `unknown target: bogus-target` on stderr with rc 2. The second half is
#  the load-bearing one: it proves the run got all the way through resolution
#  into dispatch, rather than dying early and printing nothing — which is how a
#  check of this shape passes over nothing at all.
#
#  The remote is a LOCAL BARE REPOSITORY, through APEX_SHELL_REMOTE, which
#  exists in build-local.sh for this suite. Each fixture branch gets its own
#  commit, so the assertion is on the resolved SHA and not merely on the label
#  printed beside it — a message naming the right branch while pinning the
#  wrong sha would pass a label-only check.
#
#  Both directions, seven ways: the exact match wins; the roadmap/v2.2 rung is
#  taken on a task/* branch; the main rung is taken and ANNOUNCES ITSELF when
#  the remote lacks roadmap/v2.2; a detached HEAD is not asked for as a branch
#  name; an unreachable remote is a loud FATAL and not a fallback; a remote with
#  no usable branch at all is also fatal; and an explicit APEX_SHELL_REF is
#  honoured without consulting the remote at all — proved by pointing
#  APEX_SHELL_REMOTE at a path that does not exist and still succeeding.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
ROOT="$PWD"
SCRIPT="$ROOT/build-local.sh"

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }

[ -f "$SCRIPT" ] || { echo "FATAL: $SCRIPT does not exist"; exit 1; }
command -v git >/dev/null || { echo "FATAL: git is not installed"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/nokey"

export GIT_AUTHOR_NAME=apex-test GIT_AUTHOR_EMAIL=apex@test.invalid
export GIT_COMMITTER_NAME=apex-test GIT_COMMITTER_EMAIL=apex@test.invalid
unset APEX_SHELL_REF APEX_SHELL_REMOTE 2>/dev/null || true

# ── fixtures ────────────────────────────────────────────────────────────────

# A bare "apex-shell" carrying exactly the named branches, each on its own
# commit so the branches have distinct shas.
seed_remote() {  # $1 = bare path, $2.. = branch names
    local bare="$1"; shift
    local src b
    src="$WORK/src-$(basename "$bare")"
    git init -q "$src" || return 1
    ( cd "$src" && echo seed > f && git add f && git commit -qm seed ) >/dev/null 2>&1
    git init -q --bare "$bare" || return 1
    for b in "$@"; do
        ( cd "$src" && echo "$b" > f && git add f && git commit -qm "$b" ) >/dev/null 2>&1
        git -C "$src" push -q "$bare" "HEAD:refs/heads/$b" || return 1
    done
}

sha_of() {  # $1 = bare path, $2 = branch
    git ls-remote "$1" "refs/heads/$2" 2>/dev/null | awk '{print $1}'
}

# A checkout of the REAL build-local.sh, on a chosen branch (or detached).
seed_repo() {  # $1 = branch name, or DETACHED
    local d="$WORK/repo"
    rm -rf "$d"; mkdir -p "$d"
    cp "$SCRIPT" "$d/build-local.sh"
    chmod +x "$d/build-local.sh"
    git init -q "$d" || return 1
    git -C "$d" add build-local.sh
    git -C "$d" commit -qm fixture >/dev/null 2>&1
    if [ "$1" = DETACHED ]; then
        git -C "$d" checkout -q --detach HEAD
    else
        # The branch init happened to create may already be the wanted one.
        git -C "$d" checkout -q -B "$1"
    fi
    printf '%s\n' "$d"
}

out=""; err=""; rc=0
runbl() {  # $1 = repo dir, $2 = APEX_SHELL_REMOTE, $3 = APEX_SHELL_REF ("" for unset)
    local repo="$1" remote="$2" ref="${3:-}"
    rc=0
    if [ -n "$ref" ]; then
        out="$(APEX_SIGNING_DIR="$WORK/nokey" APEX_SHELL_REMOTE="$remote" \
               APEX_SHELL_REF="$ref" \
               "$repo/build-local.sh" --allow-unsigned bogus-target 2>"$WORK/err")" || rc=$?
    else
        out="$(APEX_SIGNING_DIR="$WORK/nokey" APEX_SHELL_REMOTE="$remote" \
               "$repo/build-local.sh" --allow-unsigned bogus-target 2>"$WORK/err")" || rc=$?
    fi
    err="$(cat "$WORK/err" 2>/dev/null)"
}

# Every non-fatal case makes the same three claims: the run reached dispatch,
# it pinned the expected sha, and it named the expected branch.
expect_pinned() {  # $1 label, $2 expected sha, $3 substring the message must contain
    local label="$1" want="$2" msg="$3"
    if [ "$rc" = 2 ] && grep -q 'unknown target: bogus-target' <<<"$err"; then
        ok "$label: reached dispatch without building (rc 2)"
    else
        bad "$label: reached dispatch without building (rc 2)" "rc=$rc err=${err:0:160}"
    fi
    if grep -qF "== shell == vendoring apex-shell $want" <<<"$out"; then
        ok "$label: pinned $want"
    else
        bad "$label: pinned $want" "$(grep '== shell ==' <<<"$out" | tr '\n' ' ')"
    fi
    if grep -qF "$msg" <<<"$out"; then
        ok "$label: said why — '$msg'"
    else
        bad "$label: said why — '$msg'" "$(grep '== shell ==' <<<"$out" | tr '\n' ' ')"
    fi
}

FULL="$WORK/full.git"       # main + roadmap/v2.2 + task/build-verify
NOTASK="$WORK/notask.git"   # main + roadmap/v2.2
NARROW="$WORK/narrow.git"   # main only
EMPTY="$WORK/empty.git"     # reachable, no branches at all
GONE="$WORK/there-is-no-remote-here.git"

seed_remote "$FULL"   main roadmap/v2.2 task/build-verify || { echo "FATAL: fixture remote failed"; exit 1; }
seed_remote "$NOTASK" main roadmap/v2.2                   || { echo "FATAL: fixture remote failed"; exit 1; }
seed_remote "$NARROW" main                                || { echo "FATAL: fixture remote failed"; exit 1; }
git init -q --bare "$EMPTY"                               || { echo "FATAL: fixture remote failed"; exit 1; }

# The fixtures must actually differ, or every case below would pass by accident.
if [ -n "$(sha_of "$FULL" roadmap/v2.2)" ] \
   && [ "$(sha_of "$FULL" roadmap/v2.2)" != "$(sha_of "$FULL" main)" ] \
   && [ -z "$(sha_of "$NARROW" roadmap/v2.2)" ]; then
    ok "fixtures: branches carry distinct shas and narrow.git really lacks roadmap/v2.2"
else
    bad "fixtures: branches carry distinct shas and narrow.git really lacks roadmap/v2.2"
fi

printf '\n── the matching branch wins ──────────────────────────────────\n'

# 1. apex-shell HAS task/build-verify -> the exact match is taken, not v2.2.
repo="$(seed_repo task/build-verify)"
runbl "$repo" "$FULL"
expect_pinned "exact match" "$(sha_of "$FULL" task/build-verify)" \
    "matches this apex-os branch"

# 2. apex-shell has no task/build-verify -> the roadmap/v2.2 rung. This is the
#    rung every worktree in this program actually takes, and the one whose
#    absence killed the build this suite exists for.
repo="$(seed_repo task/build-verify)"
runbl "$repo" "$NOTASK"
expect_pinned "task/* -> roadmap/v2.2" "$(sha_of "$NOTASK" roadmap/v2.2)" \
    "apex-shell has no branch named 'task/build-verify'"

# 3. Standing ON roadmap/v2.2, the exact match is that same branch — and it must
#    be reported as a match, not as a fallback.
repo="$(seed_repo roadmap/v2.2)"
runbl "$repo" "$NOTASK"
expect_pinned "on roadmap/v2.2" "$(sha_of "$NOTASK" roadmap/v2.2)" \
    "branch 'roadmap/v2.2', which matches this apex-os branch"

printf '\n── the main fallback still fires, and still announces itself ──\n'

# 4. The other direction. A remote with neither the branch nor roadmap/v2.2
#    must still land on main, and must SAY that it did — a silent fallback is
#    the original defect wearing a different hat.
repo="$(seed_repo task/build-verify)"
runbl "$repo" "$NARROW"
expect_pinned "main fallback" "$(sha_of "$NARROW" main)" \
    "apex-shell has no branch named 'task/build-verify'"
if grep -qF "branch 'main'" <<<"$out"; then
    ok "main fallback: names main specifically"
else
    bad "main fallback: names main specifically" "$(grep '== shell ==' <<<"$out" | tr '\n' ' ')"
fi

# 5. A detached HEAD prints the literal 'HEAD'. Asking the remote for
#    refs/heads/HEAD is meaningless; it must be skipped and said out loud.
repo="$(seed_repo DETACHED)"
runbl "$repo" "$NOTASK"
expect_pinned "detached HEAD" "$(sha_of "$NOTASK" roadmap/v2.2)" \
    '<detached HEAD>'

printf '\n── failure is loud ───────────────────────────────────────────\n'

# 6. Unreachable remote. This is the case the old code got wrong: it must exit
#    non-zero WITH A MESSAGE, and must not vendor anything.
repo="$(seed_repo task/build-verify)"
runbl "$repo" "$GONE"
[ "$rc" = 1 ] \
    && ok "unreachable remote: exits 1" \
    || bad "unreachable remote: exits 1" "rc=$rc"
grep -q 'cannot reach apex-shell' <<<"$err" \
    && ok "unreachable remote: says so on stderr" \
    || bad "unreachable remote: says so on stderr" "err=${err:0:200}"
grep -q 'vendoring apex-shell' <<<"$out" \
    && bad "unreachable remote: vendors nothing" "it printed a vendoring line anyway" \
    || ok "unreachable remote: vendors nothing"
grep -q 'unknown target' <<<"$err" \
    && bad "unreachable remote: stops before dispatch" "it reached the build dispatch" \
    || ok "unreachable remote: stops before dispatch"

# 7. Reachable remote with no branches at all — every rung answers "no such
#    branch". The terminal FATAL after the loop must be reachable too.
repo="$(seed_repo task/build-verify)"
runbl "$repo" "$EMPTY"
[ "$rc" = 1 ] && grep -q 'not even main' <<<"$err" \
    && ok "remote with no branches: fatal, and names what it tried" \
    || bad "remote with no branches: fatal, and names what it tried" "rc=$rc err=${err:0:200}"

printf '\n── APEX_SHELL_REF still wins, offline ────────────────────────\n'

# 8. An explicit ref must short-circuit the remote entirely. Proved rather than
#    asserted: the remote is a path that does not exist, so any lookup at all
#    would turn case 6's FATAL on.
repo="$(seed_repo task/build-verify)"
EXPLICIT=0123456789abcdef0123456789abcdef01234567
runbl "$repo" "$GONE" "$EXPLICIT"
expect_pinned "explicit APEX_SHELL_REF" "$EXPLICIT" \
    "the remote was not consulted"

printf '\n──────────────────────────────────────────────────────────────\n'
printf '  %d passed, %d failed\n' "$pass" "$fail"
printf '  Fixture remotes only; no network, no podman, no sudo. The real\n'
printf '  remote is exercised by build-image.yml and pr-validation.yml.\n'
printf '──────────────────────────────────────────────────────────────\n'
[ "$fail" -eq 0 ]

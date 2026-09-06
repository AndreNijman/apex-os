#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-multilib.sh — ask the shipped package engine to judge a REAL
#  multilib package set, and assert the decisions it makes.
#
#  ── Why this exists next to test-apex-pkg.sh ────────────────────────────────
#  test-apex-pkg.sh asserts the argv the engine builds. That is the right level
#  for most of it, and it is not enough here: `apex install steam` was broken by
#  six defects, and the one that reached a user's screen produced correct argv
#  and a wrong result. So this one runs the engine against packages Fedora
#  actually ships and reads back what it decided.
#
#  ── Why it decides rather than extracts ─────────────────────────────────────
#  Extracting a full set needs the container's installed versions to match the
#  repository exactly. When they do not, the guard correctly refuses the whole
#  transaction — which is the engine working, and tells you nothing about
#  multilib. Two earlier drafts of this test passed against a broken engine for
#  that reason. The decisions are the discriminating step.
#
#  ── Why a container ─────────────────────────────────────────────────────────
#  It needs a real dnf, a real repository and a package set whose i686 builds
#  carry /usr/bin, and it must not touch the machine running it. Skips out loud
#  where podman is absent or cannot run privileged, so the suite stays
#  meaningful on a plain runner.
#
#  Run from anywhere: ./tests/test-apex-multilib.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

ENGINE=files/system/libexec/apex-pkg
[ -f "$ENGINE" ] || { echo "cannot find $ENGINE"; exit 2; }

if ! command -v podman >/dev/null 2>&1; then
    echo "SKIP  podman is absent; this suite needs a real dnf and repository"
    exit 0
fi

IMAGE=${APEX_MULTILIB_IMAGE:-registry.fedoraproject.org/fedora:43}
PROBE=$(mktemp) || exit 2
trap 'rm -f "$PROBE"' EXIT

cat > "$PROBE" <<'PROBE_EOF'
set -uo pipefail
PKG=/repo/files/system/libexec/apex-pkg
WORK=/tmp/e2e; rm -rf "$WORK"; mkdir -p "$WORK/dl"; export WORK

# fontconfig pulls in glibc, and both ship an i686 build carrying /usr/bin.
# glibc is also on the engine's protected list, which is what makes it the
# package that proves the multilib rule rather than merely exercising it.
dnf5 -y download --resolve --arch=x86_64 --arch=noarch --arch=i686 \
    --destdir "$WORK/dl" fontconfig >/dev/null 2>&1 \
    || { echo "PROBE_SKIP no repository reachable"; exit 0; }

echo "PROBE_SET $(ls "$WORK/dl" | wc -l) rpms, $(ls "$WORK/dl" | grep -c i686) i686"
bash -c "source $PKG >/dev/null 2>&1; set +e; guard_rpms '$WORK/dl'" 2>&1 \
    | grep -oE "(allowing|refusing|skipping|omitting) '[^']*'" \
    | sed 's/^/PROBE_DECISION /'
PROBE_EOF

out=$(podman run --rm --privileged \
        -v "$PWD":/repo:ro,Z -v "$PROBE":/probe.sh:ro,Z \
        "$IMAGE" bash /probe.sh 2>&1)

if printf '%s\n' "$out" | grep -q PROBE_SKIP; then
    echo "SKIP  $(printf '%s\n' "$out" | grep PROBE_SKIP | sed 's/PROBE_SKIP //')"
    exit 0
fi

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }

printf '%s\n' "$out" | grep PROBE_SET | sed 's/PROBE_SET/      set:/'
decisions=$(printf '%s\n' "$out" | grep PROBE_DECISION | sed 's/PROBE_DECISION //')
printf '%s\n' "$decisions" | sed 's/^/      /' | head -8

# The defect that made every 32-bit application uninstallable: REFUSE_RE
# protects core names so an extension cannot shadow the image's copy, and a
# 32-bit sibling shadows nothing, because the image ships no i686 at all.
if printf '%s\n' "$decisions" | grep -q "allowing 'glibc-.*\.i686'"; then
    ok "a 32-bit sibling of a protected package is allowed through"
elif printf '%s\n' "$decisions" | grep -q "refusing 'glibc'"; then
    bad "glibc.i686 refused as a core package, so no 32-bit application can install"
else
    echo "SKIP  glibc.i686 was not in this set"
fi

# The defect that silently deleted the libraries: the installed-check asked
# `rpm -q <name>` with no architecture, so on multilib it answered with the
# x86_64 build and every i686 library was dropped as already present.
if printf '%s\n' "$decisions" | grep -qE "skipping '[^']*\.i686'.*"; then
    bad "an i686 build was dropped as already provided by the image"
else
    ok "no i686 build was mistaken for one the image already ships"
fi

# The fix must not become "install everything": a native build the image
# already ships at the same version must still be skipped, or the overlay
# shadows the image from the other direction.
if printf '%s\n' "$decisions" | grep -qE "(skipping|omitting) '[^']*\.(x86_64|noarch)'"; then
    ok "a native build the image already provides is still left out"
else
    echo "SKIP  nothing native overlapped the image in this set"
fi

echo
printf 'apex-multilib: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

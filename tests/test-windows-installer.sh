#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-windows-installer.sh — the Windows binary builds, and it RUNS.
#
#  WHY THIS EXISTS
#
#  `windows-installer/` is Windows-targeted code living in a repository that is
#  developed, tested and built entirely on Linux. Its first round shipped 655
#  lines of it and recorded honestly that "A Windows `.exe` has not been built
#  or tested; the Windows-specific reparse-point code has not been compiled in
#  this session." Code nobody compiles rots, and rots silently — the unit tests
#  all passed, because they were compiled for Linux.
#
#  So this asserts the two things those tests cannot:
#
#    1. the cross-build produces a real PE32+ x86-64 binary, and
#    2. that binary EXECUTES under Windows semantics and prints its own words.
#
#  (2) matters more than it looks. A binary that links and then dies in the
#  loader exits non-zero exactly like one that ran and refused, so "exit 1" on
#  its own proves nothing. The assertion is on the program's own message.
#
#  Wine is not a Windows machine and this suite does not pretend otherwise:
#  raw disk access, volume locking and UEFI variables do not work under it. It
#  proves the binary is well-formed and its argument handling reaches the user,
#  which is precisely the part that was previously unverifiable here.
#
#  Run from anywhere: ./tests/test-windows-installer.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

BUILDER=windows-installer/build-windows.sh
[ -x "$BUILDER" ] || { echo "cannot find $BUILDER"; exit 2; }

if ! command -v podman >/dev/null 2>&1; then
    echo "SKIP  podman is absent; the Windows cross-build needs a container"
    exit 0
fi

IMAGE="${APEX_WIN_BUILD_IMAGE:-registry.fedoraproject.org/fedora:43}"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail+1)); }

# ── 1. it cross-builds ───────────────────────────────────────────────────────
build_log="$OUT/build.log"
if "$BUILDER" "$OUT" >"$build_log" 2>&1; then
    ok "the Windows cross-build completed"
else
    rc=$?
    if [ "$rc" = 77 ]; then
        echo "SKIP  $(tail -1 "$build_log")"
        exit 0
    fi
    bad "the Windows cross-build failed (rc $rc)"
    tail -15 "$build_log" | sed 's/^/        /'
    printf '\napex-windows-installer: %d passed, %d failed\n' "$pass" "$fail"
    exit 1
fi

exe="$OUT/apex-windows-installer.exe"
if [ -s "$exe" ]; then
    ok "a binary was produced"
else
    bad "no binary at $exe"
fi

# ── 2. it is the right KIND of binary ────────────────────────────────────────
# An ELF here would mean the host target was used and every Windows-only path
# in the source was skipped by cfg() — the build would still be green.
kind="$(file -b "$exe" 2>/dev/null)"
case "$kind" in
    PE32+*x86-64*) ok "it is a PE32+ x86-64 Windows binary" ;;
    *)             bad "not a 64-bit Windows PE: $kind" ;;
esac

# ── 3. it RUNS, and the words are its own ────────────────────────────────────
# Wine's own failures (missing loader, bad image) also exit non-zero, so the
# exit code is not the assertion — the program's message is.
run_out="$OUT/run.txt"
podman run --rm -v "$OUT":/exe:ro,Z "$IMAGE" bash -c '
    dnf install -y -q --setopt=install_weak_deps=False wine >/dev/null 2>&1 \
        || { echo "APEX_WINE_UNAVAILABLE"; exit 0; }
    export WINEDEBUG=-all WINEPREFIX=/tmp/wineprefix
    wine /exe/apex-windows-installer.exe >/tmp/out 2>/tmp/err
    echo "APEX_RC=$?"
    echo "APEX_STDOUT_BEGIN"; cat /tmp/out
    echo "APEX_STDERR_BEGIN"; cat /tmp/err
' >"$run_out" 2>&1

if grep -q APEX_WINE_UNAVAILABLE "$run_out"; then
    echo "      wine unavailable in $IMAGE; execution not checked"
else
    rc_line="$(grep -m1 '^APEX_RC=' "$run_out" | cut -d= -f2)"
    if [ "$rc_line" = 1 ]; then
        ok "running it with no arguments exits 1"
    else
        bad "expected exit 1 with no arguments, got '${rc_line:-none}'"
    fi

    # The discriminator: our text, not wine's.
    if grep -q 'apex-windows-installer lab FILE.img' "$run_out"; then
        ok "the usage line the program itself prints reached the user"
    else
        bad "the program's own usage text never appeared — it may not have run at all"
        sed -n '1,12p' "$run_out" | sed 's/^/        /'
    fi

    # It must keep saying what it will not do.
    if grep -q 'installation and firmware changes are disabled' "$run_out"; then
        ok "it still declares that installation and firmware changes are disabled"
    else
        bad "the read-only disclaimer is gone from the binary's output"
    fi
fi

echo
printf 'apex-windows-installer: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-verbs.sh — every verb the CLI is supposed to have is in the binary.
#
#  ── The failure this exists for ─────────────────────────────────────────────
#  Two agents worked on `apexd/apex/src/main.rs` in parallel. One built its
#  commit from a copy of `main.rs` taken before the other's landed, so applying
#  it silently removed `mod task;` and the `Cmd::Task` arm.
#
#  Nothing failed to compile. Removing `mod task;` also stops
#  `apexd/apex/src/task.rs` from being compiled at all, so there was no orphaned
#  reference for rustc to complain about, no dead-code warning, and no test
#  failure — `apex task` simply was not in the binary any more. It was caught by
#  a person re-reading a diff, which is not a mechanism.
#
#  The whole class is invisible to the compiler: a verb's absence looks exactly
#  like a verb that was never written. Only asking the built artifact what it
#  can do will catch it.
#
#  ── Why an exact list and not a count ───────────────────────────────────────
#  A count passes while one verb is swapped for another. This repository has a
#  `-ge 30` assertion in its history that stayed green while 20 of 68 items were
#  silently dropped, so the list is enumerated and every entry checked by name.
#
#  Adding a verb means adding it here. That is the point: the list is the
#  statement of what APEX-OS offers, and changing it should be deliberate.
#
#  PASS = every verb below answers `--help` from the built binary.
#
#  Run from anywhere: ./tests/test-apex-verbs.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")" || exit 2
REPO=$(cd .. && pwd)

pass=0; fail=0
ok()  { printf 'PASS  %-46s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-46s %s\n' "$1" "$2"; fail=$((fail+1)); }

APEX_BIN=${APEX_BIN:-$REPO/apexd/target/debug/apex}
if [ ! -x "$APEX_BIN" ]; then
    echo "building the apex binary (not found at $APEX_BIN)…"
    ( cd "$REPO/apexd" && cargo build --locked --bin apex ) || {
        echo "FATAL: could not build the apex binary; nothing below can run" >&2
        exit 2
    }
fi
[ -x "$APEX_BIN" ] || { echo "FATAL: no apex binary at $APEX_BIN" >&2; exit 2; }

# ── every verb, by roadmap section ──────────────────────────────────────────
# Ordered by the section that asked for it, so a reader can trace a verb back to
# why it exists. `help` is clap's own and is deliberately not listed.
VERBS="
status tier profile battery fan game mode workload perf gaming
fingerprint pin rollback update shell metrics doctor changelog
install remove resolve search repo pkg env
agent project request secret
blueprint apply sync plugin
ai host build send open
task recover disposable boot
"

for v in $VERBS; do
    [ -n "$v" ] || continue
    # `--help` on a subcommand exits 0 and needs no privilege, no D-Bus and no
    # hardware, so this is a pure question about what the binary contains.
    if "$APEX_BIN" "$v" --help >/dev/null 2>&1; then
        ok "apex $v is in the binary"
    else
        bad "apex $v is in the binary" "not a recognised subcommand"
    fi
done

# ── the guard on the guard ──────────────────────────────────────────────────
# If the binary answered --help for anything at all, this file would pass while
# proving nothing. A verb that certainly does not exist must be rejected.
if "$APEX_BIN" definitely-not-a-verb --help >/dev/null 2>&1; then
    bad "a verb that does not exist is rejected" "the binary accepted a nonsense verb, so every check above is vacuous"
else
    ok "a verb that does not exist is rejected"
fi

# And the top-level help must list them, not merely accept them — a verb hidden
# from help is a verb nobody can find.
top="$("$APEX_BIN" --help 2>&1)"
missing_from_help=""
for v in $VERBS; do
    [ -n "$v" ] || continue
    grep -qE "^[[:space:]]+$v([[:space:]]|$)" <<<"$top" || missing_from_help="$missing_from_help $v"
done
if [ -z "$missing_from_help" ]; then
    ok "every verb is listed in apex --help"
else
    bad "every verb is listed in apex --help" "absent:$missing_from_help"
fi

printf '\napex verbs: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1

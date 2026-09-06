#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  check-doc-verbs.sh — every `apex …` command a doc shows, asked of the CLI
#  that ships.
#
#  ── Why ─────────────────────────────────────────────────────────────────────
#  `docs/agent-runtime.md` told the reader to run `apex request install clang`
#  and `apex project restore`. Neither parses: the verbs are
#  `apex request ask install` and `apex project layout restore`. Both had been
#  wrong long enough to be quoted into other work. A doc command that does not
#  parse is a user following instructions into an error, and prose review does
#  not catch it because the sentence around it reads correctly.
#
#  So this asks clap instead of trusting the prose. `--help` on a real
#  subcommand exits 0; on an unknown one it exits non-zero. Nothing is run.
#
#  ── The allow file, and why it is necessary rather than a cop-out ───────────
#  Good documentation names commands that do NOT exist, on purpose:
#
#    docs/boot-v2.md   "There is deliberately no `apex boot ack` verb"
#    docs/recovery.md  "Adding an `apex recover previous` verb would have been…"
#    docs/rollback.md  "An earlier version claimed `apex reset --keep-home`
#                       shipped in M3. It never did"
#
#  All three are the doc being careful, and a checker that flagged them would
#  train people to delete the sentence that explains the absence. This does not
#  parse English negation; it takes a list of deliberate mentions instead, one
#  per line, with a written reason. `#` starts a comment.
#
#  Historical records under docs/m*-notes.md, docs/m0-results.md and
#  docs/p*-progress.md are NOT checked by the CI wrapper: they record what was
#  true on a date, and rewriting them to match today's CLI would be falsifying
#  a log rather than fixing a doc.
#
#  Usage:  tests/check-doc-verbs.sh docs/agent-runtime.md [more.md …]
#          APEX=/path/to/apex tests/check-doc-verbs.sh docs/*.md
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

# The INSTALLED apex by default, which is the honest question for a shipped
# doc: does the command a reader will type work on the machine they are typing
# it on. It is the wrong question on a branch that adds a verb — the docs
# describe the new CLI and the old binary refuses it — so point APEX at the
# build:
#
#     APEX=target/debug/apex tests/check-doc-verbs.sh docs/*.md
#
# A false BAD from a verb this branch introduces is the tool working; a false
# PASS from silently accepting one would not be.
APEX=${APEX:-apex}
ALLOW=${ALLOW:-tests/doc-verbs-allow}

command -v "$APEX" >/dev/null 2>&1 || {
    echo "SKIP  no \`$APEX\` on PATH; this suite asks the shipped CLI"
    exit 0
}

allowed() {
    [ -f "$ALLOW" ] || return 1
    grep -v '^[[:space:]]*#' "$ALLOW" | grep -qxF "$1"
}

pass=0; fail=0; skip=0
for doc in "$@"; do
    [ -f "$doc" ] || { echo "no such file: $doc"; fail=$((fail+1)); continue; }
    mapfile -t cmds < <(
        grep -oE '(^|[`[:space:]])(sudo )?apex [a-z][a-z-]*( [a-z][a-z-]*)?' "$doc" \
            | sed -E 's/^[`[:space:]]+//; s/^sudo //' \
            | sort -u
    )
    for c in "${cmds[@]}"; do
        if $APEX ${c#apex } --help >/dev/null 2>&1; then
            pass=$((pass+1))
        elif allowed "$c"; then
            printf 'ALLOW %-34s %s\n' "$c" "$(basename "$doc")"
            skip=$((skip+1))
        else
            printf 'BAD   %-34s %s\n' "$c" "$(basename "$doc")"
            fail=$((fail+1))
        fi
    done
done

printf '\ndoc verbs: %d valid, %d deliberate, %d not a command\n' "$pass" "$skip" "$fail"
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  mutate-a11y-stack.sh — prove test-apex-a11y-stack.sh can go red.
#
#  The suite makes two claims that are easy to assert vacuously: a package is
#  installed, and a package is NOT started. The first is the M7 trap — the word
#  appears in the stanza's own prose, so a grep stays green with the package
#  deleted. The second is the N8 trap — "not found" is what a broken search
#  returns as well as a clean one.
#
#  Restores are `git checkout --`, never `cp -p` and never `mv`. The file set is
#  compared against HEAD after every mutate AND every restore.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

CORE="Containerfile.core"
AUTO="files/desktop/labwc/autostart"
SUITE_F="tests/test-apex-a11y-stack.sh"
FILES="$CORE $AUTO $SUITE_F"
SUITE="./tests/test-apex-a11y-stack.sh"

applied=0; noapply=0; caught=0; survived=0

tree_clean() { [ -z "$(git diff --name-only -- $FILES 2>/dev/null)" ]; }

restore() {
    git checkout -- $FILES 2>/dev/null
    if ! tree_clean; then
        echo "ABORT: tree still dirty after restore; verdicts would be meaningless" >&2
        git diff --stat -- $FILES >&2
        exit 3
    fi
}

run_suite() { "$SUITE" 2>&1; }

mutate() {   # mutate <id> <file> <from> <to> <assertion substring that must go red>
    local id="$1" file="$2" from="$3" to="$4" want="$5"

    tree_clean || { echo "ABORT: tree dirty BEFORE $id" >&2; exit 3; }
    if ! grep -qF -- "$from" "$file"; then
        printf '%-5s NO-APPLY  anchor absent in %s — this mutant proves nothing\n' "$id" "$file"
        noapply=$((noapply + 1)); return
    fi
    python3 - "$file" "$from" "$to" <<'EDIT'
import sys
p, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(p).read()
assert s.count(a) >= 1
open(p, 'w').write(s.replace(a, b, 1))
EDIT
    if tree_clean; then
        printf '%-5s NO-APPLY  the edit changed nothing\n' "$id"
        noapply=$((noapply + 1)); restore; return
    fi
    applied=$((applied + 1))

    local out; out="$(run_suite)"
    if printf '%s' "$out" | grep -q "^FAIL  .*$want"; then
        printf '%-5s CAUGHT    %s\n' "$id" "$want"
        caught=$((caught + 1))
    else
        printf '%-5s SURVIVED  %s\n' "$id" "$want"
        printf '%s\n' "$out" | grep -E '^(FAIL|SKIP|apex-a11y-stack)' | sed 's/^/      /'
        survived=$((survived + 1))
    fi
    restore
}

echo "── baseline: green, or nothing below means anything ──"
base="$(run_suite)"
printf '%s\n' "$base" | grep -E '^apex-a11y-stack'
if ! printf '%s' "$base" | grep -qE '^apex-a11y-stack: [0-9]+ passed, 0 failed'; then
    echo "ABORT: the suite is not green to begin with" >&2
    printf '%s\n' "$base" | grep -E '^(FAIL|SKIP)' >&2
    exit 3
fi

echo
echo "── the mutants ──"

# D1 — the package is removed and the paragraph explaining it is left behind.
#      This is the whole reason the suite parses instead of grepping: the word
#      `orca` is still in the file half a dozen times after this edit.
mutate D1 "$CORE" \
    '    dnf5 -y install orca; \' \
    '    dnf5 -y install; \' \
    "Containerfile.core installs orca"

# D2 — somebody switches the reader on for everybody. A screen reader that
#      starts unbidden talks over a sighted user's first boot, and it is the
#      kind of "helpful" change that would never be questioned.
mutate D2 "$AUTO" \
    'command -v fcitx5 >/dev/null && fcitx5 -d -r &' \
    'command -v fcitx5 >/dev/null && fcitx5 -d -r &
command -v orca >/dev/null && orca --replace &' \
    "nothing in the image autostarts orca"

# D3 — the vacuity floor. An extractor that finds nothing makes every claim
#      below it a claim about an empty set.
mutate D3 "$SUITE_F" \
    "        m = re.search(r'\\bdnf5?\\b.*?\\binstall\\b(.*)\$', c)" \
    "        m = re.search(r'\\bNOTHING-MATCHES-THIS\\b(.*)\$', c)" \
    "the package list was extracted from Containerfile.core"

# D4 — the extractor stops discarding comment lines, which is precisely the
#      failure it exists to avoid: a commented-out install line would then be
#      read as a live one. The fixture carries exactly that shape, which is why
#      the negative control is a fixture and not a word from the real file.
mutate D4 "$SUITE_F" \
    "    if line.startswith('#'):
        continue" \
    "    if False:
        continue" \
    "a package named only in a comment is not extracted"

echo
printf 'mutants applied=%d, failed-to-apply=%d, caught=%d, SURVIVED=%d\n' \
    "$applied" "$noapply" "$caught" "$survived"
tree_clean || { echo "ABORT: tree dirty at end of run" >&2; exit 3; }
echo "the tree matches HEAD"
[ "$survived" -eq 0 ] && [ "$noapply" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  mutate-installer-a11y.sh — prove test-installer-a11y.sh can go red.
#
#  The suite it checks is an accessibility audit, and an audit that cannot fail
#  is worse than none: it produces a green line somebody will quote. This unit
#  has already been caught three times by assertions structurally incapable of
#  failing (N8, K4, M7 in ROADMAP/state/agents/p2-b.md), and the audit's central
#  claim -- "every focusable control announces itself" -- is precisely the shape
#  that passes over an empty tree.
#
#  Each mutant changes ONE arm and a NAMED assertion must go red.
#
#  Restores are `git checkout --`, never `cp -p` and never `mv`: authoritative
#  about content, and a fresh mtime. The file set is compared against HEAD after
#  every mutate AND every restore, because a previous round of this unit spent a
#  whole run producing verdicts against a tree that had been silently corrupted.
#
#  SLOW: every mutant starts SIX GUI processes on a private Xvfb -- five audited
#  pages plus the wifi page again with a stubbed scanner. Budget a few minutes
#  each.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

GUI="installer/apex-installer-gui"
LIB="tests/lib/atspi.sh"
SUITE_F="installer/test-installer-a11y.sh"
FILES="$GUI $LIB $SUITE_F"
SUITE="./installer/test-installer-a11y.sh"

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

run_suite() {
    env -i HOME="$HOME" PATH="$PATH" USER="${USER:-$(id -un)}" \
        TMPDIR="${TMPDIR:-/tmp}" "$SUITE" 2>&1
}

# mutate <id> <file> <from> <to> <assertion substring that must go red>
mutate() {
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
        printf '      ── what the suite said instead ──\n'
        printf '%s\n' "$out" | grep -E '^(FAIL|SKIP|installer-a11y)' | sed 's/^/      /'
        survived=$((survived + 1))
    fi
    restore
}

echo "── baseline: green, or nothing below means anything ──"
base="$(run_suite)"
printf '%s\n' "$base" | grep -E '^installer-a11y'
if ! printf '%s' "$base" | grep -qE '^installer-a11y: [0-9]+ passed, 0 failed'; then
    echo "ABORT: the suite is not green to begin with" >&2
    printf '%s\n' "$base" | grep -E '^(FAIL|SKIP)' >&2
    exit 3
fi

echo
echo "── the mutants ──"

# ── the wifi page's Tab ring (round 23) ─────────────────────────────────────
# Every one of these four is only exercisable because the scan is stubbed: on a
# machine with no adapter the page they live on does not exist.

# B11 — the defect the ring walk found. Every network in the list was a Tab stop
#       announcing NOTHING: the node that takes the focus is the GtkListBoxRow
#       GTK creates around a plain Gtk.Box, and the SSID label lives inside it.
#       Invisible to the page audit, which does not consider `list item`
#       interactive, and invisible to any grep, because the name IS in the tree.
#
#       The mutant is an EMPTY name rather than a commented-out call: `a11y(row,
#       …)` is a four-line expression, so commenting its first line alone leaves
#       three orphaned continuation lines and the GUI stops parsing. The suite
#       then fails on every page, which is a Python error being caught and not
#       this assertion. An empty name is also the exact shape of the defect that
#       was there: the node existed and announced nothing.
mutate B11 "$GUI" \
    'a11y(row, n["ssid"],' \
    'a11y(row, "",' \
    "every network in the list announces its name"

# B12 — the signal bars go back into the accessibility tree, where a reader
#       spells them out one block character at a time in front of the network's
#       name. Nothing about names or Tab stops changes; only the glyph check
#       can see this.
mutate B12 "$GUI" \
    '                        r.append(lbl(bars, "apex-accent apex-mono", wrap=False,
                                     pres=True))' \
    '                        r.append(lbl(bars, "apex-accent apex-mono", wrap=False))' \
    "the signal bars are out of the accessibility tree"

# B13 — the page opens with the focus on a nameless scroll container, which is
#       the defect as it was found: `role=generic | name= | states=focusable,
#       focused`.
#
#       BOTH guards go, and that is a measured statement about the code rather
#       than a weaker mutant. Removing `set_focusable(False)` alone SURVIVES —
#       run and checked, not assumed: the explicit grab still lands the focus on
#       the named list, and GTK does not tab out to an ancestor the focus is
#       already inside, so the re-focusable container is reachable by neither
#       assertion. Removing the grab alone is B14. They are two guards against
#       one defect and only removing both reproduces it; a mutant for either on
#       its own would be asserting that defence in depth is redundant.
#
#       One substitution, both guards: the line that grabs the focus becomes a
#       line that makes the container focusable again, and since it runs AFTER
#       `sc.set_focusable(False)` it overrides it. The alternative -- a `from`
#       spanning seven lines of source including the comments between them --
#       is an anchor that breaks the next time somebody rewords a comment.
mutate B13 "$GUI" \
    '        GLib.idle_add(lambda: (self.net_list.grab_focus(), False)[1])' \
    '        sc.set_focusable(True)' \
    "page 'wifi': the control focused when the page opens says what it is"

# B14 — nothing claims the keyboard when the page opens. With the scroll
#       container out of the ring and no explicit grab, a keyboard user starts
#       nowhere: no node reports the focused state at all.
mutate B14 "$GUI" \
    '        GLib.idle_add(lambda: (self.net_list.grab_focus(), False)[1])' \
    '        pass  # GLib.idle_add(lambda: (self.net_list.grab_focus(), False)[1])' \
    "page 'wifi': the control focused when the page opens says what it is"

echo
printf 'mutants applied=%d, failed-to-apply=%d, caught=%d, SURVIVED=%d\n' \
    "$applied" "$noapply" "$caught" "$survived"
tree_clean || { echo "ABORT: tree dirty at end of run" >&2; exit 3; }
echo "the tree matches HEAD"
[ "$survived" -eq 0 ] && [ "$noapply" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-a11y-stack.sh — the image ships a screen reader, and nothing
#  starts it for the user (roadmap P2-003, "screen reader … validated").
#
#  ── The claim this corrects ─────────────────────────────────────────────────
#
#  It was on record that APEX shipped "ZERO accessibility packages". That was
#  derived by grepping Containerfiles, which only ever finds what is named
#  explicitly. Asked of the built image's OWN rpmdb instead —
#
#      rpm --dbpath <deploy>/usr/share/rpm -q at-spi2-core speech-dispatcher espeak-ng
#
#  — all three are installed, as transitive dependencies of gtk4 and Qt. The
#  plumbing and the speech engine were already there. The one missing piece was
#  the reader, which is a much smaller delta than the note suggested, and this
#  suite pins both halves so neither claim can drift again.
#
#  ── Why it does not grep for the word ───────────────────────────────────────
#
#  "orca" appears in Containerfile.core in prose as well as in the install line,
#  and a `grep -q orca` would stay green with the package removed and the
#  comment left behind — the exact shape (a grep over a stanza that also NAMES
#  the thing) that let mutant M7 survive earlier in this unit. So the package
#  list is EXTRACTED from the dnf5 invocations, comments discarded, and the
#  extractor is itself checked first against a FIXTURE whose right answer is
#  known — a real install line, a commented-out one, and a trailing-# comment on
#  a live line — and only then run over Containerfile.core. An earlier draft
#  used `ibus` (named only in this file's prose) as the negative control, and
#  that control could not fail: no comment here carries a dnf5 install line for
#  it to be wrongly read out of, so the assertion held however broken the parser
#  was. The fixture can fail, and does.
#
#  ── The other half: nothing starts it ───────────────────────────────────────
#
#  Shipping a screen reader is not the same as switching one on, and switching
#  one on for everybody would be a defect: a reader that starts unbidden talks
#  over a sighted user's first boot. The image autostarts fcitx5 in three
#  places; this asserts orca is in none of them, so "not autostarted" stays a
#  decision rather than becoming an accident either way.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#
#  It starts nothing, opens no window, installs nothing and speaks. Reads files
#  in this repository, and the deployed image's rpmdb if there is one.
#
#  Run from anywhere: ./tests/test-apex-a11y-stack.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e

cd "$(dirname "$0")" || exit 2
ROOT="$(cd .. && pwd)"
CORE="$ROOT/Containerfile.core"
[ -f "$CORE" ] || { echo "FATAL: cannot find $CORE" >&2; exit 2; }

pass=0; fail=0; skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp()  { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
note() { printf 'NOTE  %s\n' "$1"; }
section() { printf '\n── %s ──\n' "$1"; }
finish() {
    printf '\napex-a11y-stack: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

# ═════════════════════════════════════════════════════════════════════════════
section "what Containerfile.core really installs"
# ═════════════════════════════════════════════════════════════════════════════

W="$(mktemp -d "${TMPDIR:-/tmp}/apex-a11y-stack.XXXXXX")" || exit 2
trap 'rm -rf "$W"' EXIT

# Every package name handed to a dnf5 install in a Containerfile, and nothing
# else: comments stripped first, continuations joined, options and shell
# operators discarded.
cat >"$W/extract.py" <<'PY'
import re, sys

text = open(sys.argv[1], errors='replace').read()
text = re.sub(r'\\\n', ' ', text)          # join RUN continuations
out = set()
for raw in text.splitlines():
    line = raw.strip()
    if line.startswith('#'):
        continue
    for chunk in re.split(r'[;&|]{1,2}', line):
        c = chunk.strip()
        m = re.search(r'\bdnf5?\b.*?\binstall\b(.*)$', c)
        if not m:
            continue
        for tok in m.group(1).split():
            if tok.startswith('#'):
                break
            if tok.startswith('-') or '=' in tok or tok == '\\':
                continue
            out.add(tok)
for p in sorted(out):
    print(p)
PY

# ── the extractor's own self-test, on a fixture built for it ────────────────
# "orca is installed" is a claim about this parser, so the parser is checked
# first against a file whose right answer is known — one real install line, one
# commented-out install line, and a trailing comment on a live line. All three
# shapes have to come out correctly or nothing below means anything.
cat >"$W/fixture.Containerfile" <<'FIX'
# An install line that is only prose:
#     dnf5 -y install ghost-from-a-comment
RUN set -eux; \
    dnf5 -y install real-one real-two \
        --setopt=install_weak_deps=False; \
    command -v real-one >/dev/null; \
    dnf5 clean all
RUN dnf5 -y install real-three   # ghost-after-a-hash
FIX
FIXOUT="$(python3 "$W/extract.py" "$W/fixture.Containerfile" | tr '\n' ' ')"
FIXOUT="${FIXOUT% }"
if [ "$FIXOUT" = "real-one real-three real-two" ]; then
    ok "the extractor takes exactly the packages a dnf5 install names"
else
    bad "the extractor takes exactly the packages a dnf5 install names" \
        "want [real-one real-three real-two] got [$FIXOUT]"
fi
if printf '%s\n' "$FIXOUT" | grep -q 'ghost-from-a-comment'; then
    bad "a package named only in a comment is not extracted" \
        "the parser read a commented-out install line — this is the grep trap it exists to avoid"
else
    ok "a package named only in a comment is not extracted"
fi
if printf '%s\n' "$FIXOUT" | grep -q 'ghost-after-a-hash'; then
    bad "a word after a trailing # on a live install line is not extracted" \
        "got [$FIXOUT]"
else
    ok "a word after a trailing # on a live install line is not extracted"
fi

PKGS="$(python3 "$W/extract.py" "$CORE")"

n_pkgs="$(printf '%s\n' "$PKGS" | grep -c .)"
if [ "$n_pkgs" -ge 50 ]; then
    ok "the package list was extracted from Containerfile.core ($n_pkgs names)"
else
    bad "the package list was extracted from Containerfile.core" \
        "only $n_pkgs names — the extractor found almost nothing, so every result below is meaningless"
    finish; exit 1
fi

if printf '%s\n' "$PKGS" | grep -qx 'fcitx5'; then
    ok "and it finds a package this file really does install (fcitx5)"
else
    bad "and it finds a package this file really does install (fcitx5)" \
        "fcitx5 is on a dnf5 install line here"
fi

if printf '%s\n' "$PKGS" | grep -qx 'orca'; then
    ok "Containerfile.core installs orca — the image ships a screen reader"
else
    bad "Containerfile.core installs orca — the image ships a screen reader" \
        "P2-003 names a screen reader in its acceptance line and there is none in the image"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "and nothing starts it for the user"
# ═════════════════════════════════════════════════════════════════════════════
# A reader that starts unbidden talks over a sighted user's first boot. The
# three places this image really does autostart something are checked by name,
# so the assertion cannot pass by looking in the wrong files.

AUTOSTARTS="$ROOT/files/desktop/hypr/apex/session.lua
$ROOT/files/desktop/labwc/autostart
$ROOT/files/system/libexec/apex-hypr-migrate"

present=0; started=""
while IFS= read -r f; do
    [ -f "$f" ] || continue
    present=$((present + 1))
    grep -qE '(^|[^-[:alnum:]])orca([^-[:alnum:]]|$)' "$f" && started="$started $(basename "$f")"
done <<EOF
$AUTOSTARTS
EOF

if [ "$present" -eq 3 ]; then
    ok "all three of this image's autostart surfaces were found and read"
else
    bad "all three of this image's autostart surfaces were found and read" \
        "only $present of 3 exist; a renamed file would make the next assertion vacuous"
fi
# The positive control for the same read: fcitx5 IS autostarted in these files,
# so a grep that finds nothing at all is a broken grep rather than a clean bill.
control=0
while IFS= read -r f; do
    [ -f "$f" ] || continue
    grep -q 'fcitx5' "$f" && control=$((control + 1))
done <<EOF
$AUTOSTARTS
EOF
if [ "$control" -gt 0 ]; then
    ok "the same read does find the thing this image DOES autostart (fcitx5, in $control of them)"
else
    bad "the same read does find the thing this image DOES autostart (fcitx5)" \
        "if fcitx5 cannot be found here, 'orca is absent' means nothing"
fi
if [ -z "$started" ]; then
    ok "nothing in the image autostarts orca"
else
    bad "nothing in the image autostarts orca" \
        "started from:$started — a screen reader must be started by the user who wants it"
fi

# ═════════════════════════════════════════════════════════════════════════════
section "the plumbing was already there — asked of the image, not of a grep"
# ═════════════════════════════════════════════════════════════════════════════

DEPLOY="$(ls -d /ostree/deploy/*/deploy/*/ 2>/dev/null | head -1)"
DB=""
[ -n "$DEPLOY" ] && [ -d "$DEPLOY/usr/share/rpm" ] && DB="$DEPLOY/usr/share/rpm"

if [ -z "$DB" ]; then
    skp "the built image carries at-spi2-core, speech-dispatcher and espeak-ng" \
        "no ostree deployment on this machine to ask; COULD-NOT-RUN, not a pass"
elif ! rpm --dbpath "$DB" -q rpm >/dev/null 2>&1; then
    skp "the built image carries at-spi2-core, speech-dispatcher and espeak-ng" \
        "the deployment's rpmdb at $DB could not be read (permission denied is not absence)"
else
    for p in at-spi2-core at-spi2-atk speech-dispatcher espeak-ng; do
        if rpm --dbpath "$DB" -q "$p" >/dev/null 2>&1; then
            ok "the built image carries $p ($(rpm --dbpath "$DB" -q "$p"))"
        else
            bad "the built image carries $p" \
                "it used to arrive as a transitive dependency of gtk4/Qt; if it is gone, orca alone is not a working reader"
        fi
    done

    # Deliberately a NOTE. This machine's deployment may predate the stanza
    # above, and there is no way from here to tell "built before the change"
    # apart from "the change regressed" — so it is reported and not asserted.
    if rpm --dbpath "$DB" -q orca >/dev/null 2>&1; then
        note "the running deployment already carries $(rpm --dbpath "$DB" -q orca)"
    else
        note "the running deployment has no orca — expected until an image built from this tree is deployed"
        note "  (the assertion that the SOURCE installs it is above, and it is the one that gates the change)"
    fi
fi

finish

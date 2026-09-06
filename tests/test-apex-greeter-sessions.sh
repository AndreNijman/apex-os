#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  BASE-013 criterion 3 regression guard: "compositor names remain implementation
#  details for normal users".
#
#  apex-greet's session picker (files/desktop/apex-greet/GreetContext.qml) reads
#  a session .desktop file's Name= field verbatim and shows it to every user
#  before they have ever logged in — there is no translation layer. That makes
#  Name= the actual user-visible session name, and this asserts it never
#  regresses to a raw compositor name for any of the three sessions this image
#  ships (Hyprland, niri, labwc), for any user session offered at the greeter.
#
#  Values are extracted from the real sources — the printf in Containerfile.base
#  and the shipped .desktop files under files/desktop/wayland-sessions/ — never
#  retyped here, so a future edit to either is exactly what this test sees.
#
#  Needs neither root nor network. Run from the repository root:
#      ./tests/test-apex-greeter-sessions.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINERFILE="${ROOT}/Containerfile.base"
NIRI_DESKTOP="${ROOT}/files/desktop/wayland-sessions/niri.desktop"
LABWC_DESKTOP="${ROOT}/files/desktop/wayland-sessions/apex-labwc.desktop"
for f in "$CONTAINERFILE" "$NIRI_DESKTOP" "$LABWC_DESKTOP"; do
    [ -f "$f" ] || { printf 'missing %s\n' "$f" >&2; exit 1; }
done
command -v python3 >/dev/null 2>&1 || { printf 'python3 is required\n' >&2; exit 1; }

pass=0
fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

# A raw compositor identifier, case-insensitive, at a word boundary on both
# sides — so it catches "Hyprland", "hyprland-uwsm" and "labwc (APEX)" (the
# three literal strings this project shipped before this fix) but does not
# false-positive on the product words this fix introduces ("APEX Dynamic",
# "APEX Scrolling", "APEX Floating" contain none of "hyprland"/"niri"/"labwc").
contains_raw_name() {
    printf '%s' "$1" | tr '[:upper:]' '[:lower:]' \
        | grep -qE '(^|[^a-z])(hyprland|niri|labwc)([^a-z]|$)'
}

section "self-check: the detector has teeth"
# Negative control, repo-style: prove the regex actually catches the exact
# strings BASE-013 verification found shipping, before trusting it to clear
# the real files. A detector that cannot fail is not a detector.
contains_raw_name "Hyprland"      && ok "detector flags the old bare 'Hyprland'"     || bad "detector flags the old bare 'Hyprland'"
contains_raw_name "niri"          && ok "detector flags the old bare 'niri'"          || bad "detector flags the old bare 'niri'"
contains_raw_name "labwc (APEX)"  && ok "detector flags the old 'labwc (APEX)'"       || bad "detector flags the old 'labwc (APEX)'"
if contains_raw_name "APEX Dynamic";   then bad "detector false-positives on 'APEX Dynamic'";   else ok "detector leaves 'APEX Dynamic' alone";   fi
if contains_raw_name "APEX Scrolling"; then bad "detector false-positives on 'APEX Scrolling'"; else ok "detector leaves 'APEX Scrolling' alone"; fi
if contains_raw_name "APEX Floating";  then bad "detector false-positives on 'APEX Floating'";  else ok "detector leaves 'APEX Floating' alone";  fi

check_name() {
    local label="$1" name="$2"
    if [ -z "$name" ]; then
        bad "${label}: could not extract a Name= value at all"
        return
    fi
    if contains_raw_name "$name"; then
        bad "${label}: Name='${name}' leaks a raw compositor name to the greeter"
    else
        ok "${label}: Name='${name}' is a product term, not a compositor name"
    fi
}

# Extracts the Name= value from the printf'd [Desktop Entry] block in
# Containerfile.base, where hyprland.desktop is written inline (no standalone
# source file exists for it — the package normally ships its own, and this is
# only the fallback if it doesn't). Matches the literal two-byte "\n" field
# separator in the printf format string, not an actual newline.
extract_containerfile_name() {
    python3 - "$CONTAINERFILE" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r"printf '\[Desktop Entry\]\\nName=([^\\]*)\\n", text)
print(m.group(1) if m else "")
PY
}

section "hyprland.desktop (generated inline by Containerfile.base)"
check_name "hyprland.desktop" "$(extract_containerfile_name)"

section "niri.desktop"
check_name "niri.desktop" "$(sed -n 's/^Name=//p' "$NIRI_DESKTOP" | head -n1)"

section "apex-labwc.desktop"
check_name "apex-labwc.desktop" "$(sed -n 's/^Name=//p' "$LABWC_DESKTOP" | head -n1)"

section "machine-facing fields are untouched (negative control)"
# The fix must not have "solved" this by deleting the compositor identity
# everywhere. Exec=, DesktopNames= and the session id (the .desktop filename,
# checked by every other test and by portals.conf(5)) are what
# XDG_CURRENT_DESKTOP consumers and hyprctl/niri-msg-shaped tooling actually key
# off, and a maintainer debugging a session still needs them to say the truth.
grep -q '^DesktopNames=niri$' "$NIRI_DESKTOP" \
    && ok "niri.desktop DesktopNames is untouched" || bad "niri.desktop DesktopNames is untouched"
grep -q '^Exec=niri --session$' "$NIRI_DESKTOP" \
    && ok "niri.desktop Exec is untouched" || bad "niri.desktop Exec is untouched"
grep -q '^DesktopNames=labwc$' "$LABWC_DESKTOP" \
    && ok "apex-labwc.desktop DesktopNames is untouched" || bad "apex-labwc.desktop DesktopNames is untouched"
grep -q '^Exec=labwc$' "$LABWC_DESKTOP" \
    && ok "apex-labwc.desktop Exec is untouched" || bad "apex-labwc.desktop Exec is untouched"
grep -q "DesktopNames=Hyprland" "$CONTAINERFILE" \
    && ok "the generated hyprland.desktop DesktopNames is untouched" || bad "the generated hyprland.desktop DesktopNames is untouched"
grep -q "Exec=Hyprland" "$CONTAINERFILE" \
    && ok "the generated hyprland.desktop Exec is untouched" || bad "the generated hyprland.desktop Exec is untouched"

section "the raw name is not simply lost (Comment= still names it)"
# BASE-013 asked that compositor names stay an implementation detail, not that
# they vanish: a maintainer debugging a session, or a user who read a forum
# post naming the compositor by its project name, should still be able to find
# it in the file. Case-insensitive because Comment= prose capitalises "niri" at
# the start of a sentence.
comment_has_raw_name() {
    printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | grep -qE "$2"
}
niri_comment="$(sed -n 's/^Comment=//p' "$NIRI_DESKTOP" | head -n1)"
comment_has_raw_name "$niri_comment" 'niri' \
    && ok "niri.desktop Comment= still names niri" || bad "niri.desktop Comment= still names niri"
labwc_comment="$(sed -n 's/^Comment=//p' "$LABWC_DESKTOP" | head -n1)"
comment_has_raw_name "$labwc_comment" 'labwc' \
    && ok "apex-labwc.desktop Comment= still names labwc" || bad "apex-labwc.desktop Comment= still names labwc"
hypr_comment="$(python3 - "$CONTAINERFILE" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r"\\nComment=([^\\]*)\\n", text)
print(m.group(1) if m else "")
PY
)"
comment_has_raw_name "$hypr_comment" 'hyprland' \
    && ok "the generated hyprland.desktop Comment= still names Hyprland" || bad "the generated hyprland.desktop Comment= still names Hyprland"

printf '\napex-greet session names: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

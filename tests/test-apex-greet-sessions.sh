#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-greet-sessions.sh — what the login session picker calls the three
#  desktops, proved by running the greeter's own enumeration.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  BASE-013's third acceptance criterion is "compositor names remain
#  implementation details for normal users". It was recorded as FAILING, and
#  not for want of a test: the greeter read `Name=` out of each
#  /usr/share/wayland-sessions entry and rendered it verbatim, so the carousel
#  every user cycles through at every login read
#
#      labwc (APEX)   ·   niri   ·   Hyprland
#
#  — three vendored project names, one of them the default session. The fix is
#  data, not code: the entries this repo owns carry product names, and the one
#  entry it does not own (hyprland.desktop belongs to the Hyprland package) is
#  renamed by Containerfile.base. The greeter stays a faithful desktop-entry
#  consumer, which is the behaviour the spec asks of it.
#
#  ── Why it extracts rather than restates ────────────────────────────────────
#  A test that re-implements the enumeration proves only that the test author
#  and the greeter author agree today. There are already THREE copies of that
#  loop in this tree (GreetContext.qml, Containerfile.apex's build assertion,
#  and apex-session-select's validation), which is exactly how a fourth would
#  rot unnoticed. So this suite:
#
#    * lifts the `sh -c` script out of the shipped GreetContext.qml and RUNS
#      it, with only the sessions directory repointed at a staging tree;
#    * lifts the `Name=` rewrite out of Containerfile.base and APPLIES it to an
#      upstream-shaped hyprland.desktop, so the one rename this repo performs
#      at build time is exercised here rather than only in an image build;
#    * lifts the directory-wide wording gate out of Containerfile.apex and RUNS
#      it, then mutates a staged entry and requires it to FAIL.
#
#  Each extraction is checked for plausibility before it is used. An extraction
#  that silently returned an empty script would make every assertion below pass
#  for no reason, so a short or shapeless extraction is a FAILURE, never a skip.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#  Nothing here starts a compositor, a greeter or a session; it opens no window
#  and asks for no password. Everything happens in a temp directory, and the
#  only binaries stubbed are put on a private PATH.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# Deliberately +e, like the other suites in this directory: CI invokes a suite
# as `bash -e {0}`, and under -e an assignment from a failing command ends the
# run silently, mid-section.
set +e

cd "$(dirname "$0")" || exit 2
ROOT="$(cd .. && pwd)"
GREETER="$ROOT/files/desktop/apex-greet/GreetContext.qml"
CF_BASE="$ROOT/Containerfile.base"
CF_APEX="$ROOT/Containerfile.apex"
SESSIONS_SRC="$ROOT/files/desktop/wayland-sessions"
for f in "$GREETER" "$CF_BASE" "$CF_APEX"; do
    [ -f "$f" ] || { echo "FATAL: cannot find $f" >&2; exit 2; }
done

pass=0; fail=0; skip=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
skp() { printf 'SKIP  %s%s\n' "$1" "${2:+  — $2}"; skip=$((skip + 1)); }
section() { printf '\n── %s ──\n' "$1"; }
is() {
    local name=$1 want=$2 got=$3
    if [ "$got" = "$want" ]; then ok "$name"
    else bad "$name" "want [$want] got [$got]"; fi
}
finish() {
    printf '\ngreet-sessions: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

WORK="$(mktemp -d "${TMPDIR:-/tmp}/apex-greet-sessions.XXXXXX")" || exit 2
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT INT TERM

# The tokens a normal user must never be shown by the session picker. This list
# is the suite's own; the build gate carries its own copy, and §5 proves the two
# agree by running the build gate rather than by comparing the lists.
IMPL_TOKENS='labwc hyprland niri sway wlroots openbox gamescope wayfire river'

names_look_like_implementations() {
    # Reads names on stdin, prints the offending ones.
    local n
    while IFS= read -r n; do
        local low t
        low="$(printf '%s' "$n" | tr 'A-Z' 'a-z')"
        for t in $IMPL_TOKENS; do
            case "$low" in *"$t"*) printf '%s\n' "$n" ;; esac
        done
    done
}

# ─────────────────────────────────────────────────────────────────────────────
section "§1 the greeter's enumeration, lifted out of the shipped QML"
# ─────────────────────────────────────────────────────────────────────────────
# GreetContext.qml holds the enumeration as a JS concatenation of string
# literals inside a Process's `command` array. Reassembling it is a few lines of
# parsing; re-typing it is a fourth copy of a loop that already exists three
# times in this tree.
ENUM_SH="$WORK/greeter-enumerate.sh"
python3 - "$GREETER" "$ENUM_SH" <<'PY'
import json, re, sys

src = open(sys.argv[1], encoding="utf-8").read()
# The block is identified by the glob it walks, which is also the one string in
# the file that names the sessions directory inside a command array.
anchor = src.find('"for f in /usr/share/wayland-sessions/*.desktop; do"')
if anchor < 0:
    sys.exit("no wayland-sessions enumeration found in the greeter")

# Walk forward literal by literal. The array cannot be found by searching for
# the next "]" — the shell script itself contains "[ -r \"$f\" ]", and a naive
# search stops inside the first string. So: consume quoted literals with the
# scanner, and treat a "]" seen OUTSIDE a literal as the end of the array.
LIT = re.compile(r'"(?:[^"\\]|\\.)*"')
parts, i, n = [], anchor, len(src)
while i < n:
    c = src[i]
    if c == '"':
        m = LIT.match(src, i)
        if not m:
            sys.exit("unterminated string literal in the enumeration")
        parts.append(json.loads(m.group(0)))
        i = m.end()
        continue
    if c == "]":
        break
    if c not in " \t\r\n+":
        sys.exit("unexpected %r between the enumeration's literals" % c)
    i += 1
else:
    sys.exit("the enumeration's command array is not terminated")
open(sys.argv[2], "w", encoding="utf-8").write("".join(parts))
PY
extract_rc=$?
is "the enumeration is extractable from the shipped GreetContext.qml" "0" "$extract_rc"

# Plausibility. Without these four the extraction could return "" and every
# assertion in §3 would pass by enumerating nothing.
enum_src="$(cat "$ENUM_SH" 2>/dev/null)"
if [ "${#enum_src}" -ge 200 ]; then
    ok "…and is a whole script rather than a fragment (${#enum_src} chars)"
else
    bad "…and is a whole script rather than a fragment" "got ${#enum_src} chars"
fi
case "$enum_src" in
    *"s/^Name=//p"*) ok "…and it is the code that reads Name= out of the entries" ;;
    *) bad "…and it is the code that reads Name= out of the entries" "no Name= sed in the extract" ;;
esac
case "$enum_src" in
    *"TryExec"*) ok "…and the TryExec gate came with it" ;;
    *) bad "…and the TryExec gate came with it" "no TryExec in the extract" ;;
esac
case "$enum_src" in
    *"/usr/share/wayland-sessions"*) ok "…and it still names the real sessions directory" ;;
    *) bad "…and it still names the real sessions directory" "path missing" ;;
esac

# ─────────────────────────────────────────────────────────────────────────────
section "§2 a sessions directory with the shape of a built image"
# ─────────────────────────────────────────────────────────────────────────────
STAGE="$WORK/wayland-sessions"
mkdir -p "$STAGE"
copied=0
for f in "$SESSIONS_SRC"/*.desktop; do
    [ -f "$f" ] || continue
    cp "$f" "$STAGE/" && copied=$((copied + 1))
done
is "the three session entries this repo owns are stageable" "3" "$copied"

# hyprland.desktop is NOT in this repo — it comes from the Hyprland package. The
# upstream shape is reproduced here so the rename below has something real to
# act on, and it is deliberately the name upstream ships.
cat > "$STAGE/hyprland.desktop" <<'EOF'
[Desktop Entry]
Name=Hyprland
Comment=An intelligent dynamic tiling Wayland compositor
Exec=Hyprland
Type=Application
DesktopNames=Hyprland
EOF
is "the upstream hyprland entry starts out naming its compositor" "Hyprland" \
   "$(sed -n 's/^Name=//p' "$STAGE/hyprland.desktop" | head -n1)"

# The rename rule, lifted out of Containerfile.base and run here. Extracting it
# rather than repeating it is what makes this assertion about the BUILD.
RENAME="$(grep -oE "sed -i 's/\^Name=\.\*/Name=[^/]*/' /usr/share/wayland-sessions/hyprland\.desktop" "$CF_BASE" | head -n1)"
if [ -n "$RENAME" ]; then
    ok "Containerfile.base carries a Name= rewrite for the upstream entry"
    sh -c "${RENAME//\/usr\/share\/wayland-sessions/$STAGE}"
    is "…and running it renames the entry away from the compositor's name" \
       "APEX Tiling" "$(sed -n 's/^Name=//p' "$STAGE/hyprland.desktop" | head -n1)"
else
    bad "Containerfile.base carries a Name= rewrite for the upstream entry" \
        "no sed rule found; the image would show users 'Hyprland'"
    skp "…and running it renames the entry away from the compositor's name" \
        "nothing to run"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "§3 what the greeter would actually put on the carousel"
# ─────────────────────────────────────────────────────────────────────────────
# A private PATH so the TryExec gate is answered by this suite and not by
# whatever happens to be installed on the machine running it.
STUBS="$WORK/stubs"; mkdir -p "$STUBS"
run_enum() {
    # $1: extra PATH entries (may be empty). Prints "id<TAB>name<TAB>exec".
    local extra="$1" script
    script="${enum_src//\/usr\/share\/wayland-sessions/$STAGE}"
    PATH="${extra:+$extra:}/usr/bin:/bin" sh -c "$script"
}

out="$(run_enum "")"
is "the greeter enumerates every session whose binary is present" "3" \
   "$(printf '%s\n' "$out" | grep -c .)"

name_of() { printf '%s\n' "$out" | awk -F'\t' -v id="$1" '$1 == id { print $2 }'; }
is "labwc is offered as a product name"   "APEX Floating"    "$(name_of apex-labwc)"
is "niri is offered as a product name"    "APEX Scrolling"   "$(name_of niri)"
is "Hyprland is offered as a product name" "APEX Tiling"     "$(name_of hyprland)"

# The criterion itself, asserted over what the greeter PRINTS rather than over
# the files — a name could be reintroduced by a future fallback in the QML
# without any file changing.
offenders="$(printf '%s\n' "$out" | awk -F'\t' '{ print $2 }' | names_look_like_implementations)"
if [ -z "$offenders" ]; then
    ok "nothing the session picker shows names a compositor project"
else
    bad "nothing the session picker shows names a compositor project" \
        "$(printf '%s' "$offenders" | tr '\n' ' ')"
fi

# ids are the load-bearing half and must NOT have moved with the wording.
ids="$(printf '%s\n' "$out" | awk -F'\t' '{ print $1 }' | sort | tr '\n' ' ')"
is "the ids behind those names are unchanged" "apex-labwc hyprland niri " "$ids"

# The default session is named, not positional, and it is named by id.
want_default="$(grep -oE 'defaultSession: "[^"]*"' "$GREETER" | head -n1 | cut -d'"' -f2)"
is "the greeter still names its default session by id" "hyprland" "$want_default"
if printf '%s\n' "$out" | awk -F'\t' '{ print $1 }' | grep -qx "$want_default"; then
    ok "…and that id is one the enumeration actually offers"
else
    bad "…and that id is one the enumeration actually offers" "$want_default not enumerated"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "§4 the gaming session, which is gated on a binary rather than a name"
# ─────────────────────────────────────────────────────────────────────────────
if printf '%s\n' "$out" | grep -q '^apex-gaming'; then
    bad "apex-gaming stays hidden while gamescope is absent" "it was offered"
else
    ok "apex-gaming stays hidden while gamescope is absent"
fi

# TryExec names an absolute path, so the stub has to live at it. A private root
# plus a PATH entry is not enough; the entry says /usr/bin/gamescope.
tryexec="$(sed -n 's/^TryExec=//p' "$STAGE/apex-gaming.desktop" | head -n1)"
is "the gaming entry gates on an absolute binary path" "/usr/bin/gamescope" "$tryexec"
if [ -x "$tryexec" ]; then
    # A machine that really has gamescope (katana does) proves the other half
    # for free, and honestly: the entry must appear, under its product name.
    out_with="$(run_enum "")"
    is "…and where gamescope IS installed the session appears" "APEX Gaming Mode" \
       "$(printf '%s\n' "$out_with" | awk -F'\t' '$1 == "apex-gaming" { print $2 }')"
else
    # Otherwise assert the same thing against a relative gate, by rewriting the
    # staged copy only — this proves the enumeration's `command -v` branch, not
    # a hand-waved claim about it.
    printf '#!/bin/sh\nexit 0\n' > "$STUBS/gamescope-probe"; chmod +x "$STUBS/gamescope-probe"
    sed -i 's|^TryExec=.*|TryExec=gamescope-probe|' "$STAGE/apex-gaming.desktop"
    out_with="$(run_enum "$STUBS")"
    is "…and once its binary exists the session appears, under its product name" \
       "APEX Gaming Mode" \
       "$(printf '%s\n' "$out_with" | awk -F'\t' '$1 == "apex-gaming" { print $2 }')"
    sed -i "s|^TryExec=.*|TryExec=$tryexec|" "$STAGE/apex-gaming.desktop"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "§5 the build gate, run here and then proved able to fail"
# ─────────────────────────────────────────────────────────────────────────────
# Containerfile.apex sweeps the finished sessions directory and refuses a build
# whose picker names a compositor. That gate only ever runs inside an image
# build, which on this program happens on katana and not on every commit — so
# it is lifted out and run against the staged tree, where it is cheap.
GATE="$WORK/wording-gate.sh"
python3 - "$CF_APEX" "$GATE" "$STAGE" <<'PY'
import importlib.machinery, importlib.util, pathlib, re, sys

cf, out, stage = sys.argv[1], sys.argv[2], sys.argv[3]
# Reuse the repo's own Containerfile reader so comment-stripping and line
# continuation behave exactly as the Dockerfile parser does.
# The checker has no .py suffix, so the loader has to be named explicitly;
# spec_from_file_location alone returns None for an extension-less file.
checker = pathlib.Path(cf).parent / "files/scripts/check-containerfile-order"
spec = importlib.util.spec_from_file_location(
    "cf_order", str(checker),
    loader=importlib.machinery.SourceFileLoader("cf_order", str(checker)))
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

logical = [line for _, line in mod.logical_lines(cf)]
hits = [l for l in logical if "session wording:" in l]
if len(hits) != 1:
    sys.exit("expected exactly one wording gate in %s, found %d" % (cf, len(hits)))
line = hits[0]
# The sweep cannot be found by looking for "for f in <sessions>/*.desktop" — the
# same RUN replays the greeter's OWN enumeration from inside a single-quoted
# ENUM='...' string, whose `done'` does not end with the semicolon a naive
# span would look for, so one span swallows both loops. The sweep is anchored on
# its counter instead, which is unique and is also the thing that makes it
# unable to pass over an empty directory.
spans = [m.group(1) for m in re.finditer(
    r'(swept=0; .*?echo "session picker: [^"]*";)', line, re.S)]
if len(spans) != 1:
    sys.exit("expected exactly one wording sweep, found %d" % len(spans))
body = spans[0].rstrip(";")
if "session wording:" not in body or "swept=$((swept + 1))" not in body:
    sys.exit("the extracted sweep is missing its per-entry report or its counter")
open(out, "w", encoding="utf-8").write(
    "set -e\n" + body.replace("/usr/share/wayland-sessions", stage) + "\n")
PY
gate_rc=$?
is "the wording gate is extractable from Containerfile.apex" "0" "$gate_rc"

if [ "$gate_rc" -eq 0 ]; then
    gate_out="$(sh "$GATE" 2>&1)"; gate_status=$?
    is "…and the staged sessions directory passes it" "0" "$gate_status"
    # A gate that prints one line per session is a gate that looked at them all.
    is "…having looked at every entry" "4" \
       "$(printf '%s\n' "$gate_out" | grep -c 'session wording:')"

    # MUTATION. Put the upstream name back on one entry and require a refusal —
    # otherwise this whole section is a gate that cannot fail.
    cp "$STAGE/hyprland.desktop" "$WORK/hyprland.desktop.orig"
    sed -i 's/^Name=.*/Name=Hyprland/' "$STAGE/hyprland.desktop"
    mut_out="$(sh "$GATE" 2>&1)"; mut_status=$?
    if [ "$mut_status" -ne 0 ]; then
        ok "…and refuses the moment one entry names its compositor again"
    else
        bad "…and refuses the moment one entry names its compositor again" \
            "the gate passed with Name=Hyprland present"
    fi
    case "$mut_out" in
        *hyprland.desktop*) ok "…naming the file it refused" ;;
        *) bad "…naming the file it refused" "message: $(printf '%s' "$mut_out" | tr '\n' ' ')" ;;
    esac
    cp "$WORK/hyprland.desktop.orig" "$STAGE/hyprland.desktop"

    # MUTATION, the other way: an entry with no Name= at all is a picker that
    # would render an empty label, which is a different failure and also refused.
    cp "$STAGE/niri.desktop" "$WORK/niri.desktop.orig"
    sed -i '/^Name=/d' "$STAGE/niri.desktop"
    sh "$GATE" >/dev/null 2>&1
    if [ $? -ne 0 ]; then ok "…and refuses an entry with no Name= at all"
    else bad "…and refuses an entry with no Name= at all" "the gate passed"; fi
    cp "$WORK/niri.desktop.orig" "$STAGE/niri.desktop"

    sh "$GATE" >/dev/null 2>&1
    is "…and is green again once both mutations are reverted" "0" "$?"
else
    for m in "…and the staged sessions directory passes it" \
             "…having looked at every entry" \
             "…and refuses the moment one entry names its compositor again" \
             "…naming the file it refused" \
             "…and refuses an entry with no Name= at all" \
             "…and is green again once both mutations are reverted"; do
        skp "$m" "the gate could not be extracted"
    done
fi

# ─────────────────────────────────────────────────────────────────────────────
section "§6 the fields the rename must not have moved"
# ─────────────────────────────────────────────────────────────────────────────
# Each of these is something in the tree that keys off a session entry. They are
# ids and desktop names, never the label, and that is the property that makes
# the rename safe.
is "niri still announces DesktopNames=niri (niri-portals.conf keys off it)" "niri" \
   "$(sed -n 's/^DesktopNames=//p' "$STAGE/niri.desktop" | head -n1)"
is "labwc still announces DesktopNames=labwc (the shell's only detection signal)" "labwc" \
   "$(sed -n 's/^DesktopNames=//p' "$STAGE/apex-labwc.desktop" | head -n1)"
is "the gaming entry still announces DesktopNames=gamescope" "gamescope" \
   "$(sed -n 's/^DesktopNames=//p' "$STAGE/apex-gaming.desktop" | head -n1)"
is "the labwc session still execs the compositor itself" "labwc" \
   "$(sed -n 's/^Exec=//p' "$STAGE/apex-labwc.desktop" | head -n1)"
is "the niri session still execs it as a session" "niri --session" \
   "$(sed -n 's/^Exec=//p' "$STAGE/niri.desktop" | head -n1)"

# apex-session-select validates a requested session against the directory, by
# id. A rename that moved ids would break it silently — the helper would refuse
# every session, and the greeter would have nothing to hand it.
SELECT="$ROOT/files/system/libexec/apex-session-select"
if [ -f "$SELECT" ]; then
    if grep -q 'wayland-sessions' "$SELECT"; then
        ok "apex-session-select still resolves sessions out of the same directory"
    else
        bad "apex-session-select still resolves sessions out of the same directory" \
            "it no longer names the directory"
    fi
    if grep -qE '\bName=' "$SELECT"; then
        bad "…and keys off ids rather than labels" "it reads Name= from the entries"
    else
        ok "…and keys off ids rather than labels"
    fi
else
    skp "apex-session-select still resolves sessions out of the same directory" "not present"
    skp "…and keys off ids rather than labels" "not present"
fi

finish

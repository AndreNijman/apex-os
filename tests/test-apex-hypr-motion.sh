#!/usr/bin/env bash
# The compositor's motion classes, and APEX Shell's surfaces left to the shell.
#
#     tests/test-apex-hypr-motion.sh
#
# files/desktop/hypr/apex/appearance.lua gives Hyprland one motion class per
# kind of change (APEX Shell UI/UX roadmap v3 Phase 20): windows in, windows
# out, moves, fades, layers, workspaces, borders — on the shell's own curves, so
# a window opening and a panel opening read as one system. And it exempts the
# shell's layer surfaces from Hyprland's layer animations, because the shell
# draws every one of their motions itself.
#
# That exemption is the part worth measuring, and it was measured before it was
# written: in a nested Hyprland 0.56.2 on the stock tree, the network panel
# poured out of the bar TRANSLUCENT — 66-90 % of its footprint a blend of panel
# and wallpaper from 240 to 345 ms into the open, snapping opaque at 400 ms,
# which is fadeLayersIn (inherited from `fade`, 400 ms) running on top of the
# shell's own pour. With the rule, the only blend left is the shell's content
# fade at its edges (at most 7 %). That probe needs a GPU, a nested compositor
# and the shell, so it is not this suite; this suite pins what it found.
#
# Two halves:
#
#   STATIC (always, python3 only) — the curves are the shell's tokens, every
#     class exists, closing is shorter than opening, a move is direct, and the
#     exemption names the shell's namespace with no_anim.
#   VERIFY (where Hyprland is installed) — `Hyprland --verify-config` accepts
#     the file. It rejects an unknown leaf, an unknown style and an unknown
#     layer-rule key, so "config ok" means the names are real in THIS Hyprland.
#
# Each half is mutated to prove it can fail. The verify half skips with status 0
# where Hyprland is missing (CI); the static half runs everywhere.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
F="$root/files/desktop/hypr/apex/appearance.lua"

pass=0; fail=0; skipped=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
skip() { printf 'SKIP  %s\n' "$1"; skipped=$((skipped + 1)); }
sec()  { printf '\n── %s ──\n' "$1"; }
finish() {
    printf '\napex hypr-motion: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skipped"
    [ "$fail" -eq 0 ]
}

[ -f "$F" ] || { bad "appearance.lua is present"; finish; exit $?; }

# ── STATIC ───────────────────────────────────────────────────────────────────
# One verdict line per rule: "<RULE> PASS|FAIL <detail>".
static_verdicts() {
    python3 - "$1" <<'PY'
import re, sys
src = open(sys.argv[1]).read()
code = "\n".join(l.split("--", 1)[0] for l in src.split("\n"))

def verdict(rule, good, detail=""):
    print(rule, "PASS" if good else "FAIL", detail)

# The shell's tokens (apex-shell src/theme/motion.js CURVES), as control points.
TOKENS = {
    "apexDecel":    [(0.05, 0.7), (0.1, 1.0)],   # emphasizedDecel
    "apexAccel":    [(0.3, 0.0), (0.8, 0.15)],   # emphasizedAccel
    "apexStandard": [(0.2, 0.0), (0.0, 1.0)],    # standard
    "apexEffects":  [(0.3, 0.7), (0.3, 1.0)],    # effects
}
curves = {}
for m in re.finditer(r'hl\.curve\(\s*"(\w+)"\s*,\s*\{[^}]*points\s*=\s*\{\s*\{\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\}\s*,\s*\{\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\}', code):
    curves[m.group(1)] = [(float(m.group(2)), float(m.group(3))), (float(m.group(4)), float(m.group(5)))]
off = [n for n, p in TOKENS.items() if curves.get(n) != p]
verdict("CURVES", not off, "drifted or missing: " + ",".join(off))

anims = {}
for m in re.finditer(r'hl\.animation\(\s*\{([^}]*)\}\s*\)', code):
    body = m.group(1)
    leaf = re.search(r'leaf\s*=\s*"(\w+)"', body)
    if not leaf: continue
    speed = re.search(r'speed\s*=\s*([\d.]+)', body)
    curve = re.search(r'bezier\s*=\s*"(\w+)"', body)
    on = re.search(r'enabled\s*=\s*true', body)
    anims[leaf.group(1)] = dict(speed=float(speed.group(1)) if speed else None,
                                curve=curve.group(1) if curve else None, on=bool(on))
need = ["windowsIn", "windowsOut", "windowsMove", "fadeIn", "fadeOut",
        "workspaces", "layersIn", "layersOut", "border"]
missing = [l for l in need if l not in anims or not anims[l]["on"] or anims[l]["speed"] is None]
verdict("CLASSES", not missing, "missing or disabled: " + ",".join(missing))

def spd(l): return anims.get(l, {}).get("speed") or 0
verdict("CLOSE_SHORTER",
        0 < spd("windowsOut") < spd("windowsIn") and 0 < spd("layersOut") < spd("layersIn"),
        f"windows {spd('windowsIn')}/{spd('windowsOut')}, layers {spd('layersIn')}/{spd('layersOut')}")

cv = lambda l: anims.get(l, {}).get("curve")
verdict("CHARACTER",
        cv("windowsIn") == "apexDecel" and cv("windowsOut") == "apexAccel"
        and cv("windowsMove") == "apexStandard" and cv("workspaces") == "apexDecel",
        f"in={cv('windowsIn')} out={cv('windowsOut')} move={cv('windowsMove')} ws={cv('workspaces')}")

rules = [m.group(1) for m in re.finditer(r'hl\.layer_rule\(\s*\{(.*?)\}\s*\)', code, re.S)]
exempt = [r for r in rules
          if re.search(r'namespace\s*=\s*"\^quickshell\$"', r) and re.search(r'no_anim\s*=\s*true', r)]
verdict("EXEMPT", len(exempt) == 1, f"{len(exempt)} rule(s) exempt ^quickshell$")
PY
}

label() {
    case "$1" in
        CURVES)        echo "the four curves are APEX Shell's tokens (emphasizedDecel/Accel, standard, effects)" ;;
        CLASSES)       echo "every motion class is declared and enabled: windows in/out/move, fades, workspaces, layers, border" ;;
        CLOSE_SHORTER) echo "closing is shorter than opening, for windows and for layers" ;;
        CHARACTER)     echo "opening decelerates, closing accelerates, a move is direct, a workspace switch settles" ;;
        EXEMPT)        echo "exactly one layer rule leaves APEX Shell's surfaces (^quickshell\$) unanimated" ;;
    esac
}

sec "static: the motion classes"
verdicts="$(static_verdicts "$F")"
while read -r rule verdict detail; do
    [ -n "$rule" ] || continue
    if [ "$verdict" = PASS ]; then ok "$(label "$rule")"; else bad "$(label "$rule") — $detail"; fi
done <<<"$verdicts"
if [ "$(grep -c . <<<"$verdicts")" -eq 5 ]; then ok "all five static rules were evaluated"
else bad "expected five static verdicts, got: $verdicts"; fi

MW="$(mktemp -d)"; trap 'rm -rf "$MW"' EXIT INT TERM
mutant() {   # mutant <label> <python old> <new> <rule that must FAIL>
    cp "$F" "$MW/a.lua"
    if ! python3 - "$MW/a.lua" "$2" "$3" <<'PY'
import sys
p, old, new = sys.argv[1:4]
s = open(p).read()
if old not in s: sys.exit(3)
open(p, "w").write(s.replace(old, new, 1))
PY
    then bad "self-test $1: the mutation did not apply"; return; fi
    if static_verdicts "$MW/a.lua" | grep -q "^$4 FAIL"; then ok "self-test $1: caught"
    else bad "self-test $1: SURVIVED"; fi
}
mutant "a curve drifting from its token" '{ { 0.05, 0.7 }, { 0.1, 1.0 } }' '{ { 0.05, 0.7 }, { 0.2, 1.0 } }' CURVES
mutant "the move class dropped" 'hl.animation({ leaf = "windowsMove"' 'hl.animation({ leaf = "windowsMoveX"' CLASSES
mutant "a close as long as the open" 'leaf = "windowsOut",  enabled = true, speed = 1.75' 'leaf = "windowsOut",  enabled = true, speed = 2.4' CLOSE_SHORTER
mutant "a close on the opening curve" 'speed = 1.75, bezier = "apexAccel"' 'speed = 1.75, bezier = "apexDecel"' CHARACTER
mutant "the exemption animating again" 'no_anim = true' 'no_anim = false' EXEMPT

# ── VERIFY ───────────────────────────────────────────────────────────────────
sec "verify: Hyprland accepts it"
if ! command -v Hyprland >/dev/null 2>&1; then
    skip "Hyprland is not installed; the names cannot be checked against a real parser here"
    finish; exit $?
fi

verify() {   # verify <appearance.lua> — prints Hyprland's parsing result
    local H RT
    H="$(mktemp -d)"; RT="$(mktemp -d)"; chmod 0700 "$RT"
    mkdir -p "$H/.config/hypr/apex"
    cp "$1" "$H/.config/hypr/apex/appearance.lua"
    printf 'require("apex.appearance")\n' > "$H/.config/hypr/hyprland.lua"
    env -i HOME="$H" PATH=/usr/bin:/bin XDG_RUNTIME_DIR="$RT" \
        timeout 60 Hyprland --verify-config -c "$H/.config/hypr/hyprland.lua" 2>&1 \
        | sed -n '/Config parsing result/,$p'
    rm -rf "$H" "$RT"
}

got="$(verify "$F")"
if grep -qx 'config ok' <<<"$got"; then
    ok "Hyprland --verify-config: config ok"
else
    bad "Hyprland --verify-config: config ok — got: $(grep -v '^$\|====' <<<"$got" | head -3)"
fi

vmutant() {   # vmutant <label> <old> <new> — verify must REJECT it
    cp "$F" "$MW/v.lua"
    python3 - "$MW/v.lua" "$2" "$3" <<'PY' || { bad "self-test $1: the mutation did not apply"; return; }
import sys
p, old, new = sys.argv[1:4]
s = open(p).read()
if old not in s: sys.exit(3)
open(p, "w").write(s.replace(old, new, 1))
PY
    if grep -qx 'config ok' <<<"$(verify "$MW/v.lua")"; then bad "self-test $1: ACCEPTED"
    else ok "self-test $1: rejected"; fi
}
vmutant "an animation leaf this Hyprland does not have" 'leaf = "windowsIn"' 'leaf = "windowsInn"'
vmutant "a style this Hyprland does not have" 'style = "popin 87%"' 'style = "wobble"'
vmutant "a layer-rule key this Hyprland does not have" 'namespace = "^quickshell$"' 'namespacex = "^quickshell$"'

finish

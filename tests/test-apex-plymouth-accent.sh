#!/usr/bin/env bash
# The boot splash follows the owner's matugen accent: an initramfs helper reads
# /var/lib/apex-greet/accents/<last-user> through /sysroot and tells the theme
# `plymouth update --status=apex-accent-NN`. This runs the real helper against a
# fake /sysroot with a stub plymouth that records what it was told, and checks
# the theme ships and handles every bucket the helper can name.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HELPER="$ROOT/files/dracut/apex-plymouth-accent/apex-plymouth-accent"
THEME="$ROOT/files/branding/plymouth/apex-os-chartreuse"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0 fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }

mkdir -p "$WORK/bin"
cat > "$WORK/bin/plymouth" <<'STUB'
#!/bin/sh
case "$1" in
    --ping) exit 0 ;;
    update) printf '%s\n' "$2" >> "$PLYMOUTH_LOG" ;;
esac
exit 0
STUB
chmod +x "$WORK/bin/plymouth"

# run <layout> <user> <accent file content or ABSENT> -> the status sent, or ""
run() {
    local sys="$WORK/sys-$RANDOM" var
    case "$1" in
        ostree)   var="$sys/ostree/deploy/default/var" ;;
        composefs) var="$sys/state/os/default/var" ;;
    esac
    mkdir -p "$var/lib/apex-greet/accents"
    printf '%s' "$2" > "$var/lib/apex-greet/last-user"
    [ "$3" = ABSENT ] || printf '%b' "$3" > "$var/lib/apex-greet/accents/$2"
    local log="$WORK/log-$RANDOM"; : > "$log"
    PATH="$WORK/bin:$PATH" PLYMOUTH_LOG="$log" APEX_SYSROOT="$sys" sh "$HELPER"
    cat "$log"
}

got="$(run ostree andre '#fab898\n')"
[ "$got" = "--status=apex-accent-01" ] && ok "#fab898 (peach, hue 20.6°) picks bucket 01 on an ostree root" \
    || bad "#fab898 (peach, hue 20.6°) picks bucket 01 on an ostree root (got '$got')"
got="$(run composefs andre '#d9f99d\n')"
[ "$got" = "--status=apex-accent-05" ] && ok "the chartreuse itself (hue 80.9°) picks bucket 05 on a composefs root" \
    || bad "the chartreuse itself (hue 80.9°) picks bucket 05 on a composefs root (got '$got')"
got="$(run ostree andre '#a6d0f7\n')"
[ "$got" = "--status=apex-accent-14" ] && ok "a blue (#a6d0f7, hue 209°) picks bucket 14" \
    || bad "a blue (#a6d0f7, hue 209°) picks bucket 14 (got '$got')"
got="$(run ostree andre '#fa98a0\n')"
[ "$got" = "--status=apex-accent-00" ] && ok "a hue just under 360° wraps to bucket 00" \
    || bad "a hue just under 360° wraps to bucket 00 (got '$got')"
got="$(run ostree andre '#c8c8c8\n')"
[ -z "$got" ] && ok "a grey accent has no hue and keeps the default" || bad "a grey accent has no hue and keeps the default (got '$got')"
got="$(run ostree andre ABSENT)"
[ -z "$got" ] && ok "no accent published keeps the default" || bad "no accent published keeps the default (got '$got')"
got="$(run ostree andre 'orange\n')"
[ -z "$got" ] && ok "a value that is not #rrggbb is ignored" || bad "a value that is not #rrggbb is ignored (got '$got')"
got="$(run ostree '../etc' '#fab898\n')"
[ -z "$got" ] && ok "a last-user that is not a plain name is refused" || bad "a last-user that is not a plain name is refused (got '$got')"

# The theme must ship, and name, every bucket the helper can send.
missing=0
for n in $(seq 0 23); do
    nn=$(printf '%02d' "$n")
    for img in comet glow spark; do [ -s "$THEME/accent/$nn/$img.png" ] || missing=$((missing + 1)); done
    grep -q "ACCENT_NAME\[$n\] = \"$nn\"" "$THEME/apex-os.script" || missing=$((missing + 1))
done
[ "$missing" -eq 0 ] && ok "the theme ships and names all 24 buckets" || bad "the theme ships and names all 24 buckets ($missing missing)"
grep -q 'Plymouth.SetUpdateStatusFunction(apex_accent_status);' "$THEME/apex-os.script" \
    && ok "the theme listens for the status" || bad "the theme listens for the status"
grep -qE 'global\.spark_image *= *Image\(dir \+ "spark.png"\)' "$THEME/apex-os.script" \
    && ok "the handler replaces the image refresh() draws, not a local copy" \
    || bad "the handler replaces the image refresh() draws, not a local copy"

printf '\napex plymouth-accent: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

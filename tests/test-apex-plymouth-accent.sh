#!/usr/bin/env bash
# The boot and shutdown splash follow the owner's matugen accent, from the FIRST
# frame: the theme is chosen before plymouthd starts, never switched mid-way.
#
#   writer  files/system/libexec/apex-plymouth-accent (real root): accent file →
#           hue bucket → /etc/plymouth/plymouthd.conf (shutdown) and
#           loader/credentials/apex.accent.cred on the ESP (boot)
#   reader  files/dracut/apex-plymouth-accent/apex-plymouth-theme (initramfs,
#           before plymouth-start): the credential → the initramfs's
#           plymouthd.conf
#
# Both run here for real, against directories standing in for the ESP.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRITER="$ROOT/files/system/libexec/apex-plymouth-accent"
READER="$ROOT/files/dracut/apex-plymouth-accent/apex-plymouth-theme"
THEMES="$ROOT/files/branding/plymouth"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0 fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }

# write <accent or ABSENT> -> sets up a machine, runs the writer; echoes its dir
write() {
    local m="$WORK/m-$RANDOM"
    mkdir -p "$m/greet/accents" "$m/esp" "$m/etc"
    printf 'andre' > "$m/greet/last-user"
    [ "$1" = ABSENT ] || printf '%s\n' "$1" > "$m/greet/accents/andre"
    printf '[Daemon]\nTheme=apex-os-chartreuse\nShowDelay=0\n' > "$m/etc/plymouthd.conf"
    APEX_GREET_VAR="$m/greet" APEX_PLYMOUTH_CONF="$m/etc/plymouthd.conf" \
        APEX_PLYMOUTH_THEMES="$THEMES" APEX_ESP_DIRS="$m/esp" sh "$WRITER"
    printf '%s' "$m"
}
# read <machine dir> -> runs the reader against that ESP; echoes the theme chosen
boot() {
    local c="$1/initrd-plymouthd.conf"
    printf '[Daemon]\nTheme=apex-os-chartreuse\n' > "$c"
    APEX_PLYMOUTH_CONF="$c" APEX_PLYMOUTH_THEMES="$THEMES" APEX_ESP_DIRS="$1/esp" sh "$READER"
    sed -n 's/^Theme=//p' "$c"
}

m="$(write '#fab898')"
[ "$(cat "$m/esp/loader/credentials/apex.accent.cred" 2>/dev/null)" = "01" ] \
    && ok "#fab898 (peach) writes bucket 01 to the ESP credential" \
    || bad "#fab898 (peach) writes bucket 01 to the ESP credential"
grep -qx 'Theme=apex-os-accent-01' "$m/etc/plymouthd.conf" && grep -qx 'ShowDelay=0' "$m/etc/plymouthd.conf" \
    && ok "shutdown's plymouthd.conf names the accent theme, other settings kept" \
    || bad "shutdown's plymouthd.conf names the accent theme, other settings kept"
[ "$(boot "$m")" = "apex-os-accent-01" ] \
    && ok "the initramfs reader starts plymouth in apex-os-accent-01" \
    || bad "the initramfs reader starts plymouth in apex-os-accent-01 (got '$(boot "$m")')"

for pair in '#d9f99d 05' '#a6d0f7 14' '#fa98a0 00'; do
    set -- $pair
    m="$(write "$1")"
    [ "$(boot "$m")" = "apex-os-accent-$2" ] && ok "$1 boots in apex-os-accent-$2" \
        || bad "$1 boots in apex-os-accent-$2 (got '$(boot "$m")')"
done

m="$(write '#fab898')"; mv "$m/greet/accents/andre" "$m/x"; printf '#c8c8c8\n' > "$m/greet/accents/andre"
APEX_GREET_VAR="$m/greet" APEX_PLYMOUTH_CONF="$m/etc/plymouthd.conf" APEX_PLYMOUTH_THEMES="$THEMES" \
    APEX_ESP_DIRS="$m/esp" sh "$WRITER"
[ ! -e "$m/esp/loader/credentials/apex.accent.cred" ] && grep -qx 'Theme=apex-os-chartreuse' "$m/etc/plymouthd.conf" \
    && ok "a grey accent removes the credential and restores the default theme" \
    || bad "a grey accent removes the credential and restores the default theme"
[ "$(boot "$m")" = "apex-os-chartreuse" ] && ok "...and the next boot is the default" \
    || bad "...and the next boot is the default (got '$(boot "$m")')"

m="$(write ABSENT)"
[ "$(boot "$m")" = "apex-os-chartreuse" ] && ok "no accent published boots the default" || bad "no accent published boots the default"

m="$(write '#fab898')"; printf 'zz' > "$m/esp/loader/credentials/apex.accent.cred"
[ "$(boot "$m")" = "apex-os-chartreuse" ] && ok "a credential that is not a bucket is ignored" || bad "a credential that is not a bucket is ignored"
printf '99' > "$m/esp/loader/credentials/apex.accent.cred"
[ "$(boot "$m")" = "apex-os-chartreuse" ] && ok "a bucket out of range is ignored" || bad "a bucket out of range is ignored"

# The UKI path: systemd-stub's copy wins over the ESP read.
grep -q '/.extra/global_credentials/apex.accent.cred' "$READER" \
    && ok "the reader takes the stub-delivered credential first (UKI path)" \
    || bad "the reader takes the stub-delivered credential first (UKI path)"

# Every bucket the writer can name is a complete theme.
missing=0
for n in $(seq 0 23); do
    t="apex-os-accent-$(printf '%02d' "$n")"
    for f in "$t.plymouth" apex-os.script comet.png glow.png spark.png flash.png; do
        [ -s "$THEMES/$t/$f" ] || missing=$((missing + 1))
    done
    grep -q "^ImageDir=/usr/share/plymouth/themes/$t$" "$THEMES/$t/$t.plymouth" || missing=$((missing + 1))
done
[ "$missing" -eq 0 ] && ok "all 24 accent themes are complete" || bad "all 24 accent themes are complete ($missing missing)"
# Every word on the splash is an Image.Text(), and Image.Text() draws nothing
# at all without a label plugin: no error, just an empty sprite. Until
# 2026-09-26 no image had one, so an encrypted machine's passphrase prompt was
# invisible. The package must be installed, and the build must refuse an
# initramfs the plugin or its font did not reach.
grep -q 'Image.Text' "$THEMES"/apex-os-chartreuse/apex-os.script \
    && grep -vE '^[[:space:]]*#' "$ROOT/Containerfile.core" \
        | grep -E 'dnf5 -y install plymouth ' | grep -qw 'plymouth-plugin-label' \
    && ok "the splash's text has a renderer: core installs plymouth-plugin-label" \
    || bad "the splash's text has a renderer: core installs plymouth-plugin-label"
_want="$(sed -n '/for want in usr\/bin\/apex-plymouth-theme/,/; do/p' "$ROOT/Containerfile.apex")"
printf '%s\n' "$_want" | grep -qF 'usr/lib64/plymouth/label-freetype.so' \
    && printf '%s\n' "$_want" | grep -qF 'usr/share/fonts/Plymouth.ttf' \
    && ok "the build asserts the label plugin and its font are in the initramfs" \
    || bad "the build asserts the label plugin and its font are in the initramfs"
# The initramfs draws with the freetype label, which drew U+00B7 as a
# missing-glyph box. Keep every message sent to the splash plain ASCII.
_nonascii="$(grep -rlE 'plymouth +message' "$ROOT/files" 2>/dev/null | grep -v '\.script$' \
    | xargs -r env LC_ALL=C grep -nP '^[^#]*--text=.*[^\x00-\x7F]' 2>/dev/null)"
[ -z "$_nonascii" ] && ok "every splash message is plain ASCII" \
    || bad "every splash message is plain ASCII: $_nonascii"
# No mid-animation switching: the theme is chosen before plymouth starts.
grep -rq 'SetUpdateStatusFunction' "$THEMES"/apex-os-accent-*/apex-os.script "$THEMES"/apex-os-chartreuse/apex-os.script \
    && bad "no theme switches colour mid-animation" || ok "no theme switches colour mid-animation"

printf '\napex plymouth-accent: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

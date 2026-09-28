#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-rime-plymouth-accent-legacy.sh — the splash accent on an ESP an APEX-OS
#  image wrote.
#
#  Every ESP in the field carries loader/credentials/apex.accent.cred, and the
#  first boot of this image starts plymouth before anything has written
#  rime.accent.cred. So the initramfs reader must take the old name when the
#  new one is absent (and prefer the new one when both exist), and the writer
#  must keep an existing old file in step — a rollback to the APEX image reads
#  only that one — without ever creating it where it is absent. A copy of the
#  reader that forgot the old name must fail the first case.
#
#  Both scripts run for real, against directories standing in for the ESP.
#  Run from anywhere: ./tests/test-rime-plymouth-accent-legacy.sh
# ─────────────────────────────────────────────────────────────────────────────
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRITER="$ROOT/files/system/libexec/rime-plymouth-accent"
READER="$ROOT/files/dracut/rime-plymouth-accent/rime-plymouth-theme"
THEMES="$ROOT/files/branding/plymouth"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/rime-accent-legacy.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
O=apex   # rime-rename: keep (the credential name APEX-OS images wrote)
pass=0 fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }
is()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected '$2', got '$3')"; fi; }

esp() {  # $1 = name, rest = file=value pairs under loader/credentials
    local m="$WORK/$1" kv; shift
    mkdir -p "$m/esp/loader/credentials" "$m/greet/accents" "$m/etc"
    for kv in "$@"; do printf '%s' "${kv#*=}" > "$m/esp/loader/credentials/${kv%%=*}"; done
    printf '%s' "$m"
}
boot() {  # $1 = machine dir, $2 = reader -> the theme the initramfs would start
    local c="$1/initrd-plymouthd.conf"
    printf '[Daemon]\nTheme=rime-os-chartreuse\n' > "$c"
    RIME_PLYMOUTH_CONF="$c" RIME_PLYMOUTH_THEMES="$THEMES" RIME_ESP_DIRS="$1/esp" sh "${2:-$READER}"
    sed -n 's/^Theme=//p' "$c"
}
publish() {  # $1 = machine dir, $2 = accent hex or ABSENT
    printf 'andre' > "$1/greet/last-user"
    rm -f "$1/greet/accents/andre"
    [ "$2" = ABSENT ] || printf '%s\n' "$2" > "$1/greet/accents/andre"
    printf '[Daemon]\nTheme=rime-os-chartreuse\n' > "$1/etc/plymouthd.conf"
    RIME_GREET_VAR="$1/greet" RIME_PLYMOUTH_CONF="$1/etc/plymouthd.conf" \
        RIME_PLYMOUTH_THEMES="$THEMES" RIME_ESP_DIRS="$1/esp" sh "$WRITER"
}
cred() { cat "$1/esp/loader/credentials/$2.accent.cred" 2>/dev/null; }

echo "── the initramfs reader ──"
m="$(esp only-old "$O.accent.cred=05")"
is "an ESP with only apex.accent.cred boots in its accent" rime-os-accent-05 "$(boot "$m")"
m="$(esp both "$O.accent.cred=05" "rime.accent.cred=14")"
is "when both exist the new name wins" rime-os-accent-14 "$(boot "$m")"
m="$(esp bad-new "$O.accent.cred=05" "rime.accent.cred=xx")"
is "an unreadable new one falls back to the old" rime-os-accent-05 "$(boot "$m")"
m="$(esp none)"
is "no credential at all keeps the default" rime-os-chartreuse "$(boot "$m")"

echo "── the writer ──"
m="$(esp upgraded "$O.accent.cred=05")"
publish "$m" '#fab898'
is "the new credential is written"                       01 "$(cred "$m" rime)"
is "an existing apex.accent.cred is kept in step"        01 "$(cred "$m" "$O")"
m="$(esp fresh)"
publish "$m" '#fab898'
is "a fresh ESP gets the new name"                       01 "$(cred "$m" rime)"
[ ! -e "$m/esp/loader/credentials/$O.accent.cred" ] && ok "…and never the old one" || bad "…and never the old one"
m="$(esp grey "$O.accent.cred=05" "rime.accent.cred=05")"
publish "$m" '#c8c8c8'
[ ! -e "$m/esp/loader/credentials/rime.accent.cred" ] && [ ! -e "$m/esp/loader/credentials/$O.accent.cred" ] \
    && ok "a grey accent removes both" || bad "a grey accent removes both"

echo "── fails both ways ──"
sed 's/for n in rime apex; do/for n in rime; do/' "$READER" > "$WORK/reader-mutant"
if cmp -s "$WORK/reader-mutant" "$READER"; then
    bad "mutant: the sed changed nothing"
else
    m="$(esp mutant "$O.accent.cred=05")"
    got="$(boot "$m" "$WORK/reader-mutant")"
    [ "$got" != rime-os-accent-05 ] && ok "mutant caught: a reader without the old name starts the default ($got)" \
        || bad "mutant: a reader without the old name still found the accent"
fi

echo
echo "rime plymouth-accent legacy: $pass passed, $fail failed"
[ "$fail" -eq 0 ]

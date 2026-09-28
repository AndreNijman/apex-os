#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-rime-rename-compat.sh — the old APEX names the image keeps as aliases:
#  files/scripts/install-rename-compat (units, /usr/libexec, /usr/share) and
#  files/system/compat/apex (the /usr/bin/apex wrapper).
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  Existing machines name the old spellings in places no image update
#  rewrites: `systemctl --user enable apex-agentd` links, labwc and Hyprland
#  configs seeded into home directories, an edited /etc/greetd/config.toml,
#  ~/.claude.json MCP entries that launch `apex mcp run`. Each alias is one
#  symlink, and a wrong one is silent: a keybind that does nothing, a unit
#  that is "not found", an MCP server the client drops because a note landed
#  on stdout. So:
#
#    * every name the installer lists becomes a relative link to a file that
#      exists, and systemd reads each unit link as an alias;
#    * every new name it points at is a file this repository ships;
#    * the installer refuses a missing target and an old path that is not its
#      alias (both are what would make the lists lie);
#    * the wrapper passes arguments, stdin and the exit status through, adds
#      NOTHING to stdout, and says its one line on stderr. A copy that prints
#      the note to stdout must fail the same check.
#
#  No root; fixture roots in a temp directory.
#
#  Run from anywhere: ./tests/test-rime-rename-compat.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")/.." || exit 2

INSTALLER=files/scripts/install-rename-compat
WRAPPER=files/system/compat/apex   # rime-rename: keep (the old command's name)
for f in "$INSTALLER" "$WRAPPER"; do [ -f "$f" ] || { echo "cannot find $f"; exit 2; }; done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rime-compat-test.XXXXXX") || exit 2
trap 'rm -rf "$WORK"' EXIT
O=apex   # rime-rename: keep

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-62s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-62s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-62s %s\n' "$1" "$2"; skip=$((skip+1)); }
is()      { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "expected $(printf '%q' "$2"), got $(printf '%q' "$3")"; fi; }

# The installer's own lists, read out of the shipped file.
# From `NAME=(` to the first line holding the closing paren, which may be the
# same line.
eval "$(awk '/^(SYSTEM_UNITS|USER_UNITS|LIBEXEC|SHARE)=\(/ { on = 1 } on { print } on && /\)/ { on = 0 }' "$INSTALLER")"
[ "${#LIBEXEC[@]}" -gt 40 ] && [ "${#SYSTEM_UNITS[@]}" -gt 10 ] \
    || { echo "could not read the installer's lists"; exit 2; }

echo "── every new name is something this repository ships ──"
missing=()
for s in "${SYSTEM_UNITS[@]}" "${USER_UNITS[@]}"; do
    [ -f "files/system/units/rime$s" ] || [ -f "rimed/rimed/rime$s" ] || missing+=("unit rime$s")
done
for s in "${LIBEXEC[@]}"; do
    case "$s" in
        "") grep -q 'mkdir -p /usr/libexec/rime/tools' Containerfile.base || missing+=("/usr/libexec/rime") ;;
        -debrand-runtime) [ -f files/scripts/rime-debrand-runtime.sh ] || missing+=("rime$s") ;;
        *) [ -f "files/system/libexec/rime$s" ] || missing+=("rime$s") ;;
    esac
done
is "the units and helpers the aliases point at exist in the tree" "" "${missing[*]:-}"

# A root with every target in place, as the image has them.
mkroot() {
    local R="$1" s
    mkdir -p "$R/usr/lib/systemd/system" "$R/usr/lib/systemd/user" "$R/usr/libexec" \
             "$R/usr/share/backgrounds/rime" "$R/usr/bin"
    for s in "${SYSTEM_UNITS[@]}"; do printf '[Unit]\nDescription=%s\n[Service]\nExecStart=/bin/true\n' "rime$s" > "$R/usr/lib/systemd/system/rime$s"; done
    for s in "${USER_UNITS[@]}";   do printf '[Unit]\nDescription=%s\n[Service]\nExecStart=/bin/true\n' "rime$s" > "$R/usr/lib/systemd/user/rime$s"; done
    for s in "${LIBEXEC[@]}"; do
        if [ -z "$s" ]; then mkdir -p "$R/usr/libexec/rime/tools"; else printf '#!/bin/sh\n' > "$R/usr/libexec/rime$s"; fi
    done
    for s in "${SHARE[@]}"; do mkdir -p "$R/usr/share/rime$s"; done
}

echo "── the installer, on a root with every target ──"
R="$WORK/root"; mkroot "$R"
out="$(bash "$INSTALLER" "$R" 2>&1)"; rc=$?
is "it exits 0" 0 "$rc"
want=$(( ${#SYSTEM_UNITS[@]} + ${#USER_UNITS[@]} + ${#LIBEXEC[@]} + ${#SHARE[@]} + 1 ))
is "it made one alias per listed name" "install-rename-compat: $want alias(es)" "$(grep -oE '^install-rename-compat: [0-9]+ alias\(es\)' <<<"$out")"

bad_links=()
check_link() {  # dir old new
    local l="$R$1/$2"
    [ -L "$l" ] && [ "$(readlink "$l")" = "$3" ] && [ -e "$l" ] || bad_links+=("$1/$2")
}
for s in "${SYSTEM_UNITS[@]}"; do check_link /usr/lib/systemd/system "$O$s" "rime$s"; done
for s in "${USER_UNITS[@]}";   do check_link /usr/lib/systemd/user   "$O$s" "rime$s"; done
for s in "${LIBEXEC[@]}";      do check_link /usr/libexec            "$O$s" "rime$s"; done
for s in "${SHARE[@]}";        do check_link /usr/share              "$O$s" "rime$s"; done
check_link /usr/share/backgrounds "$O" rime
is "every old name is a relative link that resolves to its new name" "" "${bad_links[*]:-}"
is "apexd.service is an alias of rimed.service" rimed.service "$(readlink "$R/usr/lib/systemd/system/${O}d.service")"
is "the greeter's session helper keeps its old path" rime-greet-session "$(readlink "$R/usr/libexec/$O-greet-session")"
is "/usr/share/apex-shell is the shell tree" rime-shell "$(readlink "$R/usr/share/$O-shell")"

if command -v systemctl >/dev/null 2>&1; then
    # --global is the user-unit side of the same question.
    for scope in system global; do
        flag=(); [ "$scope" = global ] && flag=(--global)
        listing="$(systemctl --root="$R" "${flag[@]}" list-unit-files --no-legend --no-pager "$O*" 2>/dev/null)"
        n=${#SYSTEM_UNITS[@]}; [ "$scope" = global ] && n=${#USER_UNITS[@]}
        is "systemd ($scope) reads every old-name unit as an alias" "alias " "$(awk '{print $2}' <<<"$listing" | sort -u | tr '\n' ' ')"
        is "…all $n of them" "$n" "$(grep -c . <<<"$listing")"
    done
else
    skipped "systemd reads every old-name unit as an alias" "systemctl is absent"
fi

out2="$(bash "$INSTALLER" "$R" 2>&1)"; rc2=$?
is "a second run exits 0" 0 "$rc2"
is "…and makes nothing new" "install-rename-compat: 0 alias(es)" "$(grep -oE '^install-rename-compat: [0-9]+ alias\(es\)' <<<"$out2")"

echo "── fails both ways: what would make the lists lie is refused ──"
R2="$WORK/root2"; mkroot "$R2"; rm -f "$R2/usr/libexec/rime-greet-session"
out3="$(bash "$INSTALLER" "$R2" 2>&1)"; rc3=$?
if [ "$rc3" != 0 ] && grep -q 'FATAL: /usr/libexec/rime-greet-session does not exist' <<<"$out3"; then
    ok "a missing target fails the build, naming it"
else bad "a missing target fails the build, naming it" "rc=$rc3: $(tail -1 <<<"$out3")"; fi
R3="$WORK/root3"; mkroot "$R3"; printf 'old\n' > "$R3/usr/libexec/$O-pkg"
out4="$(bash "$INSTALLER" "$R3" 2>&1)"; rc4=$?
if [ "$rc4" != 0 ] && grep -q "FATAL: /usr/libexec/$O-pkg already exists and is not the alias" <<<"$out4"; then
    ok "an old path that is not the alias is refused"
else bad "an old path that is not the alias is refused" "rc=$rc4: $(tail -1 <<<"$out4")"; fi
R4="$WORK/root4"; mkroot "$R4"; rm -f "$R4/usr/lib/systemd/user/rime-agentd.service"
ln -s elsewhere.service "$R4/usr/lib/systemd/user/rime-agentd.service"
bash "$INSTALLER" "$R4" >/dev/null 2>&1; rc5=$?
if [ "$rc5" != 0 ]; then ok "a unit alias to a link (not a unit file) is refused"
else bad "a unit alias to a link (not a unit file) is refused" "it exited 0"; fi

echo "── the /usr/bin/apex wrapper ──"
# A stand-in for rime: prints each argument on its own line, then stdin, then
# exits 7. The wrapper is copied with its one absolute path pointed at it.
STUB="$WORK/rime-stub"
cat > "$STUB" <<'EOF'
#!/bin/sh
for a in "$@"; do printf 'arg[%s]\n' "$a"; done
printf 'stdin[%s]\n' "$(cat)"
exit 7
EOF
chmod 0755 "$STUB"
judge_wrapper() {  # $1 = wrapper file; prints PASS/FAIL lines
    local w="$WORK/apex-under-test"
    sed "s|/usr/bin/rime|$STUB|" "$1" > "$w"; chmod 0755 "$w"
    grep -q "$STUB" "$w" || { bad "the wrapper execs /usr/bin/rime" "no /usr/bin/rime in it"; return; }
    local so se rc want
    so="$(printf 'hello' | "$w" mcp run 'two words' '' 2>"$WORK/stderr")"; rc=$?
    se="$(cat "$WORK/stderr")"
    want="$(printf 'hello' | "$STUB" mcp run 'two words' '' 2>/dev/null)"
    is "stdout is exactly rime's (nothing added: stdio MCP)" "$want" "$so"
    is "the exit status is rime's" 7 "$rc"
    is "stderr is one line naming the new command" 'apex: this command is now `rime`' "$se"
}
judge_wrapper "$WRAPPER"
is "the wrapper execs /usr/bin/rime by absolute path" 1 "$(grep -c '^exec /usr/bin/rime "\$@"$' "$WRAPPER")"

before=$fail; before_p=$pass
sed 's/>&2   # rime-rename: keep$/   # rime-rename: keep/' "$WRAPPER" > "$WORK/apex-mutant"
if cmp -s "$WORK/apex-mutant" "$WRAPPER"; then
    bad "mutant: the note on stdout" "the sed changed nothing; the mutant tests nothing"
else
    judge_wrapper "$WORK/apex-mutant" >/dev/null
    caught=$((fail - before)); fail=$before; pass=$before_p
    if [ "$caught" -gt 0 ]; then ok "mutant caught: the note on stdout ($caught checks fail)"
    else bad "mutant: the note on stdout" "every check still passed"; fi
fi

echo
echo "── $pass passed, $fail failed, $skip skipped"
[ "$fail" -eq 0 ]

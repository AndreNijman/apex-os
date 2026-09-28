#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-rime-migrate-from-apex.sh — the shipped first-boot migration,
#  files/system/libexec/rime-migrate-from-apex, run against APEX-shaped
#  fixture roots. Nothing here re-implements it: every case runs the script.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  The first boot of Rime OS on a machine that ran APEX is the one boot that
#  cannot be retried. If the package state, the secret store, the greeter's
#  memory or /etc/apex's edited files do not reach their new names, the new
#  services start on empty directories — tmpfiles creates them — and the
#  machine looks freshly installed. The file that matters most is the smallest:
#  /var/lib/apex/boot-migrate/phase, which on katana says `failed`; without it
#  rime-boot-migrate would try the systemd-boot move again.
#
#  So the suite builds the tree an APEX machine really has (paths read out of
#  the pre-rename source, 795a1face), runs the migration, and asserts every
#  move, every compatibility link, that a move is a rename (same inode) and not
#  a copy, that an edited /etc file beats the image default, that the old
#  systemd enablement comes out under the new names, and that a second run
#  changes nothing. A second fixture has tmpfiles' empty directories already in
#  place, the state a boot that skipped the migration leaves behind.
#
#  ── fails both ways ─────────────────────────────────────────────────────────
#  The last section runs broken copies of the script (no compatibility links; a
#  copy instead of a rename; the phase file's directory skipped; the /etc
#  default preferred over the user's edit) and requires each to be caught. A
#  suite that passes those is a suite that inspects nothing.
#
#  No root, no writes outside a temp directory, never the real / .
#
#  Run from anywhere: ./tests/test-rime-migrate-from-apex.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e
cd "$(dirname "$0")/.." || exit 2

SCRIPT=files/system/libexec/rime-migrate-from-apex
UNIT=files/system/units/rime-migrate-from-apex.service
for f in "$SCRIPT" "$UNIT"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rime-migrate-test.XXXXXX") || exit 2
trap 'rm -rf "$WORK"' EXIT

# The old name, spelled once, like the script does.
O=apex   # rime-rename: keep (the fixture is an APEX machine)

pass=0; fail=0
ok()  { printf 'PASS  %-62s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-62s %s\n' "$1" "$2"; fail=$((fail+1)); }
is()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "expected $(printf '%q' "$2"), got $(printf '%q' "$3")"; fi; }
yes_() { if eval "$2"; then ok "$1"; else bad "$1" "not true: $2"; fi; }
no_()  { if eval "$2"; then bad "$1" "true: $2"; else ok "$1"; fi; }
says() { if grep -qF -- "$2" <<<"$3"; then ok "$1"; else bad "$1" "no line with $(printf '%q' "$2")"; fi; }

# ── the fixture: what an APEX machine has on disk ───────────────────────────
mkfile() { mkdir -p "$(dirname "$1")"; printf '%s' "$2" > "$1"; }
mklink() { mkdir -p "$(dirname "$2")"; ln -s "$1" "$2"; }

build_fixture() {
    local R="$1"
    # /var/lib/apex: the package engine, the boot-migrate phase, lid, AI, channel.
    mkfile "$R/var/lib/$O/pkg/state.json"          '{"requested":["steam"]}'
    mkfile "$R/var/lib/$O/pkg/requested"           'steam'
    mkfile "$R/var/lib/$O/pkg/local/chrome.rpm"    'rpm-bytes'
    mkfile "$R/var/lib/$O/pkg/rollback/$O-user.raw" 'previous-extension'
    mkfile "$R/var/lib/$O/boot-migrate/phase"      'failed'
    mkfile "$R/var/lib/$O/boot-migrate/bootnum"    '0006'
    mkfile "$R/var/lib/$O/lid/state.json"          '{"lid":"open"}'
    mkfile "$R/var/lib/$O/ai/models/blobs/sha256-aa" 'model'
    mkfile "$R/var/lib/$O/channel/last-update.json" '{}'
    mkfile "$R/var/lib/$O/katana-runner-20260920/steam.tar" 'prefix'
    chmod 0700 "$R/var/lib/$O/pkg"
    # The other state directories the old image wrote.
    mkfile "$R/var/lib/$O-secretd/users/1000/store"  'sealed'
    chmod 0700 "$R/var/lib/$O-secretd" "$R/var/lib/$O-secretd/users/1000"
    mkfile "$R/var/lib/$O-greet/last-user"           'andre'
    mkfile "$R/var/lib/$O-greet/last-session"        "$O-gaming"
    mkfile "$R/var/lib/$O-greet/wallpapers/andre.jpg" 'jpeg'
    mkfile "$R/var/lib/$O-backup/ssh/id"             'key'
    mkfile "$R/var/lib/${O}os/fsync-disabled"         ''
    # Not ours to move: the sysext must keep its inner name.
    mkfile "$R/var/lib/extensions/$O-user.raw"       'squashfs'
    # A package that merely has the word in it is not ours either.
    mkfile "$R/var/lib/not-$O/x" 'x'
    # First-boot Flatpak markers and the AppImage payloads.
    mkfile "$R/var/lib/flatpak/.$O-flathub-added"    ''
    mkfile "$R/var/lib/flatpak/.$O-flatpaks-installed" ''
    mkfile "$R/var/usrlocal/lib/$O-appimage/hello/AppRun" '#!/bin/sh'

    # The new image: its sessions, its /usr/etc defaults, its units.
    mkfile "$R/usr/share/wayland-sessions/rime-gaming.desktop" '[Desktop Entry]'
    mkfile "$R/usr/etc/rime/guest-accounts"  '# default'
    mkfile "$R/usr/etc/NetworkManager/conf.d/20-rime-wifi-powersave.conf" '[connection]
wifi.powersave=2'
    mkfile "$R/usr/etc/sudoers.d/040-rime-session-select" 'default rule'
    for u in rime-lid.service rime-firewall.service rime-guest-session@.service rimed.service; do
        mkfile "$R/usr/lib/systemd/system/$u" "[Unit]"
    done
    for u in rime-agentd.service rime-remoted.service rime-aid.service; do
        mkfile "$R/usr/lib/systemd/user/$u" "[Unit]"
    done
    # The image's old-name aliases: a link is not a unit, and must not be
    # taken for one.
    mklink rime-lid.service "$R/usr/lib/systemd/system/$O-lid.service"

    # /etc after the ostree merge: the image's new defaults, plus whatever
    # the user edited or added under the old names.
    mkfile "$R/etc/rime/guest-accounts"      '# default'
    mkfile "$R/etc/$O/guest-accounts"        'guest1'
    mkfile "$R/etc/$O/lid.toml"              'policy = "stay-awake"'
    mkfile "$R/etc/$O/firewall.d/ipp.conf"   'tcp 631'
    mkfile "$R/etc/$O-greet/edition"         'gaming'
    mkfile "$R/etc/NetworkManager/conf.d/20-rime-wifi-powersave.conf" '[connection]
wifi.powersave=2'
    mkfile "$R/etc/NetworkManager/conf.d/20-$O-wifi-powersave.conf" '[connection]
wifi.powersave=3'
    mkfile "$R/etc/sudoers.d/040-rime-session-select" 'default rule'
    mkfile "$R/etc/profile.d/my-$O-stuff.sh" 'echo mine'
    # Systemd state under /etc.
    mklink "/usr/lib/systemd/system/$O-lid.service" \
           "$R/etc/systemd/system/multi-user.target.wants/$O-lid.service"
    mklink "/usr/lib/systemd/system/$O-guest-session@.service" \
           "$R/etc/systemd/system/user-runtime-dir@1042.service.wants/$O-guest-session@1042.service"
    mklink "/usr/lib/systemd/user/$O-agentd.service" \
           "$R/etc/systemd/user/default.target.wants/$O-agentd.service"
    mklink /dev/null "$R/etc/systemd/user/$O-aid.service"
    mkfile "$R/etc/systemd/system/$O-lid.service.d/override.conf" '[Service]
Nice=5'
    mkfile "$R/etc/systemd/system/$O-firewall.service" '[Unit]
Description=a full copy'
    # The owner's own unit, which only happens to carry the word.
    mkfile "$R/etc/systemd/system/$O-github-runner.service" '[Unit]'
    mklink "/etc/systemd/system/$O-github-runner.service" \
           "$R/etc/systemd/system/multi-user.target.wants/$O-github-runner.service"
}

# A listing of every path with its type, mode, link target and content hash,
# so "a second run changes nothing" is checked over the whole tree.
snapshot() {
    (cd "$1" && find . -printf '%p %y %m %l\n' | sort
     find . -type f -print0 | sort -z | xargs -0 sha256sum 2>/dev/null)
}

run() { bash "$SCRIPT" --root "$1" "${@:2}" 2>&1; }

# Every assertion about a migrated fixture, so the mutants below can be judged
# by exactly the same checks. Prints PASS/FAIL lines; the caller counts.
check_migrated() {
    local R="$1" inode_before="$2"
    echo "── /var/lib ─────────────────────────────────────────────────────────"
    is  "the boot-migrate phase is still 'failed'"          failed "$(cat "$R/var/lib/rime/boot-migrate/phase" 2>/dev/null)"
    is  "its bootnum came along"                             0006   "$(cat "$R/var/lib/rime/boot-migrate/bootnum" 2>/dev/null)"
    is  "the package state reached /var/lib/rime/pkg"        steam  "$(cat "$R/var/lib/rime/pkg/requested" 2>/dev/null)"
    yes_ "the cached local rpm came along"                   "[ -f '$R/var/lib/rime/pkg/local/chrome.rpm' ]"
    yes_ "the rollback slot kept its old extension name"     "[ -f '$R/var/lib/rime/pkg/rollback/$O-user.raw' ]"
    is  "the move is a rename: same inode"                   "$inode_before" "$(stat -c %i "$R/var/lib/rime/pkg/state.json" 2>/dev/null)"
    is  "/var/lib/rime/pkg kept mode 0700"                   700 "$(stat -c %a "$R/var/lib/rime/pkg" 2>/dev/null)"
    is  "/var/lib/apex is a link to rime"                    rime "$(readlink "$R/var/lib/$O" 2>/dev/null)"
    is  "…and reads the same phase through it"               failed "$(cat "$R/var/lib/$O/boot-migrate/phase" 2>/dev/null)"
    is  "the secret store moved"                             sealed "$(cat "$R/var/lib/rime-secretd/users/1000/store" 2>/dev/null)"
    is  "…keeping mode 0700"                                 700 "$(stat -c %a "$R/var/lib/rime-secretd" 2>/dev/null)"
    is  "apex-secretd is a link to rime-secretd"             rime-secretd "$(readlink "$R/var/lib/$O-secretd" 2>/dev/null)"
    is  "the greeter's last user moved"                      andre "$(cat "$R/var/lib/rime-greet/last-user" 2>/dev/null)"
    is  "the last session is the renamed Gaming Mode"        rime-gaming "$(cat "$R/var/lib/rime-greet/last-session" 2>/dev/null)"
    yes_ "the published wallpaper moved"                     "[ -f '$R/var/lib/rime-greet/wallpapers/andre.jpg' ]"
    yes_ "the backup SSH key moved"                          "[ -f '$R/var/lib/rime-backup/ssh/id' ]"
    yes_ "apexos became rimeos"                              "[ -f '$R/var/lib/rimeos/fsync-disabled' ] && [ -L '$R/var/lib/${O}os' ]"
    yes_ "the AI blobs and katana's runner backup came along" "[ -f '$R/var/lib/rime/ai/models/blobs/sha256-aa' ] && [ -f '$R/var/lib/rime/katana-runner-20260920/steam.tar' ]"
    yes_ "the sysext keeps its name (its inner release says apex-user)" "[ -f '$R/var/lib/extensions/$O-user.raw' ] && [ ! -e '$R/var/lib/extensions/rime-user.raw' ]"
    yes_ "a directory that only contains the word is not touched" "[ -d '$R/var/lib/not-$O' ] && [ ! -e '$R/var/lib/not-rime' ]"
    yes_ "the Flatpak markers moved, old names linked"        "[ -f '$R/var/lib/flatpak/.rime-flathub-added' ] && [ -f '$R/var/lib/flatpak/.rime-flatpaks-installed' ] && [ -L '$R/var/lib/flatpak/.$O-flatpaks-installed' ] && [ -e '$R/var/lib/flatpak/.$O-flatpaks-installed' ]"
    yes_ "the AppImage payloads moved, the old path still resolves" "[ -f '$R/var/usrlocal/lib/rime-appimage/hello/AppRun' ] && [ -f '$R/var/usrlocal/lib/$O-appimage/hello/AppRun' ] && [ -L '$R/var/usrlocal/lib/$O-appimage' ]"

    echo "── /etc ─────────────────────────────────────────────────────────────"
    is  "the edited guest-accounts beats the image default"  guest1 "$(cat "$R/etc/rime/guest-accounts" 2>/dev/null)"
    no_ "…and the default is not kept aside (/usr/etc has it)" "[ -e '$R/etc/rime/guest-accounts.rime-migrate-displaced' ]"
    is  "the user's lid.toml moved"                          'policy = "stay-awake"' "$(cat "$R/etc/rime/lid.toml" 2>/dev/null)"
    is  "the user's firewall exception moved"                'tcp 631' "$(cat "$R/etc/rime/firewall.d/ipp.conf" 2>/dev/null)"
    is  "the greeter's edition override moved"               gaming "$(cat "$R/etc/rime-greet/edition" 2>/dev/null)"
    is  "/etc/apex is a link to rime"                        rime "$(readlink "$R/etc/$O" 2>/dev/null)"
    yes_ "the edited NetworkManager drop-in took the new name" "grep -q 'wifi.powersave=3' '$R/etc/NetworkManager/conf.d/20-rime-wifi-powersave.conf'"
    no_ "…and no old-name copy is left to be read twice"     "[ -e '$R/etc/NetworkManager/conf.d/20-$O-wifi-powersave.conf' ]"
    is  "an unedited sudoers rule is left alone"             'default rule' "$(cat "$R/etc/sudoers.d/040-rime-session-select" 2>/dev/null)"
    yes_ "a profile.d file of the user's own is untouched"   "[ -f '$R/etc/profile.d/my-$O-stuff.sh' ]"

    echo "── systemd enablement ───────────────────────────────────────────────"
    is  "the user-enabled lid unit is enabled under its new name" /usr/lib/systemd/system/rime-lid.service "$(readlink "$R/etc/systemd/system/multi-user.target.wants/rime-lid.service" 2>/dev/null)"
    no_ "…and the old-name link is gone"                     "[ -L '$R/etc/systemd/system/multi-user.target.wants/$O-lid.service' ]"
    is  "an instance link points at the new template"        /usr/lib/systemd/system/rime-guest-session@.service "$(readlink "$R/etc/systemd/system/user-runtime-dir@1042.service.wants/rime-guest-session@1042.service" 2>/dev/null)"
    is  "a global user enablement moved"                     /usr/lib/systemd/user/rime-agentd.service "$(readlink "$R/etc/systemd/user/default.target.wants/rime-agentd.service" 2>/dev/null)"
    is  "a mask still masks, under the new name"             /dev/null "$(readlink "$R/etc/systemd/user/rime-aid.service" 2>/dev/null)"
    no_ "…and the old mask is gone"                          "[ -L '$R/etc/systemd/user/$O-aid.service' ]"
    is  "a drop-in applies to the renamed unit"              'Nice=5' "$(tail -1 "$R/etc/systemd/system/rime-lid.service.d/override.conf" 2>/dev/null)"
    no_ "…from its own directory"                            "[ -e '$R/etc/systemd/system/$O-lid.service.d' ]"
    yes_ "a full copy of an old unit is left where it is"   "[ -f '$R/etc/systemd/system/$O-firewall.service' ] && [ ! -e '$R/etc/systemd/system/rime-firewall.service' ]"
    yes_ "the owner's own apex-github-runner is untouched"   "[ -L '$R/etc/systemd/system/multi-user.target.wants/$O-github-runner.service' ] && [ ! -e '$R/etc/systemd/system/multi-user.target.wants/rime-github-runner.service' ]"
}

echo "== fixture 1: an APEX machine, first boot of Rime OS ====================="
R1="$WORK/r1"; build_fixture "$R1"
inode="$(stat -c %i "$R1/var/lib/$O/pkg/state.json")"
before_dry="$(snapshot "$R1")"
dry_out="$(run "$R1" --dry-run)"; dry_rc=$?
is   "a dry run exits 0"                        0 "$dry_rc"
says "a dry run says what it would do"          "would move /var/lib/$O -> /var/lib/rime" "$dry_out"
is   "a dry run changes nothing"                "$before_dry" "$(snapshot "$R1")"

out1="$(run "$R1")"; rc1=$?
is   "the migration exits 0"                    0 "$rc1"
says "it says the phase survived"               "phase is still 'failed'" "$out1"
says "it names the full unit copy it left"      "full copy of the old $O-firewall.service" "$out1"
yes_ "it keeps a record under /var/lib/rime"    "grep -q 'moved /var/lib/$O -> /var/lib/rime' '$R1/var/lib/rime/migrate-from-apex.log'"
check_migrated "$R1" "$inode"

echo "── idempotent ───────────────────────────────────────────────────────"
before2="$(snapshot "$R1")"
out2="$(run "$R1")"; rc2=$?
is   "a second run exits 0"                     0 "$rc2"
is   "a second run prints nothing but the note" "" "$(grep -v 'full copy of the old' <<<"$out2")"
is   "a second run changes nothing"             "$before2" "$(snapshot "$R1")"

echo "== fixture 2: tmpfiles ran first (a boot that skipped the migration) ====="
R2="$WORK/r2"; build_fixture "$R2"
inode2="$(stat -c %i "$R2/var/lib/$O/pkg/state.json")"
# What systemd-tmpfiles leaves: the new directories, empty.
mkdir -p "$R2/var/lib/rime/pkg/local" "$R2/var/lib/rime/appimage" "$R2/var/lib/rime/boot" \
         "$R2/var/lib/rime-greet/wallpapers"
chmod 0700 "$R2/var/lib/rime/pkg"
# …and what a service wrote into them on that boot: a clash, and a duplicate.
mkfile "$R2/var/lib/rime/lid/state.json" '{"lid":"first-boot"}'
mkfile "$R2/var/lib/rime/channel/last-update.json" '{}'
run "$R2" >/dev/null; rc3=$?
is   "merging into tmpfiles' directories exits 0" 0 "$rc3"
check_migrated "$R2" "$inode2"
is   "on a clash the machine's real state wins"  '{"lid":"open"}' "$(cat "$R2/var/lib/rime/lid/state.json" 2>/dev/null)"
is   "…and the newer file is kept, not deleted"  '{"lid":"first-boot"}' "$(cat "$R2/var/lib/rime/lid/state.json.rime-migrate-displaced" 2>/dev/null)"
no_  "an identical duplicate is simply dropped"  "[ -e '$R2/var/lib/rime/channel/last-update.json.rime-migrate-displaced' ]"
yes_ "tmpfiles' own empty directory survives"    "[ -d '$R2/var/lib/rime/boot' ]"
before4="$(snapshot "$R2")"
run "$R2" >/dev/null; is "a second run after a merge changes nothing" "$before4" "$(snapshot "$R2")"

echo "== fixture 3: the owner had moved /var/lib/apex-backup elsewhere ========="
R3="$WORK/r3"; mkdir -p "$R3/var/lib" "$R3/data/backup"; echo k > "$R3/data/backup/id"
ln -s /data/backup "$R3/var/lib/$O-backup"
run "$R3" >/dev/null; rc5=$?
is   "a relocated tree exits 0"                          0 "$rc5"
is   "the new name follows the owner's link"             /data/backup "$(readlink "$R3/var/lib/rime-backup" 2>/dev/null)"
is   "…and the old link is left as the owner made it"    /data/backup "$(readlink "$R3/var/lib/$O-backup" 2>/dev/null)"

echo "== fixture 4: a machine that never ran APEX ============================="
R4="$WORK/r4"; mkdir -p "$R4/var/lib/rime" "$R4/etc/rime" "$R4/etc/systemd/system"
before6="$(snapshot "$R4")"
out6="$(run "$R4")"; rc6=$?
is   "a fresh install exits 0"                           0 "$rc6"
is   "…says nothing"                                     "" "$out6"
is   "…and changes nothing"                              "$before6" "$(snapshot "$R4")"

echo "== the unit ==============================================================="
yes_ "ordered before systemd-tmpfiles-setup"   "grep -qE '^Before=.*\\bsystemd-tmpfiles-setup\\.service\\b' '$UNIT'"
yes_ "ordered before sysinit.target"           "grep -qE '^Before=.*\\bsysinit\\.target\\b' '$UNIT'"
yes_ "ordered before rime-firewall (no default deps there)" "grep -qE '^Before=.*\\brime-firewall\\.service\\b' '$UNIT'"
yes_ "after local-fs.target"                   "grep -qx 'After=local-fs.target' '$UNIT'"
yes_ "DefaultDependencies=no"                  "grep -qx 'DefaultDependencies=no' '$UNIT'"
yes_ "runs the shipped script"                 "grep -qx 'ExecStart=/usr/libexec/rime-migrate-from-apex' '$UNIT'"
no_  "carries no [Install]: it is enabled statically" "grep -q '^\\[Install\\]' '$UNIT'"
# Nothing may depend on it: a partial migration must never block the boot.
no_  "no other unit Requires= it" "grep -rlsE '^(Requires|BindsTo|Requisite)=.*rime-migrate-from-apex' files/system/units rimed/rimed/rimed.service"
if command -v systemd-analyze >/dev/null 2>&1; then
    va="$(cp "$UNIT" "$WORK/" && cd "$WORK" && systemd-analyze verify --man=no "./$(basename "$UNIT")" 2>&1 \
          | grep -vE 'Command .* is not executable|not found|Failed to prepare filename|No such file' || true)"
    is "systemd-analyze verify has nothing to say" "" "$va"
fi

echo "== fails both ways: broken copies must be caught =========================="
# Each mutant is a copy of the SHIPPED script with one decision broken. The
# fixture is rebuilt and the same assertions run; a mutant that passes them
# means the suite would not notice that regression.
mutant() {  # $1 = name, $2 = sed expression applied to the script
    local name="$1" expr="$2" m="$WORK/mutant.sh" R="$WORK/m" before_f
    rm -rf "$R"; build_fixture "$R"
    sed -e "$expr" "$SCRIPT" > "$m"
    if cmp -s "$m" "$SCRIPT"; then bad "mutant: $name" "the sed changed nothing; the mutant tests nothing"; return; fi
    local ino; ino="$(stat -c %i "$R/var/lib/$O/pkg/state.json")"
    bash "$m" --root "$R" >/dev/null 2>&1
    before_f=$fail; local before_p=$pass
    check_migrated "$R" "$ino" >/dev/null
    pass=$before_p
    if [ "$fail" -gt "$before_f" ]; then
        local caught=$((fail - before_f)); fail=$before_f
        ok "mutant caught: $name ($caught assertions)"
    else
        bad "mutant: $name" "every assertion still passed"
    fi
}
mutant "no compatibility links"          's/^    if ln -s -- "\$(basename -- "\$new")" "\$old"; then/    if true; then/'
mutant "copy instead of rename"          's/mv -T --no-copy -- "\$src" "\$dst" 2>\/dev\/null || { ! exists "\$dst" \&\& mv -T -- "\$src" "\$dst"; }/cp -a -- "\$src" "\$dst" \&\& rm -rf -- "\$src"/'
mutant "the old /var/lib/apex skipped"   's|^    for p in "\$lib/\$OLD"\*; do|    for p in "$lib/$OLD"-*; do|'
mutant "the image default beats the edit" 's|^            rm -f -- "\$dst" \&\& record "the edited|            rm -f -- "$src" \&\& return 1; record "the edited|'
mutant "enablement left under old names" 's/^migrate_units_scope system$/:/'

echo
echo "── $pass passed, $fail failed"
[ "$fail" -eq 0 ]

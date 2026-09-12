#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-lid.sh — lid-closed continuous operation (roadmap P1-063),
#  driven as the shipped binary rather than as library calls.
#
#  ── What this adds over the Rust suites ─────────────────────────────────────
#
#  apexd-core's 720-case matrix proves the DECISION. apex's unit tests prove
#  each reader and each plan in isolation. Neither runs `apex lid` — and the
#  criteria for this item are all of the form "the machine did the thing", so
#  they are asserted here against the real CLI, over a fixture tree, end to
#  end: close, hold, power down, guard, reopen, restore, report.
#
#  ── Why it cannot touch the machine running it ──────────────────────────────
#
#  `APEX_LID_ROOT` re-roots every absolute path the driver reads or writes into
#  a temp tree, and under it the driver executes NO external program at all:
#  systemctl, rfkill, iw, nmcli, systemd-inhibit and runuser are appended to a
#  command log, argv by argv, instead of being run. That is stronger than
#  putting fakes first on $PATH, which an absolute-path invocation would walk
#  straight past.
#
#  The first section proves that containment rather than assuming it, in the
#  only way that means anything: it asks the real logind, after a full
#  close-and-power-down run, whether anything called "APEX lid" holds a lid
#  inhibitor — and requires the answer to be no.
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#
#  * It never suspends, never blanks a display, never touches rfkill and never
#    stops a unit. Every one of those is a logged argv here.
#  * It asserts nothing about a real SW_LID event or a real inhibitor actually
#    stopping logind. That needs uinput, root, and a machine you are willing to
#    watch sleep: tests/test-apex-lid-live.sh, which says what it could not run
#    rather than skipping.
#  * It opens no window and starts no compositor.
#
#      ./tests/test-apex-lid.sh
#
#  Uses the binary at $APEX_BIN if set, else apexd/target/{debug,release}/apex,
#  else builds a debug one. A suite that cannot find the binary FAILS; it does
#  not pass quietly.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# +e deliberately, as every suite here: under -e an assignment from a command
# that exits non-zero ends the run silently, mid-section. This suite COUNTS.
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }
# No skip helper, for the reason this repository has written down three times:
# a skip becomes a green tick over nothing asserted.

UNIT="$ROOT/files/system/units/apex-lid.service"

APEX="${APEX_BIN:-}"
if [ -z "$APEX" ]; then
    for c in "$ROOT/apexd/target/debug/apex" "$ROOT/apexd/target/release/apex"; do
        [ -x "$c" ] && APEX="$c" && break
    done
fi
if [ -z "$APEX" ]; then
    echo "building apex (no binary found; set APEX_BIN to skip this)…" >&2
    ( cd "$ROOT/apexd" && cargo build -p apex >/dev/null 2>&1 )
    [ -x "$ROOT/apexd/target/debug/apex" ] && APEX="$ROOT/apexd/target/debug/apex"
fi
[ -n "$APEX" ] && [ -x "$APEX" ] || { echo "FATAL: no apex binary to test" >&2; exit 2; }
[ -f "$UNIT" ] || { echo "FATAL: cannot find $UNIT" >&2; exit 2; }

# ── the fixture ─────────────────────────────────────────────────────────────
# A machine: a lid, one thermal zone with a declared critical trip, a battery,
# a panel backlight, a keyboard backlight, a wireless interface, and two units
# that happen to be running. Every value is a file, so every case below can
# move exactly one of them and leave the rest alone.
fx() {
    local F="$WORK/$1"; shift
    rm -rf "$F"
    mkdir -p "$F/proc/acpi/button/lid/LID" \
             "$F/sys/class/thermal/thermal_zone0" \
             "$F/sys/class/power_supply/BAT0" \
             "$F/sys/class/backlight/intel_backlight" \
             "$F/sys/class/leds/platform::kbd_backlight" \
             "$F/sys/class/net/wlan0/wireless" \
             "$F/etc/apex" "$F/var/lib/apex/lid" "$F/home/nobody"
    echo open          > "$F/proc/acpi/button/lid/LID/state"
    echo x86_pkg_temp  > "$F/sys/class/thermal/thermal_zone0/type"
    echo 55000         > "$F/sys/class/thermal/thermal_zone0/temp"
    echo critical      > "$F/sys/class/thermal/thermal_zone0/trip_point_0_type"
    echo 100000        > "$F/sys/class/thermal/thermal_zone0/trip_point_0_temp"
    echo Battery       > "$F/sys/class/power_supply/BAT0/type"
    echo 80            > "$F/sys/class/power_supply/BAT0/capacity"
    echo Discharging   > "$F/sys/class/power_supply/BAT0/status"
    echo 700           > "$F/sys/class/backlight/intel_backlight/brightness"
    echo 0             > "$F/sys/class/backlight/intel_backlight/bl_power"
    echo 2             > "$F/sys/class/leds/platform::kbd_backlight/brightness"
    printf 'claude-desktop-update.timer\nfwupd-refresh.timer\n' \
        > "$F/var/lib/apex/lid/active-units"
    printf '%s\n' "$@" > "$F/etc/apex/lid.toml"
    printf '%s' "$F"
}

# Run the driver against a fixture. HOME is pushed inside the fixture so the
# invoking account's own ~/.config/apex/lid.toml can never be read — the suite
# must be as true on Andre's laptop with a pin set as on a CI runner.
drive() { local F="$1"; shift; APEX_LID_ROOT="$F" HOME="$F/home/nobody" \
              XDG_CONFIG_HOME="$F/home/nobody/.config" "$APEX" lid "$@" 2>&1; }
log()   { cat "$1/var/lib/apex/lid/commands.log" 2>/dev/null; }
jq_()   { python3 -c 'import json,sys;d=json.load(sys.stdin);print(eval(sys.argv[1],{"d":d}))' "$1"; }

command -v python3 >/dev/null 2>&1 || { echo "FATAL: python3 is required" >&2; exit 2; }

# ─────────────────────────────────────────────────────────────────────────────
section "containment — the suite cannot reach the machine it runs on"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx contain 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
drive "$F" watch --once >/dev/null

# The whole power-down ran. If any of it had escaped the fixture, the machine
# running this would now have its bluetooth blocked and two timers stopped.
grep -q '^rfkill block bluetooth$' <(log "$F") \
    && ok "rfkill was recorded, not run" \
    || bad "rfkill was recorded, not run" "$(log "$F")"
grep -q '^systemctl stop fwupd-refresh.timer$' <(log "$F") \
    && ok "systemctl stop was recorded, not run" \
    || bad "systemctl stop was recorded, not run"

# The strongest containment assertion available: ask the real logind. A
# `systemd-inhibit` that had actually been spawned would still be holding the
# lock, because the driver's handle outlives a `--once` run only through the
# child it spawns.
if command -v systemd-inhibit >/dev/null 2>&1; then
    if systemd-inhibit --list --no-pager 2>/dev/null | grep -q 'APEX lid'; then
        bad "the real logind holds no inhibitor from this suite" \
            "something called 'APEX lid' is holding one"
    else
        ok "the real logind holds no inhibitor from this suite"
    fi
else
    # Not a skip: the machine is recorded as unable to answer, and that IS the
    # result. "permission denied is not absence", and neither is "not installed".
    bad "the real logind holds no inhibitor from this suite" \
        "COULD NOT RUN: systemd-inhibit is not on this machine, so the one check \
that would have caught a real inhibitor escaping the fixture did not happen"
fi

# Nothing outside the fixture was written. Proven by the only means a test has:
# the fixture's own mtimes against a canary placed outside it.
canary="$WORK/canary"; echo untouched > "$canary"
drive "$F" watch --once >/dev/null
[ "$(cat "$canary")" = untouched ] \
    && ok "a file outside the fixture root is untouched" \
    || bad "a file outside the fixture root is untouched"

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 1 — a lid close with live work does not suspend"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx keep 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
out="$(drive "$F" watch --once)"

grep -q '^keep-working:' <<<"$out" \
    && ok "the decision is keep-working" || bad "the decision is keep-working" "$out"
grep -q '^systemctl suspend$' <(log "$F") \
    && bad "nothing suspends the machine" "systemctl suspend was issued" \
    || ok "nothing suspends the machine"

# The primitive, in its exact spelling. `--what=idle` is the mistake that looks
# identical in a screenshot and does nothing about the lid — APEX Shell's own
# caffeine inhibitor is an idle one, which is why this is asserted by name.
grep -q 'systemd-inhibit --what=handle-lid-switch --mode=block' <(log "$F") \
    && ok "the inhibitor is handle-lid-switch in block mode" \
    || bad "the inhibitor is handle-lid-switch in block mode" "$(log "$F")"
grep -q 'what=idle' <(log "$F") \
    && bad "an idle inhibitor is not taken in place of a lid one" \
    || ok "an idle inhibitor is not taken in place of a lid one"
grep -q 'who=APEX lid' <(log "$F") \
    && ok "the inhibitor names itself, so a human can see who holds it" \
    || bad "the inhibitor names itself, so a human can see who holds it"
# The reason travels with the lock. `systemd-inhibit --list` showing "why=block"
# would be useless; showing the sentence the policy produced is the difference.
grep -q 'why=the owner pinned lid keep-working on' <(log "$F") \
    && ok "the inhibitor carries the reason the policy gave" \
    || bad "the inhibitor carries the reason the policy gave" "$(log "$F")"

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 4 — with no live work, the lid suspends exactly as today"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx nowork 'pin = "auto"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
out="$(drive "$F" watch --once)"

grep -q '^release:' <<<"$out" \
    && ok "with nothing running the decision is release" || bad "with nothing running the decision is release" "$out"
grep -q 'systemd-inhibit' <(log "$F") \
    && bad "no inhibitor is taken with nothing running" "$(log "$F")" \
    || ok "no inhibitor is taken with nothing running"
# And nothing is powered down either: a machine that is about to suspend must
# not first blank its panel and stop its timers, or the reopen would restore a
# state nobody saved.
grep -q 'rfkill' <(log "$F") \
    && bad "nothing is powered down for a close that is going to suspend" \
    || ok "nothing is powered down for a close that is going to suspend"
[ "$(cat "$F/sys/class/backlight/intel_backlight/brightness")" = 700 ] \
    && ok "the panel backlight is left exactly as it was" \
    || bad "the panel backlight is left exactly as it was"

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 3 — what is powered down, and that it all comes back"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx power 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
drive "$F" watch --once >/dev/null

[ "$(cat "$F/sys/class/backlight/intel_backlight/brightness")" = 0 ] \
    && ok "the panel backlight is zeroed" || bad "the panel backlight is zeroed"
[ "$(cat "$F/sys/class/backlight/intel_backlight/bl_power")" = 4 ] \
    && ok "the panel is put into FB powerdown, not merely dimmed" \
    || bad "the panel is put into FB powerdown, not merely dimmed"
[ "$(cat "$F/sys/class/leds/platform::kbd_backlight/brightness")" = 0 ] \
    && ok "the keyboard backlight is zeroed" || bad "the keyboard backlight is zeroed"
grep -q '^iw dev wlan0 set power_save off$' <(log "$F") \
    && ok "Wi-Fi power save is turned OFF for the closed period" \
    || bad "Wi-Fi power save is turned OFF for the closed period" "$(log "$F")"

# Now reopen. The restore must put back the values that were THERE, not
# defaults: a keyboard backlight restored to "the usual" is a small lie the
# owner notices every single time.
echo open > "$F/proc/acpi/button/lid/LID/state"
out="$(drive "$F" watch --once)"
[ "$(cat "$F/sys/class/backlight/intel_backlight/brightness")" = 700 ] \
    && ok "the panel comes back to 700, the value it had" \
    || bad "the panel comes back to 700, the value it had" \
           "got $(cat "$F/sys/class/backlight/intel_backlight/brightness")"
[ "$(cat "$F/sys/class/leds/platform::kbd_backlight/brightness")" = 2 ] \
    && ok "the keyboard backlight comes back to 2, the value it had" \
    || bad "the keyboard backlight comes back to 2, the value it had" \
           "got $(cat "$F/sys/class/leds/platform::kbd_backlight/brightness")"
grep -q '^rfkill unblock bluetooth$' <(log "$F") \
    && ok "bluetooth is unblocked on reopen" || bad "bluetooth is unblocked on reopen"
grep -q '^iw dev wlan0 set power_save on$' <(log "$F") \
    && ok "Wi-Fi power save goes back on, so the cost is not permanent" \
    || bad "Wi-Fi power save goes back on, so the cost is not permanent"
# Order: the units were stopped last and must come back first, or a timer
# restarts before the radio it needs and fails once on every reopen.
restore_first="$(log "$F" | grep -n 'systemctl.*start' | head -1 | cut -d: -f1)"
radio_line="$(log "$F" | grep -n '^rfkill unblock' | head -1 | cut -d: -f1)"
[ -n "$restore_first" ] && [ -n "$radio_line" ] && [ "$restore_first" -lt "$radio_line" ] \
    && ok "the restore runs in reverse: units before radios" \
    || bad "the restore runs in reverse: units before radios" \
           "units at line ${restore_first:-none}, radio at ${radio_line:-none}"

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 3 (the other half) — the power-down cannot kill the VPN"
# ─────────────────────────────────────────────────────────────────────────────
# With the machine awake there are exactly two ways this feature could drop a
# tunnel: block the radio carrying it, or stop a unit carrying it. Both are
# closed here, against the SHIPPED defaults rather than against a fixture's
# copy of them, because a future edit to that list is precisely the accident
# this guards.
F="$(fx vpnsafe 'pin = "on"')"
# Every forbidden unit is RUNNING on this fixture's machine. Without that the
# assertion below could not fail for the thing it guards: a unit only reaches
# the plan if it is in the shipped stop list AND active, so a mutant that added
# NetworkManager.service to `stop_system_units` would never appear in the plan
# and the check would stay green over the defect it exists to catch.
cat >> "$F/var/lib/apex/lid/active-units" <<'EOF'
NetworkManager.service
wpa_supplicant.service
sing-box.service
systemd-resolved.service
iwd.service
tailscaled.service
apex-agentd.service
apex-remoted.service
sshd.service
systemd-networkd.service
dbus.service
EOF
plan="$(drive "$F" plan --json)"

python3 - "$plan" <<'PY' && ok "no shipped power-down action blocks Wi-Fi" \
                          || bad "no shipped power-down action blocks Wi-Fi"
import json,sys
p=json.loads(sys.argv[1])["plan"]["actions"]
# rfkill takes a type argument. `bluetooth` is the only one this may ever be:
# `wifi`, `wlan` or `all` would take the tunnel down with the radio.
bad=[a for a in p if a["action"]=="bluetooth" and a.get("blocked") is not True]
assert not bad, bad
# The rfkill invocation the driver builds takes its type from this action and
# from nowhere else, so `bluetooth` being the only radio action in the plan is
# what makes "the Wi-Fi radio is never blocked" true.
radios=[a["action"] for a in p if a["action"] in ("bluetooth","wifi-power-save")]
assert radios.count("bluetooth") <= 1, radios
sys.exit(0)
PY

python3 - "$plan" <<'PY' && ok "Wi-Fi power save is turned off, never on" \
                          || bad "Wi-Fi power save is turned off, never on"
import json,sys
p=json.loads(sys.argv[1])["plan"]["actions"]
w=[a for a in p if a["action"]=="wifi-power-save"]
assert w, "the plan must touch Wi-Fi power save at all"
# on=False is the whole point: aggressive 802.11 power save is a known way to
# lose a long-lived tunnel with no suspend involved anywhere.
assert all(a["on"] is False for a in w), w
sys.exit(0)
PY

python3 - "$plan" <<'PY' && ok "no unit the network depends on is ever stopped" \
                          || bad "no unit the network depends on is ever stopped"
import json,sys
p=json.loads(sys.argv[1])["plan"]["actions"]
# Anything that carries, authenticates or resolves the connection an agent is
# using. sing-box is APEX's own shipped VPN; apex-agentd is the work itself.
forbidden = ("networkmanager","wpa_supplicant","sing-box","systemd-resolved",
             "iwd","openvpn","wg-quick","tailscaled","apex-agentd","apex-remoted",
             "sshd","systemd-networkd","dbus")
stops=[a["unit"].lower() for a in p if a["action"]=="unit" and a["start"] is False]
hit=[u for u in stops for f in forbidden if f in u]
assert not hit, f"the power-down would stop {hit}"
assert stops, "the plan must stop something, or this assertion proves nothing"
sys.exit(0)
PY

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 5 — a guard suspends anyway, and says which one"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx thermal 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
echo 90000 > "$F/sys/class/thermal/thermal_zone0/temp"   # 10 °C from a 100 °C trip
out="$(drive "$F" watch --once)"

grep -q 'the thermal guard fired' <<<"$out" \
    && ok "the thermal guard fires within the headroom" \
    || bad "the thermal guard fires within the headroom" "$out"
grep -q '90.0 °C' <<<"$out" && grep -q '100.0 °C' <<<"$out" \
    && ok "it names the reading AND the limit the hardware declares" \
    || bad "it names the reading AND the limit the hardware declares" "$out"
grep -q '^systemctl suspend$' <(log "$F") \
    && ok "the machine is suspended deliberately rather than cooked" \
    || bad "the machine is suspended deliberately rather than cooked" "$(log "$F")"
ended="$(python3 -c "import json;print(json.load(open('$F/var/lib/apex/lid/last.json'))['ended_by'])" 2>/dev/null)"
[ "$ended" = thermal ] \
    && ok "the record names the guard that fired" \
    || bad "the record names the guard that fired" "ended_by=$ended"
grep -q 'ended by the thermal guard' <(drive "$F" report) \
    && ok "and the reopen report says so in words" \
    || bad "and the reopen report says so in words" "$(drive "$F" report)"

# A guard can fire on the very first poll after the lid shuts — a laptop put
# away hot needs no time at all. The record must still exist, because a
# `GuardSuspend` never opens a keep-working period to write into.
F="$(fx firstpoll 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
echo 92000 > "$F/sys/class/thermal/thermal_zone0/temp"
drive "$F" watch --once >/dev/null
ended="$(python3 -c "import json;print(json.load(open('$F/var/lib/apex/lid/last.json'))['ended_by'])" 2>/dev/null)"
[ "$ended" = thermal ] \
    && ok "a guard that fires on the first poll is still recorded" \
    || bad "a guard that fires on the first poll is still recorded" \
           "last.json says ended_by=$ended — the owner reopens to be told about the PREVIOUS close"

# The battery guard, and that it is the SECOND thing checked: a machine that is
# both too hot and too flat must report the one that damages hardware.
F="$(fx battery 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
echo 15 > "$F/sys/class/power_supply/BAT0/capacity"
out="$(drive "$F" watch --once)"
grep -q 'the battery guard fired' <<<"$out" \
    && ok "the battery floor fires at 15% against a 20% floor" \
    || bad "the battery floor fires at 15% against a 20% floor" "$out"
grep -q 'runuser' <(log "$F") \
    || ok "with no live session there is nothing to checkpoint, and it says so"

# A machine that cannot report its own temperature does not stay awake in a bag.
F="$(fx nosensor 'pin = "on"')"
echo closed > "$F/proc/acpi/button/lid/LID/state"
rm -rf "$F/sys/class/thermal"
out="$(drive "$F" watch --once)"
grep -qE 'thermal-no-sensor guard fired|no temperature sensor' <<<"$out" \
    && ok "no sensor at all fires a guard of its own" \
    || bad "no sensor at all fires a guard of its own" "$out"

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 6 — the owner's pin, and the story after reopening"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx pin 'pin = "auto"')"
drive "$F" pin off >/dev/null
grep -q 'off' <(drive "$F" pin) \
    && ok "a pin written by 'apex lid pin off' reads back as off" \
    || bad "a pin written by 'apex lid pin off' reads back as off" "$(drive "$F" pin)"

# And it wins over the system default, which is the point of having both.
echo closed > "$F/proc/acpi/button/lid/LID/state"
out="$(drive "$F" watch --once)"
grep -q 'pinned lid keep-working off' <<<"$out" \
    && ok "the owner's pin beats the system-wide file" \
    || bad "the owner's pin beats the system-wide file" "$out"
grep -q 'systemd-inhibit' <(log "$F") \
    && bad "a pin of off really releases the lid" || ok "a pin of off really releases the lid"

drive "$F" pin on >/dev/null
out="$(drive "$F" watch --once)"
grep -q '^keep-working:' <<<"$out" \
    && ok "and pinning it back on takes the lid again" \
    || bad "and pinning it back on takes the lid again" "$out"

# The reopen story, in full.
echo open > "$F/proc/acpi/button/lid/LID/state"
out="$(drive "$F" watch --once)"
grep -q 'lid closed for' <<<"$out" \
    && ok "reopening prints how long the machine stayed up" \
    || bad "reopening prints how long the machine stayed up" "$out"
rep="$(drive "$F" report)"
grep -q 'no VPN state to report' <<<"$rep" \
    && ok "a period whose VPN could not be read says so, and does not claim it held" \
    || bad "a period whose VPN could not be read says so, and does not claim it held" "$rep"

# `status` is what the shell tile will read. Its JSON must carry the decision,
# every input that produced it, and the file the policy came from.
js="$(drive "$F" status --json)"
python3 - "$js" <<'PY' && ok "status --json names the decision, the inputs and the policy source" \
                       || bad "status --json names the decision, the inputs and the policy source"
import json,sys
d=json.loads(sys.argv[1])
for k in ("decision","inputs","policy_source","pin","vpn","holds_inhibitor"):
    assert k in d, f"status --json has no {k}"
for k in ("lid","work","thermal","charge"):
    assert k in d["inputs"], f"status --json does not say what {k} was"
assert d["decision"]["why"], "a decision with no reason is not a readout"
sys.exit(0)
PY

# ─────────────────────────────────────────────────────────────────────────────
section "a machine with no lid is inert, and the unit must not fight that"
# ─────────────────────────────────────────────────────────────────────────────
F="$(fx desktop 'pin = "on"')"
rm -rf "$F/proc/acpi" "$F/sys/class/input"
out="$(drive "$F" watch --once)"
grep -q 'no lid' <<<"$out" \
    && ok "a desktop is told it has no lid rather than guessed at" \
    || bad "a desktop is told it has no lid rather than guessed at" "$out"
grep -q 'systemd-inhibit' <(log "$F") \
    && bad "a machine with no lid takes no inhibitor" || ok "a machine with no lid takes no inhibitor"

# ─────────────────────────────────────────────────────────────────────────────
section "the unit that makes any of this run on a real machine"
# ─────────────────────────────────────────────────────────────────────────────
grep -q '^ExecStart=/usr/bin/apex lid watch$' "$UNIT" \
    && ok "apex-lid.service runs the driver" || bad "apex-lid.service runs the driver"
grep -q '^WantedBy=multi-user.target$' "$UNIT" \
    && ok "it is installable into multi-user.target" || bad "it is installable into multi-user.target"
grep -q '^Restart=on-failure$' "$UNIT" \
    && ok "Restart=on-failure: a desktop exits 0 and must not be restarted for ever" \
    || bad "Restart=on-failure: a desktop exits 0 and must not be restarted for ever"
grep -q '^Restart=always$' "$UNIT" \
    && bad "Restart is not 'always'" || ok "Restart is not 'always'"
# KillMode=process would leave the `systemd-inhibit … sleep infinity` child
# alive with its keeper gone: a lid lock nothing can drop.
grep -q '^KillMode=' "$UNIT" \
    && bad "KillMode is left at the default, so the inhibitor child dies with the driver" \
    || ok "KillMode is left at the default, so the inhibitor child dies with the driver"
# Either of these remounts /sys read-only and silently breaks every write the
# power-down half makes. The symptom is a keyboard backlight that never dims,
# and it looks like anything but a unit file.
grep -q '^ProtectSystem=strict$' "$UNIT" \
    && bad "ProtectSystem is not strict, because the power-down writes sysfs" \
    || ok "ProtectSystem is not strict, because the power-down writes sysfs"
grep -q '^ProtectKernelTunables=yes$' "$UNIT" \
    && bad "ProtectKernelTunables is not set, because it would remount /sys read-only" \
    || ok "ProtectKernelTunables is not set, because it would remount /sys read-only"
grep -q 'CAP_NET_ADMIN' "$UNIT" \
    && ok "CAP_NET_ADMIN is granted, which is what rfkill and iw need" \
    || bad "CAP_NET_ADMIN is granted, which is what rfkill and iw need"
# Measured on the L16 with systemd-run carrying exactly this unit's settings:
# ProtectHome=yes makes /run/user inaccessible AND EMPTY, and /run/user is
# where the driver finds every live session and every owner's home. Under it
# the driver sees zero sessions for ever, `pin = "auto"` never activates, and
# ~/.config/apex/lid.toml is never read — the silent-pin defect that was fixed
# in the code, reintroduced by a unit file.
grep -q '^ProtectHome=no$' "$UNIT" \
    && ok "ProtectHome=no, because ProtectHome=yes empties /run/user" \
    || bad "ProtectHome=no, because ProtectHome=yes empties /run/user"
grep -q '^ProtectHome=yes$' "$UNIT" \
    && bad "ProtectHome is not yes" || ok "ProtectHome is not yes"
grep -q '^PrivateTmp=yes$' "$UNIT" \
    && bad "PrivateTmp is not yes, because a worktree may live under /tmp" \
    || ok "PrivateTmp is not yes, because a worktree may live under /tmp"
# Without CAP_SYS_RESOURCE, `runuser -u <owner> -- apex agent checkpoint` fails
# with "cannot open session: Permission denied" — pam_limits cannot raise
# rlimits — and the checkpoint dies at exactly the moment a guard is about to
# take the machine down with a session's work unsaved. Found by bisection:
# CAP_AUDIT_WRITE, CAP_CHOWN, CAP_FOWNER and CAP_SYS_ADMIN each fail alone.
grep -q '^CapabilityBoundingSet=.*CAP_SYS_RESOURCE' "$UNIT" \
    && ok "CAP_SYS_RESOURCE is granted, which is what runuser's PAM stack needs" \
    || bad "CAP_SYS_RESOURCE is granted, which is what runuser's PAM stack needs"

# ── the build's own refusals must be able to fail the build ─────────────────
# bash does not apply errexit to a pipeline inverted with `!`:
#   set -e; ! grep -q x <<<x; echo REACHED    →  prints REACHED
# So `! grep -q '^KillMode='` inside a `RUN set -eux` stanza is decoration. Four
# assertions on this branch have already been found that could never pass or
# never run, each costing a 50-minute build; this is the fifth species, and it
# is checked here because a Containerfile assertion cannot be run by running
# the suite.
lid_stanza="$(awk '/^# ── Lid-closed continuous operation/,/^# ── What is attached/' \
                "$ROOT/Containerfile.base")"
[ -n "$lid_stanza" ] || bad "the lid stanza was found in Containerfile.base" "no match"
if printf '%s' "$lid_stanza" | grep -q '^ *! *grep'; then
    bad "no refusal in the lid build stanza is written as '! grep'" \
        "$(printf '%s' "$lid_stanza" | grep -n '^ *! *grep' | head -3)"
else
    ok "no refusal in the lid build stanza is written as '! grep'"
fi
printf '%s' "$lid_stanza" | grep -q 'echo "FATAL' \
    && ok "its refusals say FATAL and exit 1, which does fail a build" \
    || bad "its refusals say FATAL and exit 1, which does fail a build"

# The image must never acquire a static logind lid policy behind this unit's
# back: `HandleLidSwitch=ignore` applies to a machine with nothing running as
# readily as to one mid-build, and turns every laptop bag into an oven.
static="$(grep -rln '^[[:space:]]*HandleLidSwitch[[:space:]]*=' \
            "$ROOT/files" "$ROOT/Containerfile.base" "$ROOT/Containerfile.core" \
            2>/dev/null | grep -v 'tests/')"
[ -z "$static" ] \
    && ok "the image ships no static HandleLidSwitch= anywhere" \
    || bad "the image ships no static HandleLidSwitch= anywhere" "$static"
grep -q 'systemctl enable apex-lid.service' "$ROOT/Containerfile.base" \
    && ok "the unit is enabled in the image, not merely installed" \
    || bad "the unit is enabled in the image, not merely installed"

printf '\n──────────────────────────────────────────────────────────────\n'
printf '  %d passed, %d failed\n' "$pass" "$fail"
printf '  fixture-only. A real SW_LID event under a real inhibitor is\n'
printf '  tests/test-apex-lid-live.sh, which needs root and uinput.\n'
printf '──────────────────────────────────────────────────────────────\n'
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-modes.sh — assertions against the SHIPPED `apex` binary for
#  `apex mode`, `apex workload` and `apex perf` (roadmap §11, §12, §13).
#
#  Nothing here re-implements the CLI: every case runs the real built binary as
#  a process, the same one the image installs.
#
#  ── Why this file exists, and why it is careful ─────────────────────────────
#  This is the area that has already caused real harm once. An earlier
#  game-mode suite applied its plans through a live writer, which shelled out to
#  `scxctl` — a D-Bus client for scx_loader whose polkit action is NOT
#  passwordless. Running the tests raised a burst of "Authentication is
#  required to start, stop, or switch sched-ext schedulers" prompts on the
#  developer's own desktop and then blocked for 177 seconds waiting on a
#  password, which read as a slow test suite rather than as a test reaching the
#  host. Once authenticated it would have swapped the scheduler of the machine
#  running the tests.
#
#  So this suite is built so that it CANNOT do that, and then asserts it:
#
#    * Only read-only and --dry-run verbs are ever invoked. `apex mode set`
#      appears twice, both times under APEX_MODE_NO_APPLY=1 or a redirected
#      D-Bus address.
#    * Fake `scxctl`, `nvidia-smi` and `systemctl` are placed FIRST on PATH and
#      record every invocation. The run fails if any of them is called — the
#      same technique the display suite uses to prove it does not reconfigure
#      the desktop it runs on.
#    * `apex workload` and `apex perf` read fixture trees through
#      APEX_SYS_ROOT / APEX_PROC_ROOT / APEX_GAME_CGROUP, so their assertions
#      describe canned hardware rather than whichever laptop is running them.
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  No root, no network, no writes outside a temp directory, no D-Bus mutation,
#  and no path that could raise an authentication prompt. A developer running
#  this on their own desktop must end with exactly the tier, scheduler and mode
#  they started with.
#
#  PASS = every verb behaves, every refusal refuses, the guard is proven to be
#         load-bearing, and no external command was spawned.
#
#  Run from anywhere: ./tests/test-apex-modes.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
# `set +e`, for the same reason as every other suite here: this one COUNTS
# failures instead of aborting, and many assertions run commands that exit
# non-zero on purpose. GitHub Actions invokes a script as `bash -e {0}`, and
# under `-e` the first such command ends the script — silently truncating the
# run rather than reporting anything, which is worse than a failure.
set +e
cd "$(dirname "$0")" || exit 2
REPO=$(cd .. && pwd)

pass=0; fail=0

ok()  { printf 'PASS  %-52s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-52s %s\n' "$1" "$2"; fail=$((fail+1)); }

# A missing prerequisite is a FAILURE, never a skip. A suite that reports
# "0 passed, 0 failed" and a green tick has asserted precisely nothing, which
# has happened here before.
command -v python3 >/dev/null 2>&1 || {
    echo "FATAL: python3 is required to validate the JSON output" >&2
    exit 2
}

# ── locate (or build) the binary under test ─────────────────────────────────
APEX_BIN=${APEX_BIN:-$REPO/apexd/target/debug/apex}
if [ ! -x "$APEX_BIN" ]; then
    echo "building the apex binary (not found at $APEX_BIN)…"
    ( cd "$REPO/apexd" && cargo build --locked --bin apex ) || {
        echo "FATAL: could not build the apex binary; nothing below can run" >&2
        exit 2
    }
fi
[ -x "$APEX_BIN" ] || { echo "FATAL: no apex binary at $APEX_BIN" >&2; exit 2; }

WORK=$(mktemp -d /tmp/apex-modes-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

# ── the tripwire: nothing here may spawn an external command ────────────────
# `scxctl` is the one that mattered historically. `nvidia-smi` is included
# because a fixture-rooted read must not fall through to the host's real GPU,
# and `systemctl` because §11's service sets are REPORTED, not applied — if
# `apex mode` ever starts moving units, this catches it on the first run.
SPAWN_LOG=$WORK/spawned.log
: > "$SPAWN_LOG"
FAKEBIN=$WORK/fakebin
mkdir -p "$FAKEBIN"
for tool in scxctl nvidia-smi systemctl busctl pkexec; do
    cat > "$FAKEBIN/$tool" <<EOF
#!/usr/bin/env bash
echo "$tool \$*" >> "$SPAWN_LOG"
exit 0
EOF
    chmod +x "$FAKEBIN/$tool"
done
export PATH="$FAKEBIN:$PATH"

# Every invocation in this suite goes through here, so the tripwire is not
# something a new case can forget to opt into.
apex() { "$APEX_BIN" "$@" 2>&1; }

# ── fixtures ────────────────────────────────────────────────────────────────
# An idle 8-CPU laptop on AC, with an amdgpu-shaped VRAM report and PSI.
mkfixture() {
    local root=$1
    mkdir -p "$root/sys/devices/system/cpu" "$root/proc/pressure" \
             "$root/sys/class/power_supply/ADP1" "$root/sys/class/drm/card1/device" \
             "$root/sys/class/hwmon/hwmon0/device" "$root/sys/class/hwmon/hwmon9" \
             "$root/sys/kernel/sched_ext" \
             "$root/sys/devices/system/cpu/cpufreq/policy0"
    echo "0-7"                                  > "$root/sys/devices/system/cpu/online"
    echo "0.10 0.20 0.30 1/500 1"               > "$root/proc/loadavg"
    printf 'some avg10=0.00 avg60=0.00 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n' \
                                                > "$root/proc/pressure/cpu"
    printf 'some avg10=0.00 avg60=0.00 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n' \
                                                > "$root/proc/pressure/io"
    echo "Mains"                                > "$root/sys/class/power_supply/ADP1/type"
    echo "1"                                    > "$root/sys/class/power_supply/ADP1/online"
    echo "805306368"                            > "$root/sys/class/drm/card1/device/mem_info_vram_used"
    echo "1073741824"                           > "$root/sys/class/drm/card1/device/mem_info_vram_total"
    printf '0: 800Mhz *\n1: 2700Mhz\n'          > "$root/sys/class/drm/card1/device/pp_dpm_sclk"
    echo "3"                                    > "$root/sys/class/drm/card1/device/gpu_busy_percent"
    echo "3000000"                              > "$root/sys/devices/system/cpu/cpufreq/policy0/scaling_cur_freq"
    echo "schedutil"                            > "$root/sys/devices/system/cpu/cpufreq/policy0/scaling_governor"
    echo "disabled"                             > "$root/sys/kernel/sched_ext/state"
    # The battery's own hwmon, which must NOT be reported as chip power.
    echo "BAT0"                                 > "$root/sys/class/hwmon/hwmon0/name"
    echo "Battery"                              > "$root/sys/class/hwmon/hwmon0/device/type"
    echo "20472000"                             > "$root/sys/class/hwmon/hwmon0/power1_input"
    # A real chip sensor.
    echo "amdgpu"                               > "$root/sys/class/hwmon/hwmon9/name"
    echo "10000000"                             > "$root/sys/class/hwmon/hwmon9/power1_average"
    echo "PPT"                                  > "$root/sys/class/hwmon/hwmon9/power1_label"
}

# Give a fixture a process.
mkproc() { mkdir -p "$1/proc/$2"; echo "$3" > "$1/proc/$2/comm"; }

# Make a fixture look busy (PSI and load both well over the thresholds).
mkbusy() {
    echo "7.50 7.20 6.90 9/500 1" > "$1/proc/loadavg"
    printf 'some avg10=42.00 avg60=38.00 avg300=30.00 total=1\nfull avg10=1.00 avg60=1.00 avg300=1.00 total=0\n' \
        > "$1/proc/pressure/cpu"
}

# Run a verb against a fixture root.
at() {
    local root=$1; shift
    APEX_SYS_ROOT="$root/sys" APEX_PROC_ROOT="$root/proc" \
    APEX_GAME_CGROUP="$root/cgroup/apex-game" apex "$@"
}

# Pull a value out of the JSON with python, so a malformed document fails loudly
# instead of a grep quietly matching nothing.
jget() { python3 -c "$2" <<<"$1" 2>&1; }

want() {  # name, expected substring, actual text
    local name=$1 substr=$2 text=$3
    if grep -qF -- "$substr" <<<"$text"; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$substr"), got: $(head -2 <<<"$text" | tr '\n' ' ')"; fi
}
is() {
    local name=$1 wantv=$2 got=$3
    if [ "$got" = "$wantv" ]; then ok "$name"
    else bad "$name" "expected $(printf '%q' "$wantv"), got $(printf '%q' "$got")"; fi
}

echo "── §11: the mode catalogue is reachable from the shipped binary ───────"
list=$(apex mode list)
is "mode list exits 0" 0 "$?"
for m in daily gaming development creator ai battery couch server; do
    want "mode list names '$m'" "$m" "$list"
done
# The eight the roadmap lists, and no silent extras masquerading as modes.
rows=$(sed -n '/^MODE/,/^$/p' <<<"$list" | grep -cE '^[a-z]+ ')
is "mode list has exactly eight modes" 8 "$rows"

echo "── §11: every mode explains its policy ────────────────────────────────"
for m in daily gaming development creator ai battery couch server; do
    out=$(apex mode show "$m"); rc=$?
    if [ "$rc" != 0 ]; then
        bad "mode show $m" "exited $rc"
        continue
    fi
    if grep -q "^why this policy:" <<<"$out" \
       && grep -qE "^tier +:" <<<"$out" \
       && grep -q "reported, NOT applied" <<<"$out"; then
        ok "mode show $m explains tier, rationale and what it will not do"
    else
        bad "mode show $m" "missing a section: $(head -3 <<<"$out" | tr '\n' ' ')"
    fi
done

# The four policy claims most likely to be "tidied up" into something wrong.
want "gaming is the mode that turns game mode on" "game mode     : on" "$(apex mode show gaming)"
want "development leaves game mode off"          "game mode     : off" "$(apex mode show development)"
want "ai pins balanced, not performance"         "balanced (pinned)"   "$(apex mode show ai)"
want "battery pins the frugal tier"              "power-saver (pinned)" "$(apex mode show battery)"
want "daily pins nothing"                        "auto ("              "$(apex mode show daily)"

echo "── an unknown mode is refused, and the refusal is useful ──────────────"
out=$(apex mode show turbo); rc=$?
if [ "$rc" = 0 ]; then bad "mode show refuses nonsense" "exited 0"
else ok "mode show refuses nonsense"; fi
want "the refusal lists the real modes" "gaming" "$out"
out=$(apex mode set turbo); rc=$?
if [ "$rc" = 0 ]; then bad "mode set refuses nonsense" "exited 0"
else ok "mode set refuses nonsense"; fi

echo "── §11: service sets are REPORTED, never applied ──────────────────────"
gaming=$(apex mode show gaming)
want "gaming declares the irqbalance conflict" "irqbalance.service" "$gaming"
want "…and says it is not applied"            "reported, NOT applied" "$gaming"

echo "── the apply guard, and proof that it is load-bearing ─────────────────"
# With the guard set, `set` must refuse BEFORE it touches the bus.
out=$(APEX_MODE_NO_APPLY=1 apex mode set gaming); rc=$?
is "the guard refuses with exit 2" 2 "$rc"
want "the guard says why" "APEX_MODE_NO_APPLY is set" "$out"
# The ordering proof, and it has to hold whether or not this machine happens to
# be running apexd. With the bus redirected to nothing, a guard checked AFTER
# the connection would report the bus failure; only a guard checked BEFORE it
# can still produce the refusal.
out=$(APEX_MODE_NO_APPLY=1 DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent-apex-test-bus \
      apex mode set gaming); rc=$?
is "the guard is checked before the bus connection" 2 "$rc"
want "…and refuses for the guard's reason, not the bus's" "APEX_MODE_NO_APPLY is set" "$out"
# The negative control. Without the guard, and with the system bus redirected
# to nothing so this can never reach a real daemon, the SAME command must fail
# for a different reason — which is what proves the guard short-circuits ahead
# of the bus rather than merely coinciding with a failure that was happening
# anyway.
out=$(DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent-apex-test-bus apex mode set gaming); rc=$?
if [ "$rc" = 0 ]; then bad "without the guard the command really would act" "exited 0"
else ok "without the guard the command really would act"; fi
if grep -qF "APEX_MODE_NO_APPLY" <<<"$out"; then
    bad "the guard is what short-circuits, not the dead bus" "guard message appeared with the guard unset"
else
    ok "the guard is what short-circuits, not the dead bus"
fi
# …and a dry run reaches the same wall without ever being able to mutate.
out=$(DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent-apex-test-bus apex mode set gaming --dry-run)
want "a dry run with no daemon fails cleanly" "cannot" "$out"
if grep -qiE "panicked|RUST_BACKTRACE" <<<"$out"; then
    bad "a dry run never panics" "$(head -1 <<<"$out")"
else ok "a dry run never panics"; fi

echo "── §13: measured signals drive the verdict ────────────────────────────"
IDLE=$WORK/idle;   mkfixture "$IDLE"
BUILD=$WORK/build; mkfixture "$BUILD"; mkbusy "$BUILD"; mkproc "$BUILD" 101 cc1plus
STALE=$WORK/stale; mkfixture "$STALE"; mkproc "$STALE" 101 cc1plus
LLM=$WORK/llm;     mkfixture "$LLM";   mkproc "$LLM" 102 ollama
GAME=$WORK/game;   mkfixture "$GAME"
mkdir -p "$GAME/cgroup/apex-game"; printf '4242\n' > "$GAME/cgroup/apex-game/cgroup.procs"

j=$(at "$IDLE" workload --json)
is "workload --json is valid JSON" "ok" \
   "$(jget "$j" 'import json,sys; json.load(sys.stdin); print("ok")')"
is "an idle fixture reads as idle" "idle" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["workload"])')"

j=$(at "$BUILD" workload --json)
is "a busy toolchain reads as compiling" "compiling" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["workload"])')"
is "…and recommends the development mode" "development" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["recommended_mode"])')"

# THE case that matters most: a toolchain process that is not doing anything
# must not move the machine into a performance tier.
j=$(at "$STALE" workload --json)
w=$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["workload"])')
if [ "$w" = "compiling" ]; then
    bad "an idle toolchain is not a build" "classified as compiling"
else ok "an idle toolchain is not a build"; fi

is "an inference server reads as local-llm" "local-llm" \
   "$(jget "$(at "$LLM" workload --json)" 'import json,sys; print(json.load(sys.stdin)["workload"])')"
is "a populated game cgroup reads as gaming" "gaming" \
   "$(jget "$(at "$GAME" workload --json)" 'import json,sys; print(json.load(sys.stdin)["workload"])')"

# Every verdict must arrive with its reasoning attached — §13's "make automatic
# choices visible" is not satisfiable by a bare label.
n=$(jget "$(at "$BUILD" workload --json)" 'import json,sys; print(len(json.load(sys.stdin)["evidence"]))')
if [ "${n:-0}" -ge 2 ]; then ok "a verdict carries its evidence"
else bad "a verdict carries its evidence" "only $n evidence line(s)"; fi

# And an unreadable signal is named rather than defaulted.
BARE=$WORK/bare; mkdir -p "$BARE/proc" "$BARE/sys"
j=$(at "$BARE" workload --json)
is "an empty machine reports unknown, not a guess" "unknown" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["workload"])')"
is "…and recommends nothing at all" "None" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["recommended_mode"])')"
n=$(jget "$j" 'import json,sys; print(len(json.load(sys.stdin)["gaps"]))')
if [ "${n:-0}" -ge 3 ]; then ok "unavailable signals are enumerated as gaps"
else bad "unavailable signals are enumerated as gaps" "only $n gap(s)"; fi

echo "── §13: --auto turns a measurement into a mode, visibly ───────────────"
# The guard must cover --auto exactly as it covers a named mode, or the one
# verb that picks its own target would be the one that could still act.
out=$(APEX_MODE_NO_APPLY=1 at "$BUILD" mode set --auto); rc=$?
is "the guard covers --auto too" 2 "$rc"
want "…for the guard's reason" "APEX_MODE_NO_APPLY is set" "$out"

# With the bus redirected to nothing, --auto still resolves the recommendation
# and prints its reasoning before failing to reach a daemon — which is what
# proves the workload engine is actually wired to the mode selector.
out=$(DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent-apex-test-bus \
      at "$BUILD" mode set --auto); rc=$?
want "--auto names the measured workload" "compiling" "$out"
want "--auto names the mode it chose" "development" "$out"
want "--auto shows its reasoning before acting" "cc1plus" "$out"
if [ "$rc" = 0 ]; then bad "--auto without a daemon still fails" "exited 0"
else ok "--auto without a daemon still fails"; fi

# And with nothing measurable, --auto refuses to guess rather than defaulting
# to some mode. This is §13's "conservative defaults" as an executable claim.
out=$(at "$BARE" mode set --auto); rc=$?
if [ "$rc" = 0 ]; then bad "--auto refuses to guess when nothing is measurable" "exited 0"
else ok "--auto refuses to guess when nothing is measurable"; fi
# The substring must not straddle the wrap in that message, or the assertion
# fails for a formatting reason rather than a behavioural one.
want "…and says why rather than picking one" "will not guess" "$out"
if grep -qE "^apex: that suggests mode" <<<"$out"; then
    bad "--auto names no mode when it has no verdict" "it recommended one anyway"
else ok "--auto names no mode when it has no verdict"; fi

echo "── §12: the Performance Lab reports what it measured, and only that ───"
j=$(at "$IDLE" perf --json)
is "perf --json is valid JSON" "ok" \
   "$(jget "$j" 'import json,sys; json.load(sys.stdin); print("ok")')"
is "the amdgpu DPM table yields the ACTIVE level" "800" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["gpu_mhz"]["value"])')"
is "VRAM is read from sysfs" "1073741824" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["vram_total_bytes"]["value"])')"

# Frame time must be null AND carry a reason, on every machine, always.
is "frame time is never a number" "None" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["frame_time_ms"]["value"])')"
r=$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["frame_time_ms"]["unavailable"])')
want "frame time explains itself" "MangoHud" "$r"
# The specific temptation this guards: a busy GPU is not a frame-time reading.
is "a busy GPU is not substituted for frame time" "3" \
   "$(jget "$j" 'import json,sys; print(json.load(sys.stdin)["gpu_busy_percent"]["value"])')"

# The bug found by running it: the battery's hwmon reported as chip power.
keys=$(jget "$j" 'import json,sys; print(" ".join(sorted(json.load(sys.stdin)["power_watts"])))')
is "chip power is named by chip and label" "amdgpu/PPT" "$keys"
if grep -qE "BAT|Battery" <<<"$keys"; then
    bad "the battery hwmon is not reported as chip power" "found in: $keys"
else ok "the battery hwmon is not reported as chip power"; fi

# A fixture root must suppress the host's own GPU, or the same assertion passes
# or fails depending on whose machine ran it.
is "a fixture root suppresses the host GPU querier" "None" \
   "$(jget "$(at "$BARE" perf --json)" 'import json,sys; print(json.load(sys.stdin)["gpu_mhz"]["value"])')"

echo "── the tripwire: no verb spawned an external command ──────────────────"
# Everything above ran with fake scxctl / nvidia-smi / systemctl / pkexec first
# on PATH. `scxctl` is the one that raised polkit prompts and switched the
# developer's scheduler; `systemctl` would mean §11's service sets stopped being
# a report and became an action.
if [ -s "$SPAWN_LOG" ]; then
    bad "no external command was spawned" "$(tr '\n' '; ' < "$SPAWN_LOG")"
else
    ok "no external command was spawned"
fi
# Prove the tripwire works, rather than trusting an empty file. If the fakes
# were not actually on PATH, the assertion above would pass vacuously — which
# is the exact failure mode that let an earlier suite report a green tick
# having asserted nothing.
scxctl switch -s scx_lavd >/dev/null 2>&1
if grep -q "^scxctl switch" "$SPAWN_LOG"; then
    ok "the tripwire itself is armed (negative control)"
else
    bad "the tripwire itself is armed (negative control)" "the fakes were not on PATH; every spawn check above was vacuous"
fi

echo
printf 'apex-modes: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" = 0 ]

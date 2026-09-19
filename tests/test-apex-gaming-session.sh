#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-gaming-session.sh — what `apex-gaming-session` actually hands to
#  gamescope, measured by RUNNING it.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  Before it, the only thing standing behind this script was `bash -n` in
#  Containerfile.apex. That is a parser, not a test: it would have accepted
#  every version of the script that produced the 2026-09-19 katana failure
#  (`ROADMAP/evidence/katana-qualification-20260919.md` §6.1 and §6.2), because
#  nothing there is a syntax error. The two defects were
#
#    * gamescope was given no device preference at all, took its default — the
#      first DRM node — and on a hybrid laptop opened the Intel iGPU, whose
#      only connector is the laptop panel. The RTX 3070 with the user's only
#      monitor on it was never touched.
#    * `--rt` was passed whenever RLIMIT_RTPRIO was non-zero. gamescope gates
#      realtime on CAP_SYS_NICE instead, logged "No CAP_SYS_NICE", and ran at
#      ordinary priority — so the flag was a claim the session could not keep.
#
#  Both are now decisions this script makes from things it can read, so both
#  can be measured on a machine with no GPU at all. That is the point: the
#  selection rule is exercised here against katana's exact sysfs shape without
#  katana, and a regression is caught in CI rather than on hardware.
#
#  ── How it cannot touch the machine ─────────────────────────────────────────
#  * `gamescope`, `steam`, `mangoapp`, `getcap` and `apex` are FAKES first on
#    PATH. The gamescope fake records its argv and exits; nothing starts.
#  * A negative control proves the fakes are really in front, because without
#    it every assertion below could pass by never running anything at all.
#  * The real `apex` binary IS used for the selection — through `APEX_ROOT`, at
#    a fixture tree — because the alternative is a fake that agrees with the
#    test rather than with the code. `$APEX_BIN`, or built with cargo.
#  * No compositor, no window, no display. `APEX_GAMING_NO_APEXD=1` keeps
#    `apex game start` out of it as well.
#
#  PASS = the flags gamescope receives are the ones the evidence measured as
#         correct, on the fixtures that reproduce each machine shape.
#
#  Run from anywhere:  ./tests/test-apex-gaming-session.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SESSION="${ROOT}/files/system/libexec/apex-gaming-session"

pass=0
fail=0
skip=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); [ -n "${2:-}" ] && printf '      %s\n' "$2"; }
skp()  { printf 'SKIP  %s\n' "$1"; skip=$((skip + 1)); }
section() { printf '\n\033[1m── %s ──\033[0m\n' "$1"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/apex-gaming-session-XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# ── the real apex binary, for the real selection rule ───────────────────────
APEX_BIN="${APEX_BIN:-}"
if [ -z "$APEX_BIN" ]; then
    if cargo build --quiet --manifest-path "${ROOT}/apexd/Cargo.toml" -p apex 2>/dev/null; then
        APEX_BIN="${ROOT}/apexd/target/debug/apex"
    fi
fi
if [ ! -x "${APEX_BIN:-/nonexistent}" ]; then
    printf 'apex-gaming-session: cannot build or find the apex binary; nothing to test\n' >&2
    exit 1
fi

# ── the fakes ───────────────────────────────────────────────────────────────
# One bin directory, reused, with the call log beside it. Every fake appends
# its own name so "nothing was spawned" is an assertion and not a hope.
BIN="${WORK}/bin"
CALLS="${WORK}/calls"
mkdir -p "$BIN"
: > "$CALLS"

make_fake() {
    local name="$1" body="${2:-}"
    cat > "${BIN}/${name}" <<FAKE
#!/usr/bin/env bash
printf '%s\n' "${name}" >> "${CALLS}"
printf '%s\n' "\$*" > "${WORK}/argv-${name}"
${body}
exit 0
FAKE
    chmod +x "${BIN}/${name}"
}

make_fake gamescope
make_fake steam
make_fake mangoapp
# getcap that reports no file capability: the shipped state of every APEX
# machine today, and the state §6.2 measured.
make_fake getcap
# `apex` is a wrapper around the REAL binary so the selection rule under test
# is the one that ships, while `apex game start` still cannot reach a bus.
cat > "${BIN}/apex" <<APEXFAKE
#!/usr/bin/env bash
printf 'apex\n' >> "${CALLS}"
exec "${APEX_BIN}" "\$@"
APEXFAKE
chmod +x "${BIN}/apex"

# ── fixtures: a sysfs tree per machine shape ────────────────────────────────
# `APEX_ROOT` is a filesystem root, so the same tree answers the readiness
# probe and the DRM topology. Only the DRM half matters here.
mkfixture() {
    local name="$1"
    local r="${WORK}/root-${name}"
    mkdir -p "${r}/proc/self" "${r}/sys/class/drm"
    printf 'Uid:\t1000\t1000\t1000\t1000\nCapEff:\t0000000000000000\nCapPrm:\t0000000000000000\nCapAmb:\t0000000000000000\n' \
        > "${r}/proc/self/status"
    printf '%s' "$r"
}

card() {  # <root> <cardN> <vendor-hex> <device-hex> <boot_vga>
    mkdir -p "$1/sys/class/drm/$2/device"
    printf '%s\n' "$3" > "$1/sys/class/drm/$2/device/vendor"
    printf '%s\n' "$4" > "$1/sys/class/drm/$2/device/device"
    printf '%s\n' "$5" > "$1/sys/class/drm/$2/device/boot_vga"
}

conn() {  # <root> <cardN> <NAME> <status>
    mkdir -p "$1/sys/class/drm/$2-$3"
    printf '%s\n' "$4" > "$1/sys/class/drm/$2-$3/status"
}

# The MSI Katana GF76, exactly as §5.1/§6.1 measured it.
KATANA="$(mkfixture katana)"
card "$KATANA" card1 0x8086 0x46a6 1
card "$KATANA" card2 0x10de 0x249d 0
conn "$KATANA" card1 eDP-1 connected
conn "$KATANA" card2 HDMI-A-1 connected

# The ThinkPad L16: one AMD card, one panel.
L16="$(mkfixture l16)"
card "$L16" card1 0x1002 0x15bf 1
conn "$L16" card1 eDP-1 connected

# A machine with nothing plugged in anywhere.
DARK="$(mkfixture dark)"
card "$DARK" card1 0x8086 0x46a6 1
conn "$DARK" card1 eDP-1 disconnected

# ── running the session ─────────────────────────────────────────────────────
# Captures the log and the argv gamescope saw. Never runs a compositor: the
# gamescope on PATH is the fake, and the session's own `command -v` finds it.
run_session() {  # <fixture-root> [extra env assignments...]
    : > "$CALLS"
    rm -f "${WORK}/argv-gamescope"
    env -i \
        PATH="${BIN}:/usr/bin:/bin" \
        HOME="${WORK}/home" \
        APEX_ROOT="$1" \
        VRR_SYS="$1/sys" \
        APEX_GAMING_NO_APEXD=1 \
        "${@:2}" \
        bash "$SESSION" > "${WORK}/out" 2> "${WORK}/log"
    printf '%s' "$?"
}

gs_argv() { cat "${WORK}/argv-gamescope" 2>/dev/null; }
session_log() { cat "${WORK}/log" 2>/dev/null; }

# ── negative control ────────────────────────────────────────────────────────
section "the fakes are really in front"

rc="$(run_session "$KATANA")"
if [ -s "${WORK}/argv-gamescope" ]; then
    ok "the session ran and the FAKE gamescope is what it reached"
else
    bad "the session ran and the FAKE gamescope is what it reached" \
        "rc=${rc}; log: $(session_log | tail -3)"
fi
# Without this, every assertion below could be measuring an empty file.
if [ "$(gs_argv)" != "" ]; then
    ok "the recorded argv is non-empty, so the assertions below measure something"
else
    bad "the recorded argv is non-empty, so the assertions below measure something"
fi

# ── §6.1: the GPU and the screen ────────────────────────────────────────────
section "Gaming Mode opens the card the monitor is on"

argv="$(gs_argv)"
# The literal pair measured on katana to move gamescope onto card2, HDMI-A-1
# and 240 Hz. `[[ == * ]]` and not `grep -q`: under pipefail a matching
# `grep -q` kills its writer with SIGPIPE and the pipeline returns 141.
if [[ "$argv" == *"--prefer-vk-device 10de:249d"* ]]; then
    ok "the NVIDIA card's PCI id is passed as --prefer-vk-device"
else
    bad "the NVIDIA card's PCI id is passed as --prefer-vk-device" "argv: ${argv}"
fi
if [[ "$argv" == *"--prefer-output HDMI-A-1"* ]]; then
    ok "the external monitor is asked for by name"
else
    bad "the external monitor is asked for by name" "argv: ${argv}"
fi
if [[ "$argv" != *"eDP-1"* ]]; then
    ok "the laptop panel is not what Gaming Mode asks for"
else
    bad "the laptop panel is not what Gaming Mode asks for" "argv: ${argv}"
fi
# The id must come from sysfs, not from a constant in the script: a machine
# that is not katana has to get its own.
# Comment lines are stripped first. The script's own header QUOTES the katana
# measurement, and a checker that read the sentence explaining the rule as a
# violation of it would train the next person to delete the explanation — the
# safe-graphics suite makes the same allowance for the same reason.
runnable_session="$(grep -vE '^[[:space:]]*#' "$SESSION")"
if [[ "$runnable_session" != *"10de:"* ]]; then
    ok "no PCI id is hardcoded in the session script"
else
    bad "no PCI id is hardcoded in the session script" \
        "$(printf '%s' "$runnable_session" | grep -n '10de:')"
fi

rc="$(run_session "$L16")"
argv="$(gs_argv)"
if [[ "$argv" == *"--prefer-vk-device 1002:15bf"* ]]; then
    ok "a single-GPU AMD machine gets its own card, with no special case"
else
    bad "a single-GPU AMD machine gets its own card, with no special case" "argv: ${argv}"
fi

rc="$(run_session "$DARK")"
argv="$(gs_argv)"
log="$(session_log)"
if [[ "$argv" != *"--prefer-vk-device"* ]]; then
    ok "with nothing connected, no device is pinned"
else
    bad "with nothing connected, no device is pinned" "argv: ${argv}"
fi
# The whole point of the fix: a fallback that nobody can see IS the defect.
if [[ "$log" == *"ERROR"* ]] && [[ "$log" == *"default"* ]]; then
    ok "and the session says so loudly instead of falling back in silence"
else
    bad "and the session says so loudly instead of falling back in silence" \
        "log: $(printf '%s' "$log" | tail -5)"
fi

section "the selection can be turned off, and says that too"

rc="$(run_session "$KATANA" APEX_GAMING_NO_DEVICE_SELECT=1)"
argv="$(gs_argv)"
log="$(session_log)"
if [[ "$argv" != *"--prefer-vk-device"* ]]; then
    ok "APEX_GAMING_NO_DEVICE_SELECT restores gamescope's own default"
else
    bad "APEX_GAMING_NO_DEVICE_SELECT restores gamescope's own default" "argv: ${argv}"
fi
if [[ "$log" == *"integrated GPU"* ]]; then
    ok "…and names the consequence rather than going quiet"
else
    bad "…and names the consequence rather than going quiet" "log: ${log}"
fi

section "a hand-set override still wins"

rc="$(run_session "$KATANA" APEX_GAMESCOPE_ARGS=--prefer-output=DP-9)"
argv="$(gs_argv)"
# gamescope takes the last occurrence of a repeated option, so the override has
# to be positioned after the selection — asserted on ORDER, because both
# appearing in the argv is exactly what a wrong order also looks like.
if [[ "$argv" == *"--prefer-output HDMI-A-1"*"--prefer-output=DP-9"* ]]; then
    ok "APEX_GAMESCOPE_ARGS comes after the computed selection"
else
    bad "APEX_GAMESCOPE_ARGS comes after the computed selection" "argv: ${argv}"
fi

# ── §6.2: realtime ──────────────────────────────────────────────────────────
section "--rt is only claimed when it can be granted"

rc="$(run_session "$KATANA")"
argv="$(gs_argv)"
log="$(session_log)"
# The fixture's CapEff is all zeroes and the fake getcap prints nothing, which
# is every APEX machine today.
if [[ "$argv" != *"--rt"* ]]; then
    ok "with no CAP_SYS_NICE, --rt is not passed"
else
    bad "with no CAP_SYS_NICE, --rt is not passed" "argv: ${argv}"
fi
if [[ "$log" == *"CAP_SYS_NICE: absent"* ]]; then
    ok "and the log says which capability is missing, not that RT was requested"
else
    bad "and the log says which capability is missing, not that RT was requested" \
        "log: $(printf '%s' "$log" | tail -6)"
fi
# The old bug in one assertion: a non-zero rlimit must no longer be enough.
if [[ "$log" != *"requesting realtime"* ]]; then
    ok "the session no longer claims realtime it cannot get"
else
    bad "the session no longer claims realtime it cannot get" "log: ${log}"
fi

# A login that holds CAP_SYS_NICE in its EFFECTIVE set — bit 23, so the hex
# ends in 800000 — is the other way the flag becomes truthful.
EFF="$(mkfixture eff)"
card "$EFF" card1 0x8086 0x46a6 1
conn "$EFF" card1 eDP-1 connected
printf 'Uid:\t1000\t1000\t1000\t1000\nCapEff:\t0000000000800000\nCapPrm:\t0000000000000000\nCapAmb:\t0000000000000000\n' \
    > "${EFF}/proc/self/status"
rc="$(run_session "$EFF")"
argv="$(gs_argv)"
log="$(session_log)"
if [[ "$argv" == *"--rt"* ]]; then
    ok "CAP_SYS_NICE in the effective set turns --rt back on"
else
    bad "CAP_SYS_NICE in the effective set turns --rt back on" "argv: ${argv}"
fi
if [[ "$log" == *"effective set"* ]]; then
    ok "…and the log says which set it came from"
else
    bad "…and the log says which set it came from" "log: ${log}"
fi

# A gamescope that DOES carry the file capability turns the flag back on with
# no change to the script — the forward path if a future RPM sets it.
cat > "${BIN}/getcap" <<'CAPFAKE'
#!/usr/bin/env bash
printf '%s cap_sys_nice=ep\n' "$1"
exit 0
CAPFAKE
chmod +x "${BIN}/getcap"
rc="$(run_session "$KATANA")"
argv="$(gs_argv)"
log="$(session_log)"
if [[ "$argv" == *"--rt"* ]]; then
    ok "a gamescope with cap_sys_nice=ep gets --rt again"
else
    bad "a gamescope with cap_sys_nice=ep gets --rt again" "argv: ${argv}"
fi
if [[ "$log" == *"file capability"* ]]; then
    ok "and the log says where the capability came from"
else
    bad "and the log says where the capability came from" "log: ${log}"
fi
make_fake getcap   # back to the shipped reality

# ── §7.2: VRR, asked about the right screen and never silently ─────────────
section "adaptive sync"

rc="$(run_session "$KATANA")"
argv="$(gs_argv)"
log="$(session_log)"
# The katana fixture has no vrr_capable anywhere, which is katana. The old code
# globbed, matched nothing, passed nothing and printed nothing — and on a 240 Hz
# monitor "this machine has no VRR" and "this driver does not publish the
# property" then looked identical.
if [[ "$argv" != *"--adaptive-sync"* ]]; then
    ok "with no vrr_capable anywhere, adaptive sync is not requested"
else
    bad "with no vrr_capable anywhere, adaptive sync is not requested" "argv: ${argv}"
fi
if [[ "$log" == *"does not say"* ]]; then
    ok "…and the log distinguishes 'the driver does not say' from 'no VRR here'"
else
    bad "…and the log distinguishes 'the driver does not say' from 'no VRR here'" \
        "log: $(printf '%s' "$log" | grep VRR)"
fi

# A machine where the CHOSEN output does advertise it.
printf '1\n' > "${KATANA}/sys/class/drm/card2-HDMI-A-1/vrr_capable"
rc="$(run_session "$KATANA")"
argv="$(gs_argv)"
if [[ "$argv" == *"--adaptive-sync"* ]]; then
    ok "an output that advertises vrr_capable=1 gets adaptive sync"
else
    bad "an output that advertises vrr_capable=1 gets adaptive sync" "argv: ${argv}"
fi

# …and the case the old global glob got wrong: VRR on the PANEL, on a session
# that is running on the monitor. Asking for adaptive sync there is asking on
# behalf of a screen this session is not using.
rm -f "${KATANA}/sys/class/drm/card2-HDMI-A-1/vrr_capable"
printf '1\n' > "${KATANA}/sys/class/drm/card1-eDP-1/vrr_capable"
rc="$(run_session "$KATANA")"
argv="$(gs_argv)"
log="$(session_log)"
if [[ "$argv" != *"--adaptive-sync"* ]]; then
    ok "VRR on a screen this session is NOT using does not turn it on"
else
    bad "VRR on a screen this session is NOT using does not turn it on" "argv: ${argv}"
fi
if [[ "$log" == *"a different screen"* ]]; then
    ok "…and the log says that is what happened"
else
    bad "…and the log says that is what happened" "log: $(printf '%s' "$log" | grep VRR)"
fi
rm -f "${KATANA}/sys/class/drm/card1-eDP-1/vrr_capable"

# ── §6.3: the capability reading that explains Steam's bwrap ────────────────
section "the capability sets are logged whatever they are"

rc="$(run_session "$KATANA")"
log="$(session_log)"
if [[ "$log" == *"capabilities: CapEff="* ]]; then
    ok "CapEff/CapPrm/CapAmb are in the session log"
else
    bad "CapEff/CapPrm/CapAmb are in the session log" "log: ${log}"
fi
# A non-empty permitted set is what makes Steam's own bwrap refuse to start.
# §6.3 recorded that message and could not attribute it; this row means the
# next run can.
PRM="$(mkfixture prm)"
card "$PRM" card1 0x8086 0x46a6 1
conn "$PRM" card1 eDP-1 connected
printf 'Uid:\t1000\t1000\t1000\t1000\nCapEff:\t0000000000000000\nCapPrm:\t0000000000800000\nCapAmb:\t0000000000000000\n' \
    > "${PRM}/proc/self/status"
rc="$(run_session "$PRM")"
log="$(session_log)"
if [[ "$log" == *"permitted set"* ]] && [[ "$log" == *"bwrap"* ]]; then
    ok "a non-empty permitted set is called out, with what it breaks"
else
    bad "a non-empty permitted set is called out, with what it breaks" \
        "log: $(printf '%s' "$log" | tail -6)"
fi

# ── the fail-safe that already worked, and must keep working ───────────────
section "the fail-safe"

: > "$CALLS"
rm -f "${BIN}/gamescope"
rc="$(env -i PATH="${BIN}:/usr/bin:/bin" HOME="${WORK}/home" APEX_ROOT="$KATANA" \
      APEX_GAMING_NO_APEXD=1 bash "$SESSION" >/dev/null 2>"${WORK}/log"; printf '%s' "$?")"
if [ "$rc" != "0" ]; then
    ok "a missing gamescope exits non-zero, so greetd re-displays the greeter"
else
    bad "a missing gamescope exits non-zero, so greetd re-displays the greeter" "rc=${rc}"
fi
if [[ "$(session_log)" == *"sudo apex install"* ]]; then
    ok "…and names the command that installs it"
else
    bad "…and names the command that installs it" "log: $(session_log)"
fi
make_fake gamescope

printf '\napex-gaming-session: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]

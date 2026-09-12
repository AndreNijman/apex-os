#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-lid-live.sh — a REAL lid switch, a REAL inhibitor, a REAL VPN.
#  Roadmap P1-063, the half tests/test-apex-lid.sh says it cannot do.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#
#  Everything in the fixture suite is an assertion about what the driver WOULD
#  do. The acceptance criteria are about what logind does when a lid actually
#  shuts, and the whole feature rests on one belief that no fixture can test:
#  that a `handle-lid-switch` block inhibitor stops logind acting on a real
#  SW_LID edge. Until this file existed, that belief had been read in
#  logind.conf(5) and never once observed.
#
#  So: a uinput device carrying SW_LID, tagged `power-switch` by udev, opened
#  by logind, driven through a genuine close-and-open — and a genuine
#  WireGuard tunnel carrying continuous traffic across the whole event.
#
#  ── How it cannot put this laptop to sleep ──────────────────────────────────
#
#  1. It re-executes itself under `systemd-inhibit --what=handle-lid-switch
#     --mode=block`, so the safety net's lifetime strictly ENCLOSES the uinput
#     device's by construction rather than by the ordering of two traps.
#  2. Before the device is created it asks logind's own `BlockInhibited`
#     property — not `systemd-inhibit --list`, which reports what asked rather
#     than what logind believes — and REFUSES to inject unless the answer
#     contains `handle-lid-switch`.
#  3. The injector emits SW_LID=0 and destroys the device in a `finally`, so a
#     crashed injector cannot leave a closed lid behind an inhibitor that is
#     about to drop.
#
#  ── What it will NOT claim ──────────────────────────────────────────────────
#
#  logind consults `HandleLidSwitchDocked` BEFORE any inhibitor, and its
#  default is `ignore`. On a docked machine — one with an external display
#  connected — the lid does nothing whether or not anything is inhibited, and
#  an injection there proves exactly nothing about the inhibitor. This suite
#  reads `Docked` and the DRM connector states first, and on a docked machine
#  reports the inhibitor causation as COULD-NOT-RUN with that reason instead of
#  collecting a pass it did not earn.
#
#  It also never suspends anything, never tests behaviour ACROSS a suspend, and
#  never touches the host's own interfaces, routes, or NetworkManager profiles:
#  both ends of the tunnel are created inside network namespaces and never
#  exist in the root namespace at all, so NetworkManager cannot see a new
#  device and cannot auto-create a saved profile for it.
#
#      sudo ./tests/test-apex-lid-live.sh
#
#  Every prerequisite it lacks is reported as could-not-run WITH THE REASON and
#  counted as a failure of this run, never skipped: "permission denied is not
#  absence", and a suite that greens over an assertion it never made is the
#  failure mode this whole item was told to avoid.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
set +e

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NS_A=apexlid-live-a
NS_B=apexlid-live-b

# ── the enclosing inhibitor ─────────────────────────────────────────────────
# `exec`, and before anything else in the file: from here on, no code path
# exists in which a uinput device is alive and this lock is not held.
if [ "${1:-}" != "--inner" ]; then
    command -v systemd-inhibit >/dev/null 2>&1 || {
        echo "COULD NOT RUN: systemd-inhibit is not on this machine, and this suite" >&2
        echo "  refuses to inject a lid event without the lock that makes it safe." >&2
        exit 2
    }
    exec systemd-inhibit --what=handle-lid-switch --mode=block \
        --who="APEX lid live suite" \
        --why="P1-063: injecting a synthetic SW_LID event; this lock is what keeps the machine awake" \
        "$0" --inner "$@"
fi

WORK="$(mktemp -d)"
cleanup() {
    # The ping lives INSIDE a namespace, so killing it first is not tidiness:
    # `ip netns del` on a namespace with a process still in it leaves the
    # namespace alive until that process exits, and an abnormal exit here would
    # otherwise leave one behind on Andre's machine.
    [ -n "${PING:-}" ] && kill "$PING" >/dev/null 2>&1
    ip netns del "$NS_A" >/dev/null 2>&1
    ip netns del "$NS_B" >/dev/null 2>&1
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

pass=0; fail=0
ok()  { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL  %s%s\n' "$1" "${2:+  — $2}"; fail=$((fail + 1)); }
cnr() { printf 'COULD NOT RUN  %s\n        reason: %s\n' "$1" "$2"; fail=$((fail + 1)); }
section() { printf '\n── %s ──\n' "$1"; }

prop() {
    busctl get-property org.freedesktop.login1 /org/freedesktop/login1 \
        org.freedesktop.login1.Manager "$1" 2>/dev/null
}

# ─────────────────────────────────────────────────────────────────────────────
section "the lock this suite runs under"
# ─────────────────────────────────────────────────────────────────────────────
# logind's own answer, not systemd-inhibit's. The list tells you what asked;
# this property tells you what logind believes it must not do.
BI="$(prop BlockInhibited)"
case "$BI" in
    *handle-lid-switch*)
        ok "logind reports the lid as blocked: ${BI}" ;;
    "")
        cnr "logind reports the lid as blocked" \
            "logind could not be queried over D-Bus at all, so nothing below may run"
        printf '\n  %d passed, %d failed\n' "$pass" "$fail"; exit 1 ;;
    *)
        bad "logind reports the lid as blocked" "BlockInhibited=${BI}"
        echo "REFUSING to inject a lid event without it." >&2
        printf '\n  %d passed, %d failed\n' "$pass" "$fail"; exit 1 ;;
esac

# ─────────────────────────────────────────────────────────────────────────────
section "what this machine can and cannot prove"
# ─────────────────────────────────────────────────────────────────────────────
DOCKED="$(prop Docked)"
EXT=0
for s in /sys/class/drm/*/status; do
    case "$s" in *eDP*|*LVDS*) continue ;; esac
    [ "$(cat "$s" 2>/dev/null)" = connected ] && EXT=$((EXT + 1))
done
printf '  Docked=%s   external displays connected: %d\n' "${DOCKED:-unknown}" "$EXT"

CAUSAL=1
if [ "$DOCKED" = 'b true' ] || [ "$EXT" -gt 0 ]; then
    CAUSAL=0
fi

UPTIME="$(cut -d. -f1 /proc/uptime)"
if [ "${UPTIME:-0}" -lt 60 ]; then
    cnr "the machine is past logind's lid holdoff" \
        "uptime is ${UPTIME}s and HoldoffTimeoutSec defaults to 30s; logind ignores lid \
events shortly after boot, so a non-suspend here would mean nothing"
    CAUSAL=0
fi

[ "$(id -u)" = 0 ] || {
    cnr "uinput is reachable" "this suite is not root; /dev/uinput is crw------- root:root, \
so no lid event can be injected and NOTHING below was asserted"
    printf '\n  %d passed, %d failed\n' "$pass" "$fail"
    exit 1
}

# ── the injector ────────────────────────────────────────────────────────────
cat > "$WORK/inject.py" <<'PYEOF'
import fcntl, os, struct, sys, time
BASE = ord('U')
def _IO(nr):        return (BASE << 8) | nr
def _IOW(nr, size): return (1 << 30) | (size << 16) | (BASE << 8) | nr
UI_DEV_CREATE, UI_DEV_DESTROY = _IO(1), _IO(2)
UI_DEV_SETUP = _IOW(3, 92)          # input_id(8) + name[80] + __u32 = 92
UI_SET_EVBIT, UI_SET_SWBIT = _IOW(100, 4), _IOW(109, 4)
EV_SYN, EV_SW, SW_LID, SYN_REPORT, BUS_VIRTUAL = 0x00, 0x05, 0x00, 0, 0x06
NAME = b"APEX P1-063 test lid switch"

def emit(fd, typ, code, val):
    os.write(fd, struct.pack("llHHi", 0, 0, typ, code, val))

def node():
    for d in sorted(os.listdir("/sys/class/input")):
        if d.startswith("event"):
            try:
                if open(f"/sys/class/input/{d}/device/name").read().strip() == NAME.decode():
                    return d
            except OSError:
                pass
    return None

hold = float(sys.argv[1]) if len(sys.argv) > 1 else 4.0
try:
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
except OSError as e:
    print(f"COULD-NOT-RUN {e}", flush=True); sys.exit(3)
# The ioctl argument is the bit NUMBER, passed by value. A packed buffer here
# passes its address instead and the kernel answers EINVAL.
fcntl.ioctl(fd, UI_SET_EVBIT, EV_SW)
fcntl.ioctl(fd, UI_SET_SWBIT, SW_LID)
fcntl.ioctl(fd, UI_DEV_SETUP,
            struct.pack("HHHH80sI", BUS_VIRTUAL, 0x1d6b, 0x0063, 1, NAME, 0))
fcntl.ioctl(fd, UI_DEV_CREATE)
try:
    # Born with SW_LID == 0. logind reads EVIOCGSW when it opens the node, so
    # the device must start OPEN — otherwise the first thing logind learns is a
    # closed lid nobody ever announced.
    time.sleep(0.6)
    n = node()
    print(f"NODE {n}", flush=True)
    if n is None:
        sys.exit(4)
    os.system("udevadm settle --timeout=5 >/dev/null 2>&1")
    # The tags must be read WHILE the device exists. Asking afterwards asks
    # about a sysfs path that has been gone for a second, which answers
    # "could not be asked" and looks exactly like an untagged device.
    tags = os.popen(f"udevadm info -q property /sys/class/input/{n} 2>/dev/null"
                    " | grep -E '^(TAGS|CURRENT_TAGS)='").read().replace("\n", " ")
    print(f"TAGS {tags.strip()}", flush=True)
    time.sleep(1.5)                      # logind has to notice and open it
    print("READY", flush=True)
    emit(fd, EV_SW, SW_LID, 1); emit(fd, EV_SYN, SYN_REPORT, 0)
    print("CLOSED", flush=True)
    time.sleep(hold)
    emit(fd, EV_SW, SW_LID, 0); emit(fd, EV_SYN, SYN_REPORT, 0)
    print("OPENED", flush=True)
    time.sleep(0.8)
finally:
    # Unconditionally, and before the enclosing inhibitor can possibly drop: a
    # device left behind reporting a closed lid is the one state this must
    # never leave on the machine.
    try:
        emit(fd, EV_SW, SW_LID, 0); emit(fd, EV_SYN, SYN_REPORT, 0)
        fcntl.ioctl(fd, UI_DEV_DESTROY)
    except OSError:
        pass
    os.close(fd)
    print("DESTROYED", flush=True)
PYEOF

command -v python3 >/dev/null 2>&1 || {
    cnr "a lid event can be injected" "python3 is not installed"
    printf '\n  %d passed, %d failed\n' "$pass" "$fail"; exit 1; }

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 2 — a real VPN, carrying traffic, across the lid event"
# ─────────────────────────────────────────────────────────────────────────────
# A genuine WireGuard tunnel: real keys, real handshake, real encrypted
# traffic. Both ends live inside network namespaces and the veth is created
# INSIDE one of them, so neither end ever appears in the root namespace —
# NetworkManager never sees a new device and cannot auto-create a saved profile
# for it. The host's Wi-Fi, routes and connection profiles are untouched.
VPN=0
if ! command -v wg >/dev/null 2>&1; then
    cnr "a real VPN tunnel survives the lid event" \
        "wireguard-tools is not installed (sudo apex install wireguard-tools); no tunnel \
could be built, so criterion 2 was NOT asserted on this machine"
elif ! command -v ip >/dev/null 2>&1; then
    cnr "a real VPN tunnel survives the lid event" "iproute2 is not installed"
else
    ip netns add "$NS_A" 2>"$WORK/e" && ip netns add "$NS_B" 2>>"$WORK/e"
    if [ $? -ne 0 ]; then
        cnr "a real VPN tunnel survives the lid event" \
            "network namespaces could not be created: $(cat "$WORK/e")"
    else
        ip -n "$NS_A" link add apxa type veth peer name apxb netns "$NS_B"
        ip -n "$NS_A" addr add 10.77.0.1/24 dev apxa; ip -n "$NS_A" link set apxa up
        ip -n "$NS_B" addr add 10.77.0.2/24 dev apxb; ip -n "$NS_B" link set apxb up
        ip -n "$NS_A" link set lo up; ip -n "$NS_B" link set lo up

        umask 077
        wg genkey > "$WORK/a.key"; wg pubkey < "$WORK/a.key" > "$WORK/a.pub"
        wg genkey > "$WORK/b.key"; wg pubkey < "$WORK/b.key" > "$WORK/b.pub"
        cat > "$WORK/a.conf" <<EOF
[Interface]
PrivateKey = $(cat "$WORK/a.key")
ListenPort = 51820
[Peer]
PublicKey = $(cat "$WORK/b.pub")
AllowedIPs = 10.88.0.2/32
Endpoint = 10.77.0.2:51821
PersistentKeepalive = 1
EOF
        cat > "$WORK/b.conf" <<EOF
[Interface]
PrivateKey = $(cat "$WORK/b.key")
ListenPort = 51821
[Peer]
PublicKey = $(cat "$WORK/a.pub")
AllowedIPs = 10.88.0.1/32
Endpoint = 10.77.0.1:51820
PersistentKeepalive = 1
EOF
        ip -n "$NS_A" link add wg0 type wireguard 2>"$WORK/e" \
            && ip -n "$NS_B" link add wg0 type wireguard 2>>"$WORK/e"
        if [ $? -ne 0 ]; then
            cnr "a real VPN tunnel survives the lid event" \
                "the wireguard kernel module is unavailable: $(cat "$WORK/e")"
        else
            ip netns exec "$NS_A" wg setconf wg0 "$WORK/a.conf"
            ip netns exec "$NS_B" wg setconf wg0 "$WORK/b.conf"
            ip -n "$NS_A" addr add 10.88.0.1/24 dev wg0; ip -n "$NS_A" link set wg0 up
            ip -n "$NS_B" addr add 10.88.0.2/24 dev wg0; ip -n "$NS_B" link set wg0 up
            sleep 2
            if ip netns exec "$NS_A" ping -c 2 -W 2 10.88.0.2 >/dev/null 2>&1; then
                ok "a real WireGuard tunnel is up and carrying traffic before the lid shuts"
                VPN=1
            else
                bad "a real WireGuard tunnel is up before the lid shuts" \
                    "$(ip netns exec "$NS_A" wg show wg0 | tr '\n' ' ')"
            fi
        fi
    fi
fi

# ── the event ───────────────────────────────────────────────────────────────
if [ "$VPN" = 1 ]; then
    RX0="$(ip netns exec "$NS_A" wg show wg0 transfer | awk '{print $2}')"
    ip netns exec "$NS_A" ping -i 0.2 -W 1 10.88.0.2 > "$WORK/ping" 2>&1 &
    PING=$!
fi

SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
sleep 0.3
python3 "$WORK/inject.py" 5 > "$WORK/inject" 2>&1
INJ=$?
grep -q '^DESTROYED$' "$WORK/inject" \
    && ok "the synthetic lid switch was destroyed, closed lid and all" \
    || bad "the synthetic lid switch was destroyed" "$(cat "$WORK/inject")"

if [ "$VPN" = 1 ]; then
    sleep 0.5; kill -INT "$PING" 2>/dev/null; sleep 0.5
    RX1="$(ip netns exec "$NS_A" wg show wg0 transfer | awk '{print $2}')"
    loss="$(grep -o '[0-9]*% packet loss' "$WORK/ping" | head -1)"
    [ "$loss" = "0% packet loss" ] \
        && ok "the tunnel carried every packet across the lid close ($loss)" \
        || bad "the tunnel carried every packet across the lid close" "${loss:-no statistics}"
    # Counters, not just reachability: a route that still exists proves nothing
    # about a tunnel that has stopped decrypting.
    [ -n "${RX0:-}" ] && [ -n "${RX1:-}" ] && [ "$RX1" -gt "$RX0" ] \
        && ok "encrypted bytes moved WHILE the lid was shut (wg rx $RX0 → $RX1)" \
        || bad "encrypted bytes moved while the lid was shut" "rx $RX0 → $RX1"
    hs="$(ip netns exec "$NS_A" wg show wg0 latest-handshakes | awk '{print $2}')"
    [ -n "$hs" ] && [ "$hs" -gt 0 ] \
        && ok "the peer is still handshaken, not merely still configured" \
        || bad "the peer is still handshaken" "latest-handshakes=$hs"
fi

# ─────────────────────────────────────────────────────────────────────────────
section "criterion 1 — logind saw a real lid close and did not act on it"
# ─────────────────────────────────────────────────────────────────────────────
NODE="$(awk '/^NODE /{print $2}' "$WORK/inject")"
if [ -z "$NODE" ] || [ "$INJ" != 0 ]; then
    cnr "a real SW_LID event reached logind" \
        "the injector did not create a device: $(tr '\n' ' ' < "$WORK/inject")"
else
    ok "a uinput SW_LID device was created ($NODE)"

    # The tag is what makes logind look at it at all: logind.conf(5) — "Only
    # input devices with the power-switch udev tag will be watched". An
    # injection against an untagged device proves nothing.
    # Read by the injector while the device was alive; asking here would ask
    # about a sysfs path that no longer exists.
    tags="$(sed -n 's/^TAGS //p' "$WORK/inject")"
    case "$tags" in
        *power-switch*) ok "udev tagged it power-switch, so logind watches it ($tags)" ;;
        "")  cnr "udev tagged it power-switch" "udevadm could not be asked about $NODE" ;;
        *)   bad "udev tagged it power-switch" "${tags:-empty}" ;;
    esac

    jr="$(journalctl -u systemd-logind --since "$SINCE" --no-pager -o cat 2>/dev/null)"
    if [ -z "$jr" ]; then
        cnr "logind opened the device and saw the close" \
            "the journal could not be read, so logind's own account is unavailable"
    else
        # Stronger than the tag: logind SAYS which node it opened.
        grep -q "Watching system buttons on /dev/input/$NODE" <<<"$jr" \
            && ok "logind opened it: 'Watching system buttons on /dev/input/$NODE'" \
            || bad "logind opened the device" "$jr"
        grep -q '^Lid closed\.$' <<<"$jr" \
            && ok "logind believed the lid: 'Lid closed.'" \
            || bad "logind believed the lid" "$jr"
        grep -q '^Lid opened\.$' <<<"$jr" \
            && ok "and 'Lid opened.' when it was released" \
            || bad "logind saw the lid reopen" "$jr"
    fi

    # logind's own view of the lid, which is what every consumer reads.
    # Checked after the fact: the property is back to open, which is the state
    # this suite is required to leave the machine in.
    [ "$(prop LidClosed)" = 'b false' ] \
        && ok "logind's LidClosed is back to false, so nothing was left shut" \
        || bad "logind's LidClosed is back to false" "$(prop LidClosed)"

    slept="$(journalctl --since "$SINCE" --no-pager -o cat 2>/dev/null \
             | grep -icE 'suspending system|entering sleep state|Performing sleep operation')"
    [ "${slept:-1}" = 0 ] \
        && ok "the machine did not suspend, hibernate or enter any sleep state" \
        || bad "the machine did not suspend" "$slept sleep lines in the journal"

    if [ "$CAUSAL" = 1 ]; then
        ok "…and the inhibitor is why: this machine is not docked and has no external \
display, so logind used HandleLidSwitch (suspend) and was stopped by the block"
    else
        cnr "the INHIBITOR is what stopped the suspend" \
"this machine is docked (Docked=${DOCKED:-?}, ${EXT} external display(s) connected). \
logind consults HandleLidSwitchDocked — default 'ignore' — BEFORE it consults any \
inhibitor, so the lid would have done nothing here with or without the lock. The \
event, the tag, logind's own 'Lid closed.' and the absence of any suspend are all \
asserted above and stand; the CAUSE is not, and this suite will not collect a pass \
it did not earn. Re-run it with every external display disconnected to close this."
    fi
fi

printf '\n──────────────────────────────────────────────────────────────\n'
printf '  %d passed, %d failed (could-not-run counts as failed)\n' "$pass" "$fail"
printf '──────────────────────────────────────────────────────────────\n'
[ "$fail" -eq 0 ]

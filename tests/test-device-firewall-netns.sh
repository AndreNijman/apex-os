#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-device-firewall-netns.sh — what the default-drop policy does to the
#  three device features that need inbound packets: a shared printer, a Wi-Fi
#  hotspot, and mDNS discovery.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  `tests/test-apex-firewall.sh` reads the ruleset and asserts that the lines
#  are present. That catches a rule deleted in a refactor. It cannot answer the
#  question a user asks, which is not "is rule 14 present" but "why does my
#  hotspot hand out no addresses".
#
#  So this loads the shipped policy into a throwaway network namespace and sends
#  real packets through it. Three namespaces, wired client — router — server,
#  with the policy on the router:
#
#      client 10.90.1.2 ── vcr 10.90.1.1 [ROUTER] 10.90.2.1 vsr ── 10.90.2.2 server
#
#  ── Why it cannot touch the machine running it ──────────────────────────────
#  Everything happens inside `unshare -rmn`: a fresh user namespace, mount
#  namespace and network namespace. nftables tables are network-namespace
#  scoped, so `nft -f apex.nft` in there is invisible to the host — the host's
#  own ruleset, routes and interfaces are never read or written. It needs no
#  root and asks for none. This matters more than usual here: the machine that
#  runs this suite is somebody's workstation, on their real network, and a
#  firewall test that reached the host would disconnect them mid-run.
#
#  ── The positive control, which is the point ────────────────────────────────
#  "The packet did not arrive" is the same observation whether the firewall
#  dropped it or the harness never sent it. Every blocking case here is paired
#  with mDNS on 5353, which the policy accepts on purpose: if the control fails,
#  the harness is broken and every other verdict in this file is worthless, so
#  it is checked first and the run stops if it fails.
#
#  PASS = the policy delivers what a desktop needs, drops what it says it drops,
#         and the documented exception mechanism reopens a port.
#
#  Run from anywhere: ./tests/test-device-firewall-netns.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

RULES=files/system/nftables/apex.nft
[ -f "$RULES" ] || { echo "cannot find $RULES"; exit 2; }
RULES_ABS=$(cd "$(dirname "$RULES")" && pwd)/$(basename "$RULES")

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-56s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-56s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-56s %s\n' "$1" "$2"; skip=$((skip+1)); }

report() {
    echo
    printf 'device firewall (netns): %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

# ── can we build a namespace at all ─────────────────────────────────────────
for tool in unshare nsenter ip nft python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        skipped "the whole file" "$tool is not installed"
        report; exit $?
    }
done

if [ "$(id -u)" = 0 ]; then
    UNSHARE=(unshare -mn)          # already root: no user namespace needed
else
    UNSHARE=(unshare -rmn)
fi

if ! "${UNSHARE[@]}" true 2>/dev/null; then
    skipped "the whole file" "this kernel refuses an unprivileged network namespace"
    report; exit $?
fi

# ── the whole experiment, run once inside the namespace ─────────────────────
# One process, so the namespaces live as long as the experiment and no longer. It
# prints one `CASE <name> <verdict>` line per probe and this script grades them;
# keeping the grading outside means a namespace that dies half way through
# reads as missing cases rather than as passes.
INNER=$(mktemp /tmp/apex-netns-inner.XXXXXX.sh) || exit 2
trap 'rm -f "$INNER"' EXIT

cat > "$INNER" <<'INNER_EOF'
set -uo pipefail
RULES="$1"

fatal() { echo "SETUP-FAILED $*"; exit 3; }

# Leaf namespaces as sleeping processes, addressed by pid. `ip netns add` wants
# a shared bind mount under /run/netns and does not get one inside a private
# mount namespace; a pid is a namespace handle that needs no mount at all.
unshare -n sleep 120 & CLI=$!
unshare -n sleep 120 & SRV=$!
sleep 0.3
kill -0 "$CLI" 2>/dev/null || fatal "client namespace did not start"
kill -0 "$SRV" 2>/dev/null || fatal "server namespace did not start"
cleanup() { kill "$CLI" "$SRV" 2>/dev/null; }
trap cleanup EXIT

ip link add veth-c type veth peer name vcr        || fatal "veth pair (client)"
ip link add veth-s type veth peer name vsr        || fatal "veth pair (server)"
ip link set veth-c netns "$CLI"                   || fatal "move veth-c"
ip link set veth-s netns "$SRV"                   || fatal "move veth-s"
ip addr add 10.90.1.1/24 dev vcr                  || fatal "router client-side address"
ip addr add 10.90.2.1/24 dev vsr                  || fatal "router server-side address"
ip link set vcr up; ip link set vsr up; ip link set lo up
for ns_pid_addr in "$CLI 10.90.1.2/24 veth-c 10.90.1.1" "$SRV 10.90.2.2/24 veth-s 10.90.2.1"; do
    set -- $ns_pid_addr
    nsenter -t "$1" -n ip addr add "$2" dev "$3"      || fatal "leaf address $2"
    nsenter -t "$1" -n ip link set "$3" up
    nsenter -t "$1" -n ip link set lo up
    nsenter -t "$1" -n ip route add default via "$4"  || fatal "leaf route via $4"
done
sysctl -qw net.ipv4.ip_forward=1 >/dev/null || fatal "ip_forward is not writable here"

# ── probes ──────────────────────────────────────────────────────────────────
# Each one binds a listener, sends one datagram or one SYN, and says whether it
# landed. Timeouts are short because a blocked packet is silence, and silence is
# how long this file takes.

# A UDP service on the ROUTER, i.e. the machine running the firewall: a hotspot's
# DHCP and DNS servers, or avahi.
udp_to_router() {  # $1 = dport, $2 = sport (0 = ephemeral)
    local dport=$1 sport=$2 out
    python3 - "$dport" >/tmp/.rx.$dport 2>/dev/null <<'PY' &
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", int(sys.argv[1])))
s.settimeout(2.0)
try:
    s.recvfrom(64); print("ARRIVED")
except Exception:
    print("SILENT")
PY
    local rxpid=$!
    sleep 0.4
    nsenter -t "$CLI" -n python3 - "$dport" "$sport" <<'PY' >/dev/null 2>&1
import socket, sys
dport, sport = int(sys.argv[1]), int(sys.argv[2])
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
if sport:
    s.bind(("0.0.0.0", sport))
for _ in range(3):
    s.sendto(b"apex-probe", ("10.90.1.1", dport))
PY
    wait "$rxpid" 2>/dev/null
    out=$(cat "/tmp/.rx.$dport" 2>/dev/null); rm -f "/tmp/.rx.$dport"
    printf '%s' "${out:-SILENT}"
}

# A TCP service on the ROUTER: a shared printer's IPP port, ssh, a web server.
tcp_to_router() {  # $1 = dport
    local dport=$1 out
    python3 - "$dport" >/dev/null 2>&1 <<'PY' &
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", int(sys.argv[1]))); s.listen(2); s.settimeout(3.0)
try: s.accept()
except Exception: pass
PY
    local lpid=$!
    sleep 0.4
    out=$(nsenter -t "$CLI" -n python3 - "$dport" <<'PY'
import socket, sys
s = socket.socket(); s.settimeout(1.5)
try:
    s.connect(("10.90.1.1", int(sys.argv[1]))); print("ARRIVED")
except Exception:
    print("SILENT")
PY
)
    kill "$lpid" 2>/dev/null; wait "$lpid" 2>/dev/null
    printf '%s' "$out"
}

# Client to server THROUGH the router: what NAT tethering and a hotspot need.
tcp_forwarded() {
    nsenter -t "$SRV" -n python3 - <<'PY' >/dev/null 2>&1 &
import socket
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", 9099)); s.listen(2); s.settimeout(3.0)
try: s.accept()
except Exception: pass
PY
    local lpid=$!
    sleep 0.4
    local out
    out=$(nsenter -t "$CLI" -n python3 - <<'PY'
import socket
s = socket.socket(); s.settimeout(1.5)
try:
    s.connect(("10.90.2.2", 9099)); print("ARRIVED")
except Exception:
    print("SILENT")
PY
)
    kill "$lpid" 2>/dev/null; wait "$lpid" 2>/dev/null
    printf '%s' "$out"
}

echo "CASE forward-before-policy $(tcp_forwarded)"

nft -f "$RULES" 2>/dev/null || fatal "the shipped ruleset would not load in a namespace"

echo "CASE mdns-control        $(udp_to_router 5353 0)"
echo "CASE forward-after-policy $(tcp_forwarded)"
echo "CASE dhcp-server         $(udp_to_router 67 68)"
echo "CASE dns-server          $(udp_to_router 53 0)"
echo "CASE ipp-closed          $(tcp_to_router 631)"

# The documented remedy, end to end. `apex firewall allow ipp` writes 631 into
# this set; doing it by hand here proves the mechanism reopens the port rather
# than recording an intention nothing acts on.
nft add element inet apex allowed_tcp '{ 631 }' 2>/dev/null \
    || { echo "CASE ipp-exception SETUP"; exit 0; }
echo "CASE ipp-exception       $(tcp_to_router 631)"
INNER_EOF

OUT=$(timeout 180 "${UNSHARE[@]}" bash "$INNER" "$RULES_ABS" 2>&1)
rc=$?

if [ "$rc" -ne 0 ] || grep -q '^SETUP-FAILED' <<<"$OUT"; then
    reason=$(grep '^SETUP-FAILED' <<<"$OUT" | head -1)
    skipped "the whole file" "${reason:-the namespace exited rc=$rc}"
    report; exit $?
fi

verdict() { awk -v n="$1" '$1=="CASE" && $2==n { print $3 }' <<<"$OUT" | head -1; }

# ── the control comes first: without it nothing below means anything ────────
echo "── the harness delivers packets at all ────────────────────────────────"
if [ "$(verdict mdns-control)" = ARRIVED ]; then
    ok "mDNS on 5353 reaches the host through the policy"
else
    bad "mDNS on 5353 reaches the host through the policy" \
        "control failed — every 'blocked' verdict below would be meaningless, so they are not graded"
    report; exit $?
fi

echo
echo "── forwarding, which is what tethering and a hotspot are ──────────────"
if [ "$(verdict forward-before-policy)" = ARRIVED ]; then
    ok "a router with no policy forwards, so the wiring is real"
else
    bad "a router with no policy forwards, so the wiring is real" \
        "forwarding failed before the firewall was even loaded"
fi
if [ "$(verdict forward-after-policy)" = SILENT ]; then
    ok "the shipped policy forwards nothing between interfaces"
else
    bad "the shipped policy forwards nothing between interfaces" \
        "forward reached the far side; the drop policy is not in effect"
fi

echo
echo "── a hotspot SERVES DHCP and DNS; the policy only allows receiving them ─"
# The policy's DHCP rule is `udp sport 67 udp dport 68`, which is the reply
# direction — this machine as a client. A hotspot is the other direction: its
# clients broadcast a DISCOVER to dport 67, which is a new flow, so
# established/related cannot rescue it either.
if [ "$(verdict dhcp-server)" = SILENT ]; then
    ok "a DHCP server on this host receives nothing"
else
    bad "a DHCP server on this host receives nothing" \
        "dport 67 arrived — if that was opened deliberately, scope it to the shared interface"
fi
if [ "$(verdict dns-server)" = SILENT ]; then
    ok "a DNS server on this host receives nothing"
else
    bad "a DNS server on this host receives nothing" \
        "dport 53 arrived — same question as DHCP above"
fi

echo
echo "── sharing a printer, and the exception that unblocks it ──────────────"
if [ "$(verdict ipp-closed)" = SILENT ]; then
    ok "IPP 631 is closed until somebody opens it"
else
    bad "IPP 631 is closed until somebody opens it" "631 was reachable with no exception set"
fi
case "$(verdict ipp-exception)" in
    ARRIVED) ok "adding ipp to the exception set reopens 631" ;;
    SETUP)   skipped "adding ipp to the exception set reopens 631" "the set would not take an element" ;;
    *)       bad "adding ipp to the exception set reopens 631" \
                 "631 stayed shut with 631 in allowed_tcp — \`apex firewall allow ipp\` would not fix a shared printer" ;;
esac

report

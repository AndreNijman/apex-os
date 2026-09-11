#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-device-services-netns.sh — whether a printer, a Windows share and an
#  NFS export can be reached through the shipped firewall, and whether opening
#  the named exception puts that right.
#
#  ── What this is NOT ────────────────────────────────────────────────────────
#  It is not a test of the firewall. P1-044 owns the base policy, the helper
#  and the question "does the policy drop what it says it drops", and proves
#  those in `tests/test-apex-firewall-live.sh`. Nothing here asserts a rule.
#
#  The question here is the one a user asks about a device: my printer is
#  shared, so why can nobody print to it. A desktop feature that fails because
#  of an unmentioned firewall is the worst outcome available to P2-005, so
#  the shape of the failure is worth pinning down: which device services reach
#  this machine out of the box, which do not, and whether the documented remedy
#  works.
#
#  ── Where the port numbers come from ────────────────────────────────────────
#  From `files/system/firewall/services`, the catalogue `apex firewall allow`
#  reads, rather than hardcoded into the probes. `apex firewall allow ipp` opens
#  whatever that file says `ipp` is, so if the catalogue named the wrong port
#  the remedy would report success and change nothing.
#
#  ── Why it cannot touch the machine running it ──────────────────────────────
#  The run happens inside `unshare -rmn`: a fresh user, mount and network
#  namespace. nftables tables are network-namespace scoped, so `nft -f apex.nft`
#  in there is invisible to the host, whose ruleset, routes and interfaces are
#  never read or written. It needs no root and asks for none. The machine that
#  runs this suite is somebody's workstation on their real network, and a test
#  that reached the host would disconnect them mid-run.
#
#  ── The positive control, which is the point ────────────────────────────────
#  "The packet did not arrive" is the same observation whether the firewall
#  dropped it or the harness sent none. mDNS — which the policy accepts on
#  purpose, and which is how a driverless printer is found at all — is therefore
#  checked first, and the file stops if it fails rather than reporting a page of
#  blocked services it did not send.
#
#  PASS = discovery reaches this machine, sharing does not until someone opens
#         it, and opening it by catalogue name works.
#
#  Run from anywhere: ./tests/test-device-services-netns.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

RULES=files/system/nftables/apex.nft
CATALOGUE=files/system/firewall/services
for f in "$RULES" "$CATALOGUE"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done
RULES_ABS=$(cd "$(dirname "$RULES")" && pwd)/$(basename "$RULES")

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-58s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-58s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-58s %s\n' "$1" "$2"; skip=$((skip+1)); }

report() {
    echo
    printf 'device services (netns): %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
    [ "$fail" -eq 0 ]
}

# The catalogue is the source of truth for what a service name opens.
port_of() { awk -v n="$1" '$1==n { print $3; found=1 } END { exit !found }' "$CATALOGUE"; }

IPP=$(port_of ipp)   || { echo "no ipp in $CATALOGUE";   exit 2; }
SMB=$(port_of samba) || { echo "no samba in $CATALOGUE"; exit 2; }
NFS=$(port_of nfs)   || { echo "no nfs in $CATALOGUE";   exit 2; }

# ── the catalogue names the port the device listens on ─────────────────────
# Everything below opens whatever the catalogue says and then probes the same
# number, so the namespace run on its own cannot tell a right port from a wrong
# one — it would open 6310, reach 6310, and report success while every real
# printer stayed unreachable. These three numbers are not a firewall opinion;
# they are what CUPS, smbd and nfsd bind, so they are checked against IANA here.
echo "── the catalogue names the ports these services really use ─────────────"
for spec in "ipp 631 CUPS" "samba 445 smbd" "nfs 2049 nfsd"; do
    set -- $spec
    got=$(port_of "$1")
    [ "$got" = "$2" ] \
        && ok "$1 is port $2, which is what $3 binds" \
        || bad "$1 is port $2, which is what $3 binds" \
               "the catalogue says $got, so \`apex firewall allow $1\` opens the wrong port"
done
echo

# ── can we build a namespace at all ─────────────────────────────────────────
for tool in unshare nsenter ip nft python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        skipped "every case that needs a namespace" "$tool is not installed"
        report; exit $?
    }
done

if [ "$(id -u)" = 0 ]; then
    UNSHARE=(unshare -mn)          # already root: no user namespace needed
else
    UNSHARE=(unshare -rmn)
fi

if ! "${UNSHARE[@]}" true 2>/dev/null; then
    skipped "every case that needs a namespace" "this kernel refuses an unprivileged network namespace"
    report; exit $?
fi

# ── the whole experiment, run once inside the namespace ─────────────────────
# Two namespaces: this machine, with the policy on it, and one peer on the same
# link standing in for the laptop that wants to print. One process, so they live
# as long as the experiment and no longer. It prints one `CASE <name> <verdict>`
# line per probe and the grading happens out here, so a namespace that dies half
# way through reads as missing cases rather than as passes.
INNER=$(mktemp /tmp/apex-devsvc-inner.XXXXXX.sh) || exit 2
trap 'rm -f "$INNER"' EXIT

cat > "$INNER" <<'INNER_EOF'
set -uo pipefail
RULES="$1"; IPP="$2"; SMB="$3"; NFS="$4"

fatal() { echo "SETUP-FAILED $*"; exit 3; }

# The peer as a sleeping process, addressed by pid. `ip netns add` wants a
# shared bind mount under /run/netns and does not get one inside a private
# mount namespace; a pid is a namespace handle that needs no mount at all.
unshare -n sleep 180 & PEER=$!
sleep 0.3
kill -0 "$PEER" 2>/dev/null || fatal "the peer namespace did not start"
trap 'kill "$PEER" 2>/dev/null' EXIT

ip link add veth-p type veth peer name apexhost || fatal "veth pair"
ip link set veth-p netns "$PEER"                || fatal "move the peer end"
ip addr add 10.91.0.1/24 dev apexhost           || fatal "address on this side"
ip link set apexhost up; ip link set lo up
nsenter -t "$PEER" -n ip addr add 10.91.0.2/24 dev veth-p || fatal "peer address"
nsenter -t "$PEER" -n ip link set veth-p up
nsenter -t "$PEER" -n ip link set lo up

# ── probes ──────────────────────────────────────────────────────────────────
# Each binds a listener on this machine, has the peer send one datagram or one
# SYN, and says whether it landed. Timeouts are short because a blocked packet
# is silence, and silence is how long this file takes.

udp_probe() {  # $1 = dport
    local dport=$1 out
    python3 - "$dport" >"/tmp/.rx.$dport" 2>/dev/null <<'PY' &
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
    nsenter -t "$PEER" -n python3 - "$dport" <<'PY' >/dev/null 2>&1
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
for _ in range(3):
    s.sendto(b"apex-probe", ("10.91.0.1", int(sys.argv[1])))
PY
    wait "$rxpid" 2>/dev/null
    out=$(cat "/tmp/.rx.$dport" 2>/dev/null); rm -f "/tmp/.rx.$dport"
    printf '%s' "${out:-SILENT}"
}

tcp_probe() {  # $1 = dport
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
    out=$(nsenter -t "$PEER" -n python3 - "$dport" <<'PY'
import socket, sys
s = socket.socket(); s.settimeout(1.5)
try:
    s.connect(("10.91.0.1", int(sys.argv[1]))); print("ARRIVED")
except Exception:
    print("SILENT")
PY
)
    kill "$lpid" 2>/dev/null; wait "$lpid" 2>/dev/null
    printf '%s' "$out"
}

nft -f "$RULES" 2>/dev/null || fatal "the shipped ruleset would not load in a namespace"

# Discovery, which the policy permits so that a driverless printer can be found.
echo "CASE mdns-control  $(udp_probe 5353)"
echo "CASE llmnr-udp     $(udp_probe 5355)"
echo "CASE llmnr-tcp     $(tcp_probe 5355)"

# Sharing, which the policy closes.
echo "CASE ipp-closed    $(tcp_probe "$IPP")"
echo "CASE samba-closed  $(tcp_probe "$SMB")"
echo "CASE nfs-closed    $(tcp_probe "$NFS")"

# The documented remedy. `apex firewall allow <name>` writes the catalogue's
# port into this set; doing that here by catalogue number proves the remedy
# reopens the service rather than recording an intention nothing acts on.
for spec in "ipp $IPP" "samba $SMB" "nfs $NFS"; do
    set -- $spec
    if nft add element inet apex allowed_tcp "{ $2 }" 2>/dev/null; then
        echo "CASE $1-allowed    $(tcp_probe "$2")"
    else
        echo "CASE $1-allowed    SETUP"
    fi
done
INNER_EOF

OUT=$(timeout 240 "${UNSHARE[@]}" bash "$INNER" "$RULES_ABS" "$IPP" "$SMB" "$NFS" 2>&1)
rc=$?

if [ "$rc" -ne 0 ] || grep -q '^SETUP-FAILED' <<<"$OUT"; then
    reason=$(grep '^SETUP-FAILED' <<<"$OUT" | head -1)
    skipped "every case that needs a namespace" "${reason:-the namespace exited rc=$rc}"
    report; exit $?
fi

verdict() { awk -v n="$1" '$1=="CASE" && $2==n { print $3 }' <<<"$OUT" | head -1; }

# ── the control comes first: without it nothing below means anything ────────
echo "── the harness delivers packets at all ────────────────────────────────"
if [ "$(verdict mdns-control)" = ARRIVED ]; then
    ok "a printer's mDNS answer reaches this machine"
else
    bad "a printer's mDNS answer reaches this machine" \
        "control failed — every verdict below would be meaningless, so none is graded"
    report; exit $?
fi

echo
echo "── discovery, which has to work with nobody configuring anything ───────"
[ "$(verdict llmnr-udp)" = ARRIVED ] \
    && ok "LLMNR over UDP reaches this machine" \
    || bad "LLMNR over UDP reaches this machine" "a Windows host could not resolve this one by name"
[ "$(verdict llmnr-tcp)" = ARRIVED ] \
    && ok "LLMNR over TCP reaches this machine" \
    || bad "LLMNR over TCP reaches this machine" "the TCP half of LLMNR is shut"

echo
echo "── sharing, which is closed until somebody says otherwise ──────────────"
# This is the half a user meets as a broken device rather than as a firewall.
# It is correct that these are shut; what matters for P2-005 is that the tools
# say so at the point of failure instead of reporting an absent printer.
for spec in "ipp $IPP printer" "samba $SMB Windows-share" "nfs $NFS NFS-export"; do
    set -- $spec
    if [ "$(verdict "$1-closed")" = SILENT ]; then
        ok "a shared $3 on port $2 is unreachable by default"
    else
        bad "a shared $3 on port $2 is unreachable by default" "$2 answered with no exception set"
    fi
done

echo
echo "── and the remedy the tools are supposed to name ───────────────────────"
for spec in "ipp $IPP" "samba $SMB" "nfs $NFS"; do
    set -- $spec
    case "$(verdict "$1-allowed")" in
        ARRIVED) ok "\`apex firewall allow $1\` would make port $2 reachable" ;;
        SETUP)   skipped "\`apex firewall allow $1\` would make port $2 reachable" "the set would not take an element" ;;
        *)       bad "\`apex firewall allow $1\` would make port $2 reachable" \
                     "$2 stayed shut with $2 in allowed_tcp, so the advice would be wrong" ;;
    esac
done

report

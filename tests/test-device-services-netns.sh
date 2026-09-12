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
#  ── And the hotspot, which is P2-006's own acceptance criterion ─────────────
#  Sharing a connection inverts the policy's assumptions: the phone that joins
#  broadcasts a DHCP DISCOVER at us and then asks us to resolve names, both
#  inbound on ports the policy shuts. The rules that let that through are scoped
#  to the shared link, and the half of this file that matters is the half that
#  proves the scope holds — a second link is wired up for no other reason than
#  to watch DHCP and DNS stay shut on it while they are open on the first.
#  Nothing here reads a rule; every verdict is a packet that arrived or did not.
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
# The hotspot half calls the shipped tool rather than `nft add element`, because
# the thing under test is what NetworkManager's dispatcher invokes: the name
# validation, the not-loaded case and the message all live in there, and a test
# that reached past them would prove the ruleset works and the product does not.
FIREWALL=files/system/libexec/apex-firewall
for f in "$RULES" "$CATALOGUE" "$FIREWALL"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done
abspath() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
RULES_ABS=$(abspath "$RULES")
FIREWALL_ABS=$(abspath "$FIREWALL")

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
RULES="$1"; IPP="$2"; SMB="$3"; NFS="$4"; FIREWALL="$5"

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

# A SECOND link to the same peer. Its whole purpose is to be the interface the
# hotspot exception is NOT scoped to: every "still shut" verdict below is a
# packet that went out of this one, and without it "the exception is scoped to
# one link" would be a claim about a rule's text rather than about traffic.
ip link add veth-q type veth peer name apexhost2 || fatal "second veth pair"
ip link set veth-q netns "$PEER"                 || fatal "move the second peer end"
ip addr add 10.92.0.1/24 dev apexhost2           || fatal "address on the second link"
ip link set apexhost2 up
nsenter -t "$PEER" -n ip addr add 10.92.0.2/24 dev veth-q || fatal "second peer address"
nsenter -t "$PEER" -n ip link set veth-q up

# A third namespace on the far side of this one, so that "does the policy drop
# what it routes" can be asked with a packet. A hotspot is useless if the
# client gets an address and a name and then reaches nothing.
unshare -n sleep 180 & SRV=$!
sleep 0.3
kill -0 "$SRV" 2>/dev/null || fatal "the server namespace did not start"
trap 'kill "$PEER" "$SRV" 2>/dev/null' EXIT
ip link add veth-s type veth peer name apexsrv || fatal "server veth pair"
ip link set veth-s netns "$SRV"                || fatal "move the server end"
ip addr add 10.93.0.1/24 dev apexsrv           || fatal "address on the server link"
ip link set apexsrv up
nsenter -t "$SRV" -n ip addr add 10.93.0.2/24 dev veth-s || fatal "server address"
nsenter -t "$SRV" -n ip link set veth-s up
nsenter -t "$SRV" -n ip link set lo up
# The client's default route points at us, and the server knows the way back.
nsenter -t "$PEER" -n ip route add default via 10.91.0.1 dev veth-p 2>/dev/null
nsenter -t "$SRV"  -n ip route add 10.91.0.0/24 via 10.93.0.1 dev veth-s 2>/dev/null
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || fatal "cannot enable forwarding in this namespace"

# ── probes ──────────────────────────────────────────────────────────────────
# Each binds a listener on this machine, has the peer send one datagram or one
# SYN, and says whether it landed. Timeouts are short because a blocked packet
# is silence, and silence is how long this file takes.

# $1 = dport, $2 = destination address on this machine (default: the first
# link), $3 = source port (default: ephemeral), $4 = "bcast" to set
# SO_BROADCAST and send from the peer's matching address.
#
# The destination is a parameter because the interface a packet arrives on is
# the whole question for the hotspot rules: 10.91.0.1 lands on apexhost, the
# link this machine shares, and 10.92.0.1 lands on apexhost2, which it does not.
udp_probe() {
    local dport=$1 dst=${2:-10.91.0.1} sport=${3:-0} mode=${4:-unicast} out tag
    tag="${dport}.${dst##*.}.${sport}"
    python3 - "$dport" >"/tmp/.rx.$tag" 2>/dev/null <<'PY' &
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", int(sys.argv[1])))
s.settimeout(2.0)
try:
    s.recvfrom(512); print("ARRIVED")
except Exception:
    print("SILENT")
PY
    local rxpid=$!
    sleep 0.4
    nsenter -t "$PEER" -n python3 - "$dport" "$dst" "$sport" "$mode" <<'PY' >/dev/null 2>&1
import socket, sys
dport, dst, sport, mode = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
src = "10.91.0.2" if dst.startswith("10.91.") else "10.92.0.2"
if mode == "bcast":
    # What a client that has no address yet actually sends: SO_BROADCAST, from
    # port 68 to port 67. The destination is the link's broadcast rather than
    # 255.255.255.255 only because this namespace has two links and a limited
    # broadcast would need SO_BINDTODEVICE to choose between them; the rule
    # under test matches on the ports either way.
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    dst = src.rsplit(".", 1)[0] + ".255"
if sport:
    s.bind((src, sport))
for _ in range(3):
    s.sendto(b"apex-probe", (dst, dport))
PY
    wait "$rxpid" 2>/dev/null
    out=$(cat "/tmp/.rx.$tag" 2>/dev/null); rm -f "/tmp/.rx.$tag"
    printf '%s' "${out:-SILENT}"
}

# A client's DHCP DISCOVER: broadcast, from port 68 to port 67. The policy's
# only DHCP rule is `udp sport 67 udp dport 68` — the reply direction, a lease
# arriving for a client this machine is — and it cannot match this.
dhcp_probe() { udp_probe 67 "${1:-10.91.0.1}" 68 bcast; }

# Can a client reach anything THROUGH this machine while the policy is loaded?
# The receiver is in the third namespace, so a verdict of ARRIVED means the
# packet was routed rather than delivered locally.
forward_probe() {
    local out
    nsenter -t "$SRV" -n python3 - >"/tmp/.rx.fwd" 2>/dev/null <<'PY' &
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", 9999))
s.settimeout(2.0)
try:
    s.recvfrom(64); print("ARRIVED")
except Exception:
    print("SILENT")
PY
    local rxpid=$!
    sleep 0.4
    nsenter -t "$PEER" -n python3 - <<'PY' >/dev/null 2>&1
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
for _ in range(3):
    s.sendto(b"apex-probe", ("10.93.0.2", 9999))
PY
    wait "$rxpid" 2>/dev/null
    out=$(cat /tmp/.rx.fwd 2>/dev/null); rm -f /tmp/.rx.fwd
    printf '%s' "${out:-SILENT}"
}

tcp_probe() {  # $1 = dport, $2 = destination address on this machine
    local dport=$1 dst=${2:-10.91.0.1} out
    python3 - "$dport" >/dev/null 2>&1 <<'PY' &
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", int(sys.argv[1]))); s.listen(2); s.settimeout(3.0)
try: s.accept()
except Exception: pass
PY
    local lpid=$!
    sleep 0.4
    out=$(nsenter -t "$PEER" -n python3 - "$dport" "$dst" <<'PY'
import socket, sys
s = socket.socket(); s.settimeout(1.5)
try:
    s.connect((sys.argv[2], int(sys.argv[1]))); print("ARRIVED")
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

# ── the hotspot ─────────────────────────────────────────────────────────────
# Everything from here to `hotspot remove` runs before the remedy loop below
# adds 631/445/2049 to allowed_tcp, so "the shared link is open for DHCP and
# DNS and for nothing else" is asked while nothing else is open at all.
#
# What a client sends first: a broadcast DHCP DISCOVER, from port 68 to port 67.
# It is a directed broadcast from an addressed source rather than 255.255.255.255
# from 0.0.0.0, because this namespace has two links and a limited broadcast
# would need SO_BINDTODEVICE to pick one; the rule under test matches on the
# arrival interface and the ports, which are identical either way.
echo "CASE hs-dhcp-shut      $(dhcp_probe)"
echo "CASE hs-dnsudp-shut    $(udp_probe 53)"
echo "CASE hs-dnstcp-shut    $(tcp_probe 53)"

# Opened the way NetworkManager's dispatcher opens it.
if "$FIREWALL" hotspot add apexhost 2>&1 | grep -q "opened DHCP and DNS on 'apexhost'"; then
    echo "CASE hs-add            SAID-SO"
else
    echo "CASE hs-add            NO"
fi
echo "CASE hs-dhcp-open      $(dhcp_probe)"
echo "CASE hs-dnsudp-open    $(udp_probe 53)"
echo "CASE hs-dnstcp-open    $(tcp_probe 53)"

# The scope, which is the half that matters. Same three probes, sent down the
# link that is NOT in the set, while the first one is.
echo "CASE hs-dhcp-other     $(dhcp_probe 10.92.0.1)"
echo "CASE hs-dnsudp-other   $(udp_probe 53 10.92.0.1)"
echo "CASE hs-dnstcp-other   $(tcp_probe 53 10.92.0.1)"

# And the shared link is open for those two services, not opened wholesale.
echo "CASE hs-smb-on-shared  $(tcp_probe "$SMB")"

# What the tool tells the user while two links are shared. nft prints a set's
# elements quoted, and wraps them one per line as soon as there is more than
# one, so this is where a line-at-a-time read of `elements = { ... }` reports
# nothing and the tool says the machine is sharing on no link at all.
"$FIREWALL" hotspot add apexhost2 >/dev/null 2>&1
LIST="$("$FIREWALL" hotspot list 2>&1 | tr '\n' ' ')"
# `grep -qw`, not a `*apexhost*` glob: apexhost2 CONTAINS apexhost, so a glob
# for the first name is satisfied by the second one and a parser that dropped
# the first element would pass. That verdict could not fire until this line
# said -w.
v=NAMED
grep -qw apexhost  <<<"$LIST" || v=LOST-FIRST
grep -qw apexhost2 <<<"$LIST" || v=LOST-SECOND
case "$LIST" in *'"'*)           v=QUOTED ;; esac
case "$LIST" in *'not sharing'*) v=SAID-NOT-SHARING ;; esac
echo "CASE hs-list           $v"

# Down again. The dispatcher's `down` path runs this, and a link left open
# after the hotspot stops is the failure this whole mechanism must not have.
"$FIREWALL" hotspot remove apexhost2 >/dev/null 2>&1
"$FIREWALL" hotspot remove apexhost  >/dev/null 2>&1
echo "CASE hs-dhcp-closed    $(dhcp_probe)"
echo "CASE hs-dnsudp-closed  $(udp_probe 53)"

# ── and whether a client reaches anything through us ────────────────────────
# A hotspot that hands out an address and a name and then routes nothing is
# still broken. The policy has no forward chain — P1-044 removed it, because
# every base chain at a hook is evaluated and a drop here killed rootful
# container networking — so this should arrive. The control is the next case:
# a forward chain with policy drop, added and then deleted, so that ARRIVED is
# a measurement rather than the only answer this probe can give.
echo "CASE fwd-open          $(forward_probe)"
if nft add chain inet apex fwdprobe '{ type filter hook forward priority filter; policy drop; }' 2>/dev/null; then
    echo "CASE fwd-dropped       $(forward_probe)"
    nft delete chain inet apex fwdprobe 2>/dev/null
    echo "CASE fwd-reopen        $(forward_probe)"
else
    echo "CASE fwd-dropped       SETUP"
    echo "CASE fwd-reopen        SETUP"
fi

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

OUT=$(timeout 240 "${UNSHARE[@]}" bash "$INNER" "$RULES_ABS" "$IPP" "$SMB" "$NFS" "$FIREWALL_ABS" 2>&1)
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
echo "── a hotspot, where this machine is the server on the link ─────────────"
# P2-006's acceptance criterion. Each of these is a packet, and each "still
# shut" verdict below travels down a second link that the exception is not
# scoped to, while the first one is open.
[ "$(verdict hs-dhcp-shut)" = SILENT ] \
    && ok "a client's DHCP DISCOVER is dropped when nothing is shared" \
    || bad "a client's DHCP DISCOVER is dropped when nothing is shared" \
           "port 67 answers on a link this machine is only a guest on"
[ "$(verdict hs-dnsudp-shut)" = SILENT ] \
    && ok "DNS over UDP is dropped when nothing is shared" \
    || bad "DNS over UDP is dropped when nothing is shared" \
           "this machine is an open resolver on every network it joins"
[ "$(verdict hs-dnstcp-shut)" = SILENT ] \
    && ok "DNS over TCP is dropped when nothing is shared" \
    || bad "DNS over TCP is dropped when nothing is shared" \
           "the TCP half of 53 is open with no hotspot running"

[ "$(verdict hs-add)" = SAID-SO ] \
    && ok "\`apex firewall hotspot add\` opens the link and says which" \
    || bad "\`apex firewall hotspot add\` opens the link and says which" \
           "the command the NM dispatcher runs did not report opening apexhost"

[ "$(verdict hs-dhcp-open)" = ARRIVED ] \
    && ok "the client's DHCP DISCOVER now reaches dnsmasq" \
    || bad "the client's DHCP DISCOVER now reaches dnsmasq" \
           "a phone joining the hotspot would get no address"
[ "$(verdict hs-dnsudp-open)" = ARRIVED ] \
    && ok "the client's DNS query now reaches this machine" \
    || bad "the client's DNS query now reaches this machine" \
           "a client with an address could still resolve no name"
[ "$(verdict hs-dnstcp-open)" = ARRIVED ] \
    && ok "DNS over TCP is open on the shared link too" \
    || bad "DNS over TCP is open on the shared link too" \
           "a truncated answer could not be retried over TCP"

for spec in "hs-dhcp-other DHCP" "hs-dnsudp-other DNS-over-UDP" "hs-dnstcp-other DNS-over-TCP"; do
    set -- $spec
    [ "$(verdict "$1")" = SILENT ] \
        && ok "$2 stays shut on a link that is not the shared one" \
        || bad "$2 stays shut on a link that is not the shared one" \
               "the exception is not scoped to the link, so sharing opens $2 everywhere"
done

[ "$(verdict hs-smb-on-shared)" = SILENT ] \
    && ok "sharing a connection opens DHCP and DNS and nothing else" \
    || bad "sharing a connection opens DHCP and DNS and nothing else" \
           "port $SMB answered on the shared link, so the exception opened the link not the services"

case "$(verdict hs-list)" in
    NAMED)            ok "\`hotspot list\` names both shared links, unquoted" ;;
    SAID-NOT-SHARING) bad "\`hotspot list\` names both shared links, unquoted" \
                          "it reported no shared link while two were open — nft wraps a set's elements once there is more than one" ;;
    QUOTED)           bad "\`hotspot list\` names both shared links, unquoted" \
                          "the interface names came back with nft's quotes still on them" ;;
    *)                bad "\`hotspot list\` names both shared links, unquoted" \
                          "one of the two shared links is missing from the answer" ;;
esac

[ "$(verdict hs-dhcp-closed)" = SILENT ] \
    && ok "\`hotspot remove\` shuts DHCP again when sharing stops" \
    || bad "\`hotspot remove\` shuts DHCP again when sharing stops" \
           "the link stayed open after the hotspot went down"
[ "$(verdict hs-dnsudp-closed)" = SILENT ] \
    && ok "\`hotspot remove\` shuts DNS again when sharing stops" \
    || bad "\`hotspot remove\` shuts DNS again when sharing stops" \
           "this machine is left an open resolver after sharing stops"

echo
echo "── and whether a client reaches anything through this machine ──────────"
case "$(verdict fwd-dropped)" in
    SILENT) ok "the probe can see a forward chain drop a routed packet" ;;
    SETUP)  skipped "the probe can see a forward chain drop a routed packet" \
                    "a forward chain could not be added, so the next case proves nothing" ;;
    *)      bad "the probe can see a forward chain drop a routed packet" \
                "a policy-drop forward chain did not stop it, so ARRIVED below means nothing" ;;
esac
[ "$(verdict fwd-open)" = ARRIVED ] \
    && ok "the shipped policy routes a hotspot client's traffic" \
    || bad "the shipped policy routes a hotspot client's traffic" \
           "a client would get an address and a name and then reach nothing"
[ "$(verdict fwd-reopen)" = ARRIVED ] \
    && ok "removing that chain lets the packet through again" \
    || bad "removing that chain lets the packet through again" \
           "the control chain left forwarding broken, so the case above is not repeatable"

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

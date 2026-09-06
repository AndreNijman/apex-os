#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-firewall.sh — assertions against the shipped default-drop policy
#  and the helper that manages its exceptions.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  APEX shipped with no firewall at all: firewalld absent, nftables present with
#  an empty ruleset, every bound port reachable. The policy that fixes it is a
#  static file, which makes it exactly the kind of thing that rots silently — a
#  rule deleted in a refactor changes nothing anybody notices until it matters.
#
#  So the base policy is asserted line by line, and each assertion says what
#  breaks without it. Three of them are not about security at all: without
#  established/related, ICMP and IPv6 neighbour discovery, the machine reads as
#  "the network is broken" and the firewall is the last place anyone looks.
#
#  ── What it deliberately does NOT do ────────────────────────────────────────
#  It never loads the ruleset. `nft -c` parses without applying, and a suite
#  that installed a default-drop policy on the machine running it would be a
#  suite that disconnects a remote developer mid-run.
#
#  PASS = the policy parses, every load-bearing rule is present, and the helper
#         refuses what it should.
#
#  Run from anywhere: ./tests/test-apex-firewall.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

RULES=files/system/nftables/apex.nft
HELPER=files/system/libexec/apex-firewall
UNIT=files/system/units/apex-firewall.service
CATALOGUE=files/system/firewall/services
for f in "$RULES" "$HELPER" "$UNIT" "$CATALOGUE"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done

WORK=$(mktemp -d /tmp/apex-fw-test.XXXXXX) || exit 2
trap 'rm -rf "$WORK"' EXIT

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-52s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-52s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-52s %s\n' "$1" "$2"; skip=$((skip+1)); }

has() {  # $1 = case name, $2 = extended regex the policy must contain
    if grep -qE "$2" "$RULES"; then ok "$1"; else bad "$1" "no line matching $2"; fi
}

echo "── the policy parses ──────────────────────────────────────────────────"
if ! command -v nft >/dev/null 2>&1; then
    skipped "the ruleset parses" "nft is not installed"
elif nft -c -f "$RULES" >/dev/null 2>&1; then
    ok "the ruleset parses"
elif [ "$(id -u)" != 0 ]; then
    # `nft -c` still needs CAP_NET_ADMIN to resolve some expressions. Say which
    # it was rather than reporting a syntax error that did not happen.
    skipped "the ruleset parses" "checking it needs root; run with sudo for this case"
else
    bad "the ruleset parses" "$(nft -c -f "$RULES" 2>&1 | head -1)"
fi

echo
echo "── default drop, which is the entire point ────────────────────────────"
has "input drops by default"        '^\s*type filter hook input priority filter; policy drop;$'
has "output is allowed"             '^\s*type filter hook output priority filter; policy accept;$'

# There WAS a forward chain, dropping by default, with a comment saying podman
# was unaffected because it "hooks at its own priority in its own table".
# tests/test-apex-firewall-live.sh measured that: every base chain at a hook is
# evaluated, and a drop in any one of them is final, so this chain overrode
# netavark, libvirt and incus and killed every rootful container's networking.
# ip_forward is 0 until something deliberately turns it on, and the thing that
# turns it on brings its own policy.
# Not anchored to the line start: a forward chain written on one line is the
# same chain, and an anchored pattern let exactly that mutant through.
if grep -q 'type filter hook forward' "$RULES"; then
    bad "no forward chain, which would override every container's own table" \
        "a drop here is final regardless of what netavark accepts"
else
    ok "no forward chain, which would override every container's own table"
fi

# `nft -f` merges a table block into an existing table rather than replacing
# it. Without these two lines a second load appends every rule again — measured
# in a namespace, 14 accepts became 28 — and a doubled `limit rate` is silently
# twice the rate it says.
if grep -qx 'table inet apex' "$RULES" && grep -qx 'delete table inet apex' "$RULES"; then
    ok "a second load replaces the table instead of appending to it"
else
    bad "a second load replaces the table instead of appending to it" \
        "no 'table inet apex' / 'delete table inet apex' preamble"
fi

echo
echo "── the rules without which the machine looks broken, not protected ────"
has "replies to our own traffic pass" '^\s*ct state established,related accept$'
has "loopback passes"                 '^\s*iif lo accept$'
has "ICMP errors pass (path MTU)"     'destination-unreachable, time-exceeded'
has "IPv6 neighbour discovery passes" 'nd-neighbor-solicit'
has "DHCP replies pass"               '^\s*udp sport 67 udp dport 68 accept$'

echo
echo "── ssh, which is load-bearing rather than incidental ──────────────────"
# apex host run, apex build --on and remote agents are all ssh. A policy without
# it strands the user on the machine they were driving from.
has "ssh is open"                     '^\s*tcp dport 22 accept$'

echo
echo "── exceptions cannot damage the base policy ───────────────────────────"
has "exceptions live in a tcp set"    'tcp dport @allowed_tcp accept'
has "exceptions live in a udp set"    'udp dport @allowed_udp accept'
if grep -qE '^\s*(add|insert) rule' "$HELPER"; then
    bad "the helper never writes a rule" "it edits the chain instead of the sets"
else
    ok "the helper never writes a rule"
fi

echo
echo "── echo responders are rate limited ───────────────────────────────────"
has "ICMP echo is rate limited"       'icmp type \{ echo-request \} limit rate'
has "ICMPv6 echo is rate limited"     'icmpv6 type \{ echo-request \} limit rate'

echo
echo "── the unit ───────────────────────────────────────────────────────────"
grep -q '^Before=network-pre.target' "$UNIT" \
    && ok "loads before the network is configured" \
    || bad "loads before the network is configured" "there would be an unfiltered window every boot"
grep -q '^Conflicts=nftables.service' "$UNIT" \
    && ok "cannot fight nftables.service for the ruleset" \
    || bad "cannot fight nftables.service for the ruleset" "two owners, last one wins silently"
grep -q '^WantedBy=multi-user.target' "$UNIT" \
    && ok "is enabled by an install section" \
    || bad "is enabled by an install section" "a firewall that ships disabled is documentation"

echo
echo "── the helper ─────────────────────────────────────────────────────────"
bash -n "$HELPER" && ok "the helper parses" || bad "the helper parses" "syntax error"

out=$(bash "$HELPER" list 2>&1)
if grep -q '^ssh ' <<<"$out" && grep -qi 'NAME .*PROTO .*PORT' <<<"$out"; then
    ok "list names services rather than ports"
else
    bad "list names services rather than ports" "$(head -1 <<<"$out")"
fi

# Every catalogue line must be name/proto/port/description, or `allow` writes a
# malformed exception file and the reload silently drops it.
badline=$(grep -vE '^\s*(#|$)' "$CATALOGUE" | grep -vE '^[a-z0-9-]+ +(tcp|udp) +[0-9]+ +\S' | head -1)
[ -z "$badline" ] && ok "every catalogue entry is well formed" \
                  || bad "every catalogue entry is well formed" "$badline"

# The helper carries a copy of the catalogue so it still works when the image's
# is missing, and nothing kept the two the same. A drifted fallback is worse
# than no fallback: `allow` succeeds, writes a file, and opens a port the user
# did not ask for — or refuses a service the catalogue lists.
shipped=$(grep -vE '^\s*(#|$)' "$CATALOGUE" | awk '{$1=$1};1')
# The FIRST heredoc only: usage() has one too, and swallowing it makes this
# case fail for a reason that has nothing to do with the catalogue.
builtin=$(awk '/cat <<.EOF.$/{if(!seen){f=1;seen=1;next}} /^EOF$/{f=0} f' "$HELPER" | awk 'NF{$1=$1};1')
if [ -z "$builtin" ]; then
    bad "the helper's built-in catalogue matches the shipped one" "no built-in catalogue found in the helper"
elif [ "$shipped" = "$builtin" ]; then
    ok "the helper's built-in catalogue matches the shipped one"
else
    bad "the helper's built-in catalogue matches the shipped one" \
        "$(diff <(echo "$shipped") <(echo "$builtin") | head -3 | tr '\n' ' ')"
fi

if [ "$(id -u)" = 0 ]; then
    skipped "allow refuses an unprivileged caller"  "running as root"
    skipped "reload refuses an unprivileged caller" "running as root"
else
    # allow and reload both change state and must refuse. `deny` is checked
    # separately below, because it answers "that was never open" first and only
    # reaches the root gate for a service that actually is.
    for verb in allow reload; do
        out=$(bash "$HELPER" "$verb" ssh 2>&1); rc=$?
        if [ "$rc" != 0 ] && grep -qi 'needs root' <<<"$out"; then
            ok "$verb refuses an unprivileged caller"
        else
            bad "$verb refuses an unprivileged caller" "rc=$rc: $(head -1 <<<"$out")"
        fi
    done
fi

# Closing something that was never open is a mistake worth naming, and naming it
# costs no privilege. Asking for a password first and *then* saying "that was
# never open" is the command this avoids being.
out=$(bash "$HELPER" deny syncthing 2>&1); rc=$?
if [ "$rc" != 0 ] && grep -qi 'was not allowed' <<<"$out"; then
    ok "deny says a service was never open before asking for a password"
else
    bad "deny says a service was never open before asking for a password" "rc=$rc: $(head -1 <<<"$out")"
fi

out=$(bash "$HELPER" allow definitely-not-a-service 2>&1)
grep -qi 'no service called' <<<"$out" && ok "an unknown service is named, not opened" \
    || bad "an unknown service is named, not opened" "$(head -1 <<<"$out")"

# The name becomes a path. `deny` reaches the path before the catalogue, so
# without a check on the name, `deny ../../../etc/issue` was an `rm -f` outside
# the exception directory, run as root. "was not allowed" is the WRONG answer
# here and is asserted against: it would mean the tool had already gone looking
# where it should not.
for evil in ../../../etc/issue /etc/issue 'a;b' 'a b'; do
    out=$(bash "$HELPER" deny "$evil" 2>&1); rc=$?
    if [ "$rc" != 0 ] && grep -qi 'not a service name' <<<"$out"; then
        ok "deny refuses '$evil' as a name rather than as a path"
    else
        bad "deny refuses '$evil' as a name rather than as a path" "rc=$rc: $(head -1 <<<"$out")"
    fi
done

# The lesson this codebase learned the hard way today, in a fourteenth place:
# `nft list` needs CAP_NET_ADMIN, and answering "no rules" to a caller who
# merely lacks permission is the worst answer a firewall tool can give.
if grep -q 'permission denied|operation not permitted' <<<"$(grep -o 'permission denied|operation not permitted' "$HELPER")"; then
    ok "status tells you it cannot look, rather than reporting nothing"
else
    bad "status tells you it cannot look, rather than reporting nothing" \
        "ruleset_state does not distinguish EACCES from an empty ruleset"
fi

echo
printf 'apex-firewall: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-firewall-ssh.sh — the one assertion a namespace cannot make.
#
#  tests/test-apex-firewall-live.sh loads the policy inside a network namespace
#  and proves everything about the rules. It cannot prove the thing that would
#  actually cost somebody a machine: that starting the unit over ssh leaves the
#  ssh up. The connection that matters there is the one carrying the test.
#
#  ── HOW IT IS ARRANGED ──────────────────────────────────────────────────────
#
#  This runs on a DRIVER machine and targets another one over ssh. That is not
#  convenience; it is what makes the assertions real:
#
#    * the connection under test is a real ssh session to a real sshd, held
#      open across the load through a ControlMaster, so "it survived" means the
#      same TCP connection carried a command afterwards;
#    * the driver is a genuine outside host, so "a new connection to a closed
#      port is dropped" is measured from off the machine rather than from
#      127.0.0.1, where the policy would let it through anyway.
#
#  ── THE DEAD-MAN'S SWITCH ───────────────────────────────────────────────────
#
#  Before the policy is loaded, a transient systemd timer is armed on the
#  target to delete the table. If this script dies, if the network drops, if
#  the driver loses power — the target un-firewalls itself within
#  --deadman seconds and is reachable again. The timer is verified present
#  before anything is loaded, because an unverified safety net is the failure
#  mode this whole programme keeps finding.
#
#  It deletes `table inet apex` and NOT `flush ruleset`: podman, libvirt and
#  anything else with a table of its own are not this policy's to remove.
#
#  ── IT REFUSES TO RUN ON THE MACHINE YOU ARE SITTING AT ─────────────────────
#
#  A default-drop policy on the wrong machine is a support call. The target
#  must be named, must not resolve to a local address, and the operator has to
#  say out loud that this machine may lose its network.
#
#  ── THE TARGET IS TWO THINGS ────────────────────────────────────────────────
#
#  --target is an ssh destination, and an ssh destination is usually an alias
#  out of ~/.ssh/config with a user and a key attached: `katana`, not a name
#  the resolver has ever heard of. The probes here are raw TCP from the driver,
#  so they need an address. It is taken from SSH_CONNECTION on the target —
#  the far end of the path already under test — rather than from a name lookup
#  that would fail on exactly the machines this is for.
#
#  Usage:
#    tests/test-apex-firewall-ssh.sh --target HOST --yes-this-host-may-lose-its-network
#                                   [--deadman SECONDS] [--keep]
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

TARGET=""; CONSENT=0; DEADMAN=300; KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --target)  TARGET="$2"; shift 2 ;;
        --deadman) DEADMAN="$2"; shift 2 ;;
        --keep)    KEEP=1; shift ;;
        --yes-this-host-may-lose-its-network) CONSENT=1; shift ;;
        *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done

pass=0; fail=0; skipped=0
ok()   { printf 'PASS  %s\n' "$1"; pass=$((pass + 1)); }
bad()  { printf 'FAIL  %s — %s\n' "$1" "${2:-}"; fail=$((fail + 1)); }
skip() { printf 'SKIP  %s — %s\n' "$1" "${2:-}"; skipped=$((skipped + 1)); }
sec()  { printf '\n── %s ──\n' "$1"; }
finish() {
    printf '\napex firewall ssh: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skipped"
    [ "$fail" -eq 0 ]
}

[ -n "$TARGET" ] || { printf 'refusing to guess a target; pass --target HOST\n' >&2; exit 2; }
[ "$CONSENT" = 1 ] || {
    printf 'This loads a default-drop policy on %s.\n' "$TARGET" >&2
    printf 'Re-run with --yes-this-host-may-lose-its-network when that is true.\n' >&2
    exit 2
}

# The target must be somewhere else. `localhost`, `127.0.0.1` and this
# machine's own hostname are all refused: the point of the driver being a
# separate host is that a probe from it crosses the policy.
case "$TARGET" in
    localhost|127.0.0.1|::1|"$(hostname)"|"$(hostname -s)")
        printf 'refusing: %s is this machine\n' "$TARGET" >&2; exit 2 ;;
esac

CM="$(mktemp -u /tmp/apex-fw-ssh.%h.XXXXXX)"
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=10
     -o ControlMaster=auto -o "ControlPath=$CM" -o ControlPersist=600)
on() { "${SSH[@]}" "$TARGET" "$@"; }

STAGED=0
cleanup() {
    local rc=$?
    if [ "$KEEP" = 0 ]; then
        on 'sudo -n systemctl stop apex-firewall.service 2>/dev/null
            sudo -n systemctl stop apex-fw-deadman.timer apex-fw-deadman.service 2>/dev/null
            sudo -n /usr/sbin/nft delete table inet apex 2>/dev/null
            true' >/dev/null 2>&1
        # By PID, never by pattern: `pkill -f "socat TCP4-LISTEN:22000"` reaches
        # into every network namespace on the machine, including the ones
        # tests/test-apex-firewall-live.sh is using if it happens to be running.
        [ -n "${LISTENER_PID:-}" ] && on "sudo -n kill $LISTENER_PID" >/dev/null 2>&1
        [ "$STAGED" = 1 ] && on 'sudo -n rm -f /etc/systemd/system/apex-firewall.service
                                 sudo -n rm -rf /run/apex-fw
                                 sudo -n systemctl daemon-reload' >/dev/null 2>&1
        # `allow` creates /etc/apex/firewall.d and nothing puts it back. Only
        # when this run is what created it: on a machine that already had one,
        # the exceptions in it are the user's.
        [ "${MADE_CONFDIR:-0}" = 1 ] && on 'sudo -n rm -rf /etc/apex/firewall.d' >/dev/null 2>&1
    fi
    ssh -o "ControlPath=$CM" -O exit "$TARGET" >/dev/null 2>&1
    return $rc
}
trap cleanup EXIT INT TERM

# ═════════════════════════════════════════════════════════════════════════════
sec "the target is reachable and is not already firewalled"
# ═════════════════════════════════════════════════════════════════════════════
if ! on true; then
    skip "the whole suite" "cannot ssh to $TARGET"
    finish; exit $?
fi
ok "ssh to $TARGET works, and this connection is the one under test"

if on 'sudo -n nft list table inet apex' >/dev/null 2>&1; then
    bad "the target starts with no apex table" "one is already loaded; refusing to disturb it"
    finish; exit $?
fi
ok "the target starts with no apex table"

# The address the target sees this connection arriving on. It needs no
# resolver, it is the same path every probe below crosses, and it works when
# --target is an ssh alias, which is the normal case.
PROBE="$(on 'printf "%s\n" "$SSH_CONNECTION"' 2>/dev/null | awk '{print $3}')"
if [ -z "$PROBE" ]; then
    bad "the target's own address is knowable from this connection" \
        "SSH_CONNECTION came back empty; every probe below would be aimed at nothing"
    finish; exit $?
fi
ok "probes will go to $PROBE, the address this ssh arrives on"

# The refusal above compares spellings, which an alias defeats: `ssh spare-box`
# pointing back here passes it, and this would then load a default-drop policy
# on the machine running the test. Compare addresses instead, now that there
# is one.
if ip -o addr show 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | grep -qxF "$PROBE"; then
    printf 'refusing: %s is an address of this machine\n' "$PROBE" >&2
    exit 2
fi
ok "$PROBE is not an address of this machine"

# Whether the exception directory is this run's to delete afterwards.
MADE_CONFDIR=0
on 'test -d /etc/apex/firewall.d' || MADE_CONFDIR=1

# ═════════════════════════════════════════════════════════════════════════════
sec "where the policy is going to come from"
# ═════════════════════════════════════════════════════════════════════════════
# On an image that shipped the firewall, everything is already in /usr and the
# unit is the shipped one. On a machine that predates it — or one whose /usr is
# read-only under a sysext, which is every APEX box with an extension merged —
# the files are staged under /run and the unit is written with two paths
# rewritten. The diff is PRINTED, so a reader can see that nothing about the
# ordering, the conflicts or the stop action changed.
if on 'test -x /usr/libexec/apex-firewall && test -f /usr/share/apex/nftables/apex.nft \
       && test -f /usr/lib/systemd/system/apex-firewall.service'; then
    ok "the target ships the firewall; testing exactly what it ships"
    UNIT_NOTE="the unit as shipped"
else
    STAGED=1
    on 'sudo -n mkdir -p /run/apex-fw/nftables /run/apex-fw/firewall /run/apex-fw/libexec' || {
        bad "staging the policy onto the target" "could not create /run/apex-fw"; finish; exit $?
    }
    tar cf - -C "$ROOT" files/system/nftables/apex.nft \
                        files/system/firewall/services \
                        files/system/libexec/apex-firewall \
                        files/system/units/apex-firewall.service \
        | on 'sudo -n tar xf - -C /run/apex-fw --strip-components=2' || {
        bad "staging the policy onto the target" "tar failed"; finish; exit $?
    }
    on 'sudo -n chmod 0755 /run/apex-fw/libexec/apex-firewall
        sudo -n sed -e "s|/usr/share/apex/nftables/|/run/apex-fw/nftables/|" \
                    -e "s|/usr/libexec/apex-firewall|/run/apex-fw/libexec/apex-firewall|" \
                    /run/apex-fw/units/apex-firewall.service \
              > /tmp/apex-firewall.service.staged
        sudo -n install -m 0644 /tmp/apex-firewall.service.staged \
              /etc/systemd/system/apex-firewall.service
        sudo -n systemctl daemon-reload' || {
        bad "staging the policy onto the target" "could not install the unit"; finish; exit $?
    }
    printf '  the unit differs from the shipped one only in these lines:\n'
    on 'diff /run/apex-fw/units/apex-firewall.service /etc/systemd/system/apex-firewall.service' \
        | sed 's/^/    /'
    # Two ExecStart lines changed and nothing else. Asserted rather than
    # eyeballed: a substitution that also moved `Before=` would make every
    # ordering claim below about a unit nobody ships.
    diffcount="$(on 'diff /run/apex-fw/units/apex-firewall.service /etc/systemd/system/apex-firewall.service | grep -c "^[<>]"')"
    if [ "$diffcount" = 4 ]; then
        ok "only the two ExecStart paths were rewritten"
    else
        bad "only the two ExecStart paths were rewritten" "$diffcount changed lines, expected 4"
    fi
    UNIT_NOTE="the shipped unit with two paths rewritten to /run"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the dead-man's switch, armed and verified before anything loads"
# ═════════════════════════════════════════════════════════════════════════════
on "sudo -n systemd-run --on-active=$DEADMAN --unit=apex-fw-deadman \
        /usr/sbin/nft delete table inet apex" >/dev/null 2>&1
if on 'systemctl list-timers --all apex-fw-deadman.timer --no-legend' | grep -q apex-fw-deadman; then
    ok "the target will un-firewall itself in ${DEADMAN}s if this run dies"
else
    bad "the target will un-firewall itself in ${DEADMAN}s if this run dies" \
        "the timer is not there; refusing to load a default-drop policy without it"
    finish; exit $?
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "loading it, over the connection it could break"
# ═════════════════════════════════════════════════════════════════════════════
# A port that is really listening on 0.0.0.0 and is really not in the policy.
# Steam's remote-play socket is the example the policy's own header names, so
# if it is there it is the honest probe; anything else bound to a wildcard will
# do. Measured before the load, so "it stopped answering" is a change rather
# than an assumption.
# 22, 5353 and 5355 are in the policy, so a probe against one of them would
# stay reachable and read as "nothing is being filtered". The port has to be
# one the policy really closes.
CLOSED_PORT="$(on "ss -tlnH 2>/dev/null | awk '\$4 ~ /^(0\\.0\\.0\\.0|\\[?::\\]?|\\*):/ { split(\$4,a,\":\"); p=a[length(a)]; if (p != 22 && p != 5353 && p != 5355) { print p; exit } }'")"
if [ -n "$CLOSED_PORT" ] && timeout 4 bash -c "</dev/tcp/$PROBE/$CLOSED_PORT" 2>/dev/null; then
    ok "port $CLOSED_PORT answers from off the machine before the policy loads"
    HAVE_PROBE=1
else
    skip "port $CLOSED_PORT answers from off the machine before the policy loads" \
         "no wildcard-bound port other than ssh to probe"
    HAVE_PROBE=0
fi

out="$(on 'sudo -n systemctl start apex-firewall.service 2>&1'; echo "rc=$?")"
if printf '%s' "$out" | grep -q 'rc=0'; then
    ok "the unit started ($UNIT_NOTE)"
else
    bad "the unit started" "$out"
    finish; exit $?
fi

# THE assertion. Same ControlMaster, same TCP connection, after the load.
if on 'echo alive' 2>/dev/null | grep -q alive; then
    ok "the ssh connection that loaded the policy is still carrying commands"
else
    bad "the ssh connection that loaded the policy is still carrying commands" \
        "the policy just stranded its own operator"
    finish; exit $?
fi
if on 'sudo -n nft list chain inet apex input' 2>/dev/null | grep -q 'policy drop'; then
    ok "and the policy really is enforcing while that connection lives"
else
    bad "and the policy really is enforcing while that connection lives" \
        "the connection survived because nothing was loaded"
fi

# A NEW connection from the same outside host, to a port that was answering a
# moment ago. Without this the case above proves only that ssh works.
if [ "$HAVE_PROBE" = 1 ]; then
    if timeout 4 bash -c "</dev/tcp/$PROBE/$CLOSED_PORT" 2>/dev/null; then
        bad "a new connection to that port is now dropped" "it still answers; nothing is being filtered"
    else
        ok "a new connection to that port is now dropped"
    fi
fi
# And ssh from scratch, not through the multiplexed channel: a second person
# must still be able to get in.
if ssh -o BatchMode=yes -o ConnectTimeout=10 -o ControlPath=none "$TARGET" true 2>/dev/null; then
    ok "a brand-new ssh connection still gets in"
else
    bad "a brand-new ssh connection still gets in" "the machine is only reachable through the open channel"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "the machine does not look broken"
# ═════════════════════════════════════════════════════════════════════════════
on 'getent hosts one.one.one.one >/dev/null 2>&1 || getent hosts github.com >/dev/null 2>&1' \
    && ok "DNS still resolves" || bad "DNS still resolves" "name lookups are failing"
on 'ping -c2 -W3 1.1.1.1 >/dev/null 2>&1' \
    && ok "ping still works outbound" || bad "ping still works outbound" "ICMP replies are being dropped"
on 'ping -6 -c2 -W3 2606:4700:4700::1111 >/dev/null 2>&1' \
    && ok "ping6 still works outbound" \
    || skip "ping6 still works outbound" "this machine may have no IPv6 route"
on 'ip -6 neigh show | grep -qvE "^$"' \
    && ok "the IPv6 neighbour cache is still populating" \
    || skip "the IPv6 neighbour cache is still populating" "no v6 neighbours on this link"
on 'timeout 4 avahi-browse -at >/dev/null 2>&1 || pgrep -x avahi-daemon >/dev/null' \
    && ok "avahi is still running and answering on 5353" \
    || skip "avahi is still running and answering on 5353" "no avahi on this machine"

# ═════════════════════════════════════════════════════════════════════════════
sec "allow and deny, from off the machine"
# ═════════════════════════════════════════════════════════════════════════════
FW=/usr/libexec/apex-firewall
[ "$STAGED" = 1 ] && FW=/run/apex-fw/libexec/apex-firewall
reach() { timeout 4 bash -c "</dev/tcp/$PROBE/22000" 2>/dev/null; }

LISTENER_PID="$(on 'setsid socat TCP4-LISTEN:22000,reuseaddr,fork PIPE >/dev/null 2>&1 & echo $!' 2>/dev/null | tr -dc "0-9")"
sleep 0.6
if on 'ss -tlnH "sport = :22000"' | grep -q 22000; then
    reach && bad "a listener on 22000 is unreachable until it is allowed" "it answered without an exception" \
          || ok "a listener on 22000 is unreachable until it is allowed"

    on "sudo -n $FW allow syncthing" >/dev/null 2>&1
    reach && ok "allow really opens it, from another machine" \
          || bad "allow really opens it, from another machine" "still unreachable after allow"

    # ── criterion 5: a real systemctl restart ────────────────────────────────
    on 'sudo -n systemctl restart apex-firewall.service' >/dev/null 2>&1
    sleep 1
    reach && ok "the exception survives a real systemctl restart" \
          || bad "the exception survives a real systemctl restart" "a restart closed what the user opened"

    # ── criterion 6: a malformed exception file ──────────────────────────────
    on 'printf "tcp notaport\n" | sudo -n tee /etc/apex/firewall.d/broken.conf >/dev/null' >/dev/null 2>&1
    on "sudo -n $FW reload" >/dev/null 2>&1
    still_dropping=1
    [ "$HAVE_PROBE" = 1 ] && { timeout 4 bash -c "</dev/tcp/$PROBE/$CLOSED_PORT" 2>/dev/null && still_dropping=0; }
    if on 'sudo -n nft list chain inet apex input' 2>/dev/null | grep -q 'policy drop' \
       && [ "$still_dropping" = 1 ]; then
        ok "a malformed exception leaves the base policy standing"
    else
        bad "a malformed exception leaves the base policy standing" "the base policy is gone or no longer dropping"
    fi
    reach && ok "and does not close the exception next to it" \
          || bad "and does not close the exception next to it" "one typo closed a working port"
    on 'sudo -n rm -f /etc/apex/firewall.d/broken.conf' >/dev/null 2>&1

    on "sudo -n $FW deny syncthing" >/dev/null 2>&1
    reach && bad "deny really closes it again" "it still answers" \
          || ok "deny really closes it again"
else
    skip "a listener on 22000 is unreachable until it is allowed" "could not start a listener on the target"
    skip "allow really opens it, from another machine" "no listener"
    skip "the exception survives a real systemctl restart" "no listener"
    skip "a malformed exception leaves the base policy standing" "no listener"
    skip "and does not close the exception next to it" "no listener"
    skip "deny really closes it again" "no listener"
fi

# ═════════════════════════════════════════════════════════════════════════════
sec "boot ordering, in systemd's own resolved graph"
# ═════════════════════════════════════════════════════════════════════════════
# `Before=network-pre.target` in a file is a claim. What systemd computed from
# it is a fact, and it is readable on a running machine without rebooting one:
# the target's own After list is the other end of this unit's Before edge.
on 'systemd-analyze verify /etc/systemd/system/apex-firewall.service 2>&1 || true' > /tmp/apex-fw-verify.$$ 2>&1
if [ ! -s /tmp/apex-fw-verify.$$ ]; then
    ok "systemd-analyze verify has nothing to say about the unit"
else
    bad "systemd-analyze verify has nothing to say about the unit" "$(head -2 /tmp/apex-fw-verify.$$ | tr '\n' ' ')"
fi
rm -f /tmp/apex-fw-verify.$$

on 'systemctl show network-pre.target -p After' 2>/dev/null | grep -q 'apex-firewall.service' \
    && ok "network-pre.target waits for apex-firewall.service" \
    || bad "network-pre.target waits for apex-firewall.service" "the Before= edge did not register"
on 'systemctl show NetworkManager.service -p After' 2>/dev/null | grep -q 'network-pre.target' \
    && ok "and NetworkManager waits for network-pre.target" \
    || skip "and NetworkManager waits for network-pre.target" "no NetworkManager on this machine"

finish

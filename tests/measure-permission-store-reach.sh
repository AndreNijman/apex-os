#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  measure-permission-store-reach.sh — does a permission-store change reach an
#  ALREADY-RUNNING PipeWire client? (roadmap P1-061, criterion 4)
#
#  ── The question, and why it was open ───────────────────────────────────────
#
#  `apex permissions revoke <app> camera` runs `flatpak permission-set devices
#  camera <app> no`, which is `SetPermission` on
#  `org.freedesktop.impl.portal.PermissionStore`. The model records the timing
#  of that as `Timing::NextRequest` — "takes effect the next time the app asks"
#  — and `Timing::Immediate` exists and is unreachable, held so by
#  apex-perm-core's `nothing_claims_a_revocation_is_immediate`.
#
#  The round that wrote that said, honestly, that the conservative answer was
#  chosen because nobody had measured the other one: measuring it looked like
#  it meant taking a camera grant away from whoever was logged in. It does not.
#  Everything below runs on a private session bus, a private PipeWire, a
#  private WirePlumber and a private XDG_DATA_HOME, so the store written to is
#  $XDG_DATA_HOME/flatpak/db/devices and the grant taken away belongs to an
#  application id that does not exist.
#
#  ── The answer, measured on the L16 on 2026-09-12 ───────────────────────────
#
#  NO. `access-portal.lua` grants the client its permissions when the client
#  CONNECTS, and does nothing at all when the store changes underneath a client
#  that is already connected. Every other link in the chain was verified
#  working in the same run, which is what makes the negative worth anything:
#
#    * the client really is a portal client — the server records
#      `pipewire.access = portal` with the app id and the Camera media role;
#    * WirePlumber really did grant it — `pw-cli get-permissions` shows `rwxm-`
#      on the client's own object and on every camera node, and
#      `access-portal.lua:87` logged `setting permissions: true`;
#    * the store really did change — `Lookup` reads back `['no']`;
#    * the `Changed` signal really was on the bus — `gdbus monitor` caught
#      `Changed('devices','camera',false,<byte 0x00>,{app:['no']})`;
#    * and the module really is connected to the store — its `lookup` calls
#      answer, logged by `m-portal-permissionstore`.
#
#  And six seconds after the write the running client still had `rwxm-` on
#  every object WirePlumber had granted it, with not one line from
#  `access-portal.lua`.
#
#  So `NextRequest` is not the cautious answer any more, it is the correct one,
#  and the test holding `Timing::Immediate` unreachable is right on evidence
#  rather than on caution. Where the chain breaks — between the store's
#  `Changed` and the Lua handler at access-portal.lua:123 — is NOT established
#  here and this file does not guess at it.
#
#  ── Why it needs a C client ─────────────────────────────────────────────────
#
#  `pipewire.access` is assigned by module-access from the socket the client
#  arrived on and from the client's own connection properties. Every shipped
#  pipewire tool — pw-dump, pw-mon, pw-cli — connects with
#  `remote.intention = manager`, lands on `pipewire-0-manager`, and is given
#  `unrestricted`; and `PIPEWIRE_PROPS` does not reach client properties in any
#  of the three syntaxes tried. The connection properties ARE the measurement,
#  so they have to be set by a client that sets them itself. That is
#  tests/permission-store-client.c, thirty lines of libpipewire.
#
#  ── Running it ──────────────────────────────────────────────────────────────
#
#      ./tests/measure-permission-store-reach.sh
#
#  Needs pipewire, wireplumber, gcc, the libpipewire headers and the pipewire
#  command-line tools. None of those are in the APEX image, so it SKIPS on a
#  stock machine rather than failing. To run it there anyway, fetch the two
#  packages without installing them and point this at the result:
#
#      dnf5 download --destdir=/var/tmp/pw pipewire-utils pipewire-devel
#      cd /var/tmp/pw && for r in *.rpm; do rpm2cpio "$r" | cpio -idm; done
#      PW_TOOLS_DIR=/var/tmp/pw/usr/bin PW_INCLUDE_DIR=/var/tmp/pw/usr/include \
#          ./tests/measure-permission-store-reach.sh
#
#  This is a MEASUREMENT, not a gate: it prints what it found and exits 0 on a
#  clean run whichever way the answer falls. A machine where the answer flips
#  is a machine where the model should change, and that is a decision for a
#  person, not an exit code.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

APP="com.example.apex-perm-measure"

need() {
    command -v "$1" >/dev/null 2>&1 || { echo "SKIP: $1 is not installed"; exit 0; }
}
need pipewire
need wireplumber
need gcc
need dbus-daemon
need gdbus
need python3

TOOLS="${PW_TOOLS_DIR:-}"
if [ -z "$TOOLS" ]; then
    command -v pw-cli >/dev/null 2>&1 || {
        echo "SKIP: the pipewire command-line tools are not installed."
        echo "      See the header for how to run this without installing them."
        exit 0
    }
    TOOLS="$(dirname "$(command -v pw-cli)")"
fi
[ -x "$TOOLS/pw-cli" ] && [ -x "$TOOLS/pw-dump" ] || {
    echo "SKIP: $TOOLS does not hold pw-cli and pw-dump"; exit 0; }

INC="${PW_INCLUDE_DIR:-/usr/include}"
[ -d "$INC/pipewire-0.3" ] && [ -d "$INC/spa-0.2" ] || {
    echo "SKIP: the libpipewire headers are not under $INC"
    echo "      See the header for how to run this without installing them."
    exit 0; }

LIB="$(ls /usr/lib64/libpipewire-0.3.so.0 /usr/lib/libpipewire-0.3.so.0 2>/dev/null | head -1)"
[ -n "$LIB" ] || { echo "SKIP: libpipewire-0.3.so.0 not found"; exit 0; }

# ── the sandbox ─────────────────────────────────────────────────────────────
AMBIENT_BUS="${DBUS_SESSION_BUS_ADDRESS:-}"
W="$(mktemp -d)"
BUS_PID=""
CLIENT_PID=""
MON_PID=""
cleanup() {
    local p
    for p in "$CLIENT_PID" "$MON_PID"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
    for f in wireplumber pipewire; do
        p="$(cat "$W/$f.pid" 2>/dev/null)"
        [ -n "$p" ] && kill "$p" 2>/dev/null
    done
    sleep 0.4
    for f in wireplumber pipewire; do
        p="$(cat "$W/$f.pid" 2>/dev/null)"
        [ -n "$p" ] && kill -9 "$p" 2>/dev/null
    done
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null
    rm -rf "$W"
    return 0
}
trap cleanup EXIT INT TERM

export PATH="$TOOLS:$PATH"
export HOME="$W/home"
export XDG_RUNTIME_DIR="$W/run"
export XDG_CONFIG_HOME="$W/config"
export XDG_DATA_HOME="$W/data"
export XDG_STATE_HOME="$W/state"
export XDG_CACHE_HOME="$W/cache"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" \
         "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
chmod 0700 "$XDG_RUNTIME_DIR"

# A WirePlumber profile with NO hardware at all. A second WirePlumber that
# opened this machine's ALSA or V4L2 devices would be reaching into the session
# somebody is logged into, and the only two components this measures are the
# permission-store module and the portal client-access script.
mkdir -p "$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d"
cat > "$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d/50-apex-measure.conf" <<'CONF'
wireplumber.profiles = {
  apex-permtest = {
    inherits = [ base ]
    metadata.sm-settings = required
    metadata.sm-objects  = required
    policy.standard      = required

    support.portal-permissionstore = required
    script.client.access-portal    = required

    hardware.audio         = disabled
    hardware.bluetooth     = disabled
    hardware.video-capture = disabled
    support.reserve-device = disabled
    support.logind         = disabled
  }
}
CONF

# ── the private bus, and the refusal that keeps it private ──────────────────
addr="$(dbus-daemon --session --print-address --fork 2>/dev/null | head -1)"
[ -n "$addr" ] || { echo "FAIL: could not start a private session bus"; exit 1; }
if [ -n "$AMBIENT_BUS" ] && [ "$addr" = "$AMBIENT_BUS" ]; then
    echo "FAIL: the 'private' bus IS the session's own; refusing to write a"
    echo "      permission store that belongs to whoever is logged in."
    exit 1
fi
export DBUS_SESSION_BUS_ADDRESS="$addr"
BUS_PID="$(pgrep -u "$(id -u)" -f "dbus-daemon --session --print-address --fork" | tail -1)"

echo "bus:   $addr"
echo "store: $XDG_DATA_HOME/flatpak/db/devices"

# ── the client ──────────────────────────────────────────────────────────────
gcc -O1 -o "$W/client" "$here/permission-store-client.c" \
    -I "$INC/pipewire-0.3" -I "$INC/spa-0.2" "$LIB" 2>"$W/cc.log" || {
    echo "SKIP: the measurement client did not compile"; sed 's/^/      /' "$W/cc.log" | head -10; exit 0; }

# ── the stack ───────────────────────────────────────────────────────────────
pipewire > "$W/pipewire.log" 2>&1 &
echo $! > "$W/pipewire.pid"
for _ in $(seq 1 40); do [ -S "$XDG_RUNTIME_DIR/pipewire-0" ] && break; sleep 0.25; done
[ -S "$XDG_RUNTIME_DIR/pipewire-0" ] || {
    echo "SKIP: the private pipewire did not come up"; tail -10 "$W/pipewire.log"; exit 0; }

# Activate the permission store BEFORE WirePlumber, so that "the module started
# before the service existed" cannot be the explanation for anything below.
store() {
    gdbus call --session --dest org.freedesktop.impl.portal.PermissionStore \
        --object-path /org/freedesktop/impl/portal/PermissionStore \
        --method org.freedesktop.impl.portal.PermissionStore.SetPermission \
        devices true camera "$APP" "['$1']" >/dev/null 2>&1
}
lookup() {
    gdbus call --session --dest org.freedesktop.impl.portal.PermissionStore \
        --object-path /org/freedesktop/impl/portal/PermissionStore \
        --method org.freedesktop.impl.portal.PermissionStore.Lookup devices camera 2>&1
}
store yes || true
[ "$(lookup)" != "" ] || { echo "SKIP: xdg-permission-store did not activate"; exit 0; }

WIREPLUMBER_DEBUG="${WIREPLUMBER_DEBUG:-s-client:D,m-portal-permissionstore:D,I}" \
    wireplumber -p apex-permtest > "$W/wireplumber.log" 2>&1 &
echo $! > "$W/wireplumber.pid"
sleep 3
kill -0 "$(cat "$W/wireplumber.pid")" 2>/dev/null || {
    echo "SKIP: the private wireplumber did not stay up"; tail -15 "$W/wireplumber.log"; exit 0; }

grep -q "access-portal" "$W/wireplumber.log" || {
    echo "SKIP: client/access-portal.lua did not load, so there is nothing to measure"
    tail -15 "$W/wireplumber.log" | sed 's/^/      /'
    exit 0; }
echo "wireplumber: portal-permissionstore and access-portal.lua are loaded"

# A node the script will look at: access-portal.lua picks its node set purely
# by media.class and media.role, so the node's own behaviour is irrelevant. A
# real /dev/video0 would mean opening the camera on the machine somebody is
# using, which this has no business doing.
pw-cli create-node spa-node-factory \
    '{ factory.name=support.null-audio-sink node.name=apex-measure-cam media.class=Video/Source media.role=Camera object.linger=true }' \
    >/dev/null 2>&1
sleep 1

# ── the measurement ─────────────────────────────────────────────────────────
timeout 30 gdbus monitor --session --dest org.freedesktop.impl.portal.PermissionStore \
    > "$W/bus.log" 2>&1 &
MON_PID=$!
sleep 1

"$W/client" "$APP" pipewire-0 > "$W/client.log" 2>&1 &
CLIENT_PID=$!
sleep 3

cid="$(pw-dump Client 2>/dev/null | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    p = o.get("info", {}).get("props", {})
    if p.get("pipewire.access") == "portal":
        print(o["id"]); break
')"
[ -n "$cid" ] || {
    echo "COULD NOT RUN: no client was recorded with pipewire.access = portal,"
    echo "               so there is nothing for access-portal.lua to act on."
    pw-dump Client 2>/dev/null | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    p = o.get("info", {}).get("props", {})
    print("   client %s access=%r socket=%r" % (o["id"], p.get("pipewire.access"), p.get("pipewire.sec.socket")))'
    exit 0; }

echo
echo "the client, as the SERVER sees it:"
pw-dump Client 2>/dev/null | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    p = o.get("info", {}).get("props", {})
    if p.get("pipewire.access") == "portal":
        print("   id=%s access=%r socket=%r app_id=%r roles=%r" % (
            o["id"], p.get("pipewire.access"), p.get("pipewire.sec.socket"),
            p.get("pipewire.access.portal.app_id"),
            p.get("pipewire.access.portal.media_roles")))'

before="$(pw-cli get-permissions "$cid" 2>&1)"
echo
echo "its permissions with the grant in place (store = yes):"
sed 's/^/   /' <<<"$before"

echo
echo ">>> store := no.  The client keeps running and is NOT restarted."
wpbefore="$(wc -l < "$W/wireplumber.log")"
store no
sleep 6
after="$(pw-cli get-permissions "$cid" 2>&1)"
echo "its permissions six seconds later:"
sed 's/^/   /' <<<"$after"

echo
echo "the store now reads:"
sed 's/^/   /' <<<"$(lookup)"
echo
echo "the Changed signal on the bus:"
grep -a "Changed" "$W/bus.log" | sed 's/^/   /' || echo "   (none — the store emitted nothing)"
echo
echo "what access-portal.lua logged while the store changed:"
tail -n +$((wpbefore + 1)) "$W/wireplumber.log" | grep -i "access-portal" | sed 's/^/   /' \
    || echo "   (nothing)"
echo
echo "…and that it DOES act at connect time, from earlier in the same log:"
grep -i "access-portal.lua:8[0-9]\|access-portal.lua:3[0-9]" "$W/wireplumber.log" \
    | head -3 | sed 's/^/   /' || echo "   (nothing)"

echo
echo "── the answer ───────────────────────────────────────────────"
if [ "$before" = "$after" ]; then
    echo "NO. The running client's permissions are unchanged six seconds after"
    echo "    a permission-store write that the bus confirms was emitted."
    echo "    Timing::NextRequest is the correct statement, not merely the"
    echo "    cautious one, and Timing::Immediate stays unreachable."
else
    echo "YES. The running client's permissions CHANGED after the store write."
    echo "     Before and after differ, so a camera revocation does reach a"
    echo "     live client on this stack. apex-perm-core's Timing::Immediate"
    echo "     and docs/app-permissions.md §2.4 both need revisiting — that is"
    echo "     a decision for a person, which is why this exits 0 either way."
fi

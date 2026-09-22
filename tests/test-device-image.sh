#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-device-image.sh — the printing, share, network and dock packages the
#  image has to carry, and the units that decide whether they do anything.
#
#  ── Why this file exists ────────────────────────────────────────────────────
#  Each gap it guards was found the same way: the package was there and
#  nothing enabled or configured it, or it was missing behind a package name
#  that looked like it covered the case. `gvfs gvfs-mtp gvfs-smb` reads as a
#  share stack, and without gvfs-fuse a mounted share exists inside GTK and
#  nowhere else. hplip reads as HP support, and its SANE backend made
#  `scanimage -L` hang without returning. bolt was absent, so a Thunderbolt
#  dock sat at authorized=0 with nothing on the machine able to authorise it.
#
#  None of that is visible in a package list, so each case here says what breaks
#  rather than which string is missing.
#
#  ── The parser, and why it is not grep ──────────────────────────────────────
#  A `RUN` in a Containerfile is one shell command spread over many lines, and
#  BuildKit strips whole-line comments even in the middle of a continuation. So
#  a grep for `bolt` matches the word "bolted" in a prose comment, and a grep
#  for a package that only appears in a commented-out line reports it as
#  shipped. This joins instructions the way the builder does, drops the
#  comments first, and then asks whether a name is inside an install list.
#
#  It also parses each joined RUN body with `bash -n`. This repository has
#  shipped a Containerfile-escaping bug before, and the failure mode is a build
#  that dies twenty minutes in.
#
#  PASS = the packages sit in an install list, the daemons that need enabling
#         have it, and the ones that must not be enabled do not.
#
#  Run from anywhere: ./tests/test-device-image.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

CORE=Containerfile.core
BASE=Containerfile.base
NMCONF=files/system/NetworkManager/21-apex-connectivity.conf
for f in "$CORE" "$BASE"; do
    [ -f "$f" ] || { echo "cannot find $f"; exit 2; }
done
command -v python3 >/dev/null 2>&1 || { echo "this file needs python3"; exit 2; }

pass=0; fail=0
ok()  { printf 'PASS  %-60s\n' "$1"; pass=$((pass+1)); }
bad() { printf 'FAIL  %-60s %s\n' "$1" "$2"; fail=$((fail+1)); }

WORK=$(mktemp -d /tmp/apex-devimg.XXXXXX) || exit 2
trap 'rm -rf "$WORK"' EXIT

# ── join instructions the way the builder does ──────────────────────────────
python3 - "$CORE" "$BASE" > "$WORK/joined" <<'PY'
import sys
def instructions(path):
    out, buf = [], None
    for raw in open(path):
        line = raw.rstrip("\n")
        if line.lstrip().startswith("#"):
            continue                      # BuildKit drops these, continuation or not
        if buf is None:
            if not line.strip():
                continue
            buf = line
        else:
            buf = buf[:-1] + line if buf.endswith("\\") else buf + "\n" + line
        if buf.endswith("\\"):
            continue
        out.append(buf); buf = None
    if buf:
        out.append(buf)
    return out
for p in sys.argv[1:]:
    for ins in instructions(p):
        print(ins.replace("\n", " "))
PY

JOINED="$WORK/joined"

# Each `dnf5 … install` list, extracted once. An install command runs to the
# next `;`, so that is the window a package name has to appear in.
#
# Extracted to a file rather than piped into each test, and that is not tidiness.
# `grep -oE … | grep -q …` looks right and is a coin flip: `grep -q` exits at the
# first match and closes the pipe, the upstream grep takes SIGPIPE and returns
# 141, and `pipefail` hands that back as the verdict. It cost two false failures
# here before the second run passed with nothing changed, which is the worst
# possible behaviour for a test whose whole job is to be believed.
grep -oE 'dnf5 [^;]*install[^;]*' "$JOINED" > "$WORK/installs" || true

# Is a package inside an install list, or does it only get mentioned?
installed() { grep -qE "(^|[[:space:]])$1([[:space:]]|$)" "$WORK/installs"; }
# Does any joined instruction contain this text?
present() { grep -qF -- "$1" "$JOINED"; }

pkg() {  # $1 = package, $2 = what breaks without it
    installed "$1" && ok "$1 is installed" || bad "$1 is installed" "$2"
}
asserts() {  # $1 = literal the build must contain, $2 = case name, $3 = why
    present "$1" && ok "$2" || bad "$2" "$3"
}

# ── each RUN body is valid shell once the builder has joined it ─────────────
echo "── the Containerfiles parse as the builder will read them ─────────────"
python3 - "$CORE" "$BASE" > "$WORK/synerr" <<'PY'
import sys, subprocess, tempfile, os
def instructions(path):
    out, buf = [], None
    for raw in open(path):
        line = raw.rstrip("\n")
        if line.lstrip().startswith("#"):
            continue
        if buf is None:
            if not line.strip():
                continue
            buf = line
        else:
            buf = buf[:-1] + line if buf.endswith("\\") else buf + "\n" + line
        if buf.endswith("\\"):
            continue
        out.append(buf); buf = None
    if buf:
        out.append(buf)
    return out
for p in sys.argv[1:]:
    for ins in instructions(p):
        if not ins.startswith("RUN "):
            continue
        with tempfile.NamedTemporaryFile("w", suffix=".sh", delete=False) as f:
            f.write(ins[4:]); tmp = f.name
        r = subprocess.run(["bash", "-n", tmp], capture_output=True, text=True)
        if r.returncode:
            print(f"{p}: {ins[4:80]} :: {r.stderr.strip()[:120]}")
        os.unlink(tmp)
PY
if [ ! -s "$WORK/synerr" ]; then
    ok "each RUN body parses once continuations are joined"
else
    bad "each RUN body parses once continuations are joined" "$(head -1 "$WORK/synerr")"
fi

echo
echo "── printing and scanning ──────────────────────────────────────────────"
pkg cups            "nothing on the machine can print at all"
pkg sane-airscan    "no driverless eSCL or WSD scanning, which is how modern all-in-ones scan"
pkg simple-scan     "the only way to scan is scanimage on a terminal"
pkg nss-mdns        "a printer's .local name resolves to nothing"
asserts 'systemctl enable cups.socket' \
        "cupsd is socket-activated" \
        "cups.socket is not enabled, so nothing starts cupsd when a print job appears"
asserts 'systemctl enable avahi-daemon.service' \
        "avahi is enabled, so a driverless printer can be found" \
        "avahi is not enabled — DNS-SD discovery, .local resolution and \`apex host\` all depend on it"
asserts 'FATAL: avahi is not enabled' \
        "the build fails if the avahi enable did not take" \
        "Fedora's preset does the enable, so nothing here would notice the preset changing"
asserts 'command -v simple-scan' \
        "the scanning app is asserted on the built image" \
        "simple-scan is requested and never verified"

echo
echo "── the SANE backend list is left as the packages ship it ──────────────"
# An earlier version of this branch rewrote /etc/sane.d/dll.d/hpaio to disable
# hplip's scanner backend, on the strength of `scanimage -L` never returning.
# It does not return — on a machine where avahi is unavailable. hpaio links
# libavahi-client and probes the network in sane_hpaio_get_devices, and with no
# avahi to answer it waits past 400 seconds instead of failing. On a second
# machine running the same image with avahi up, the same command with the same
# backend list returns in twelve seconds and finds the same scanner. So the
# defect is avahi's, and disabling hpaio would have removed HP's USB scanner
# path to work around it. The condition is reported by `apex devices scan`
# instead.
if present '/etc/sane.d/dll.d/hpaio'; then
    bad "the shipped SANE backend list is left alone" \
        "hpaio is being rewritten again — the hang is avahi's absence, not hplip's presence"
else
    ok "the shipped SANE backend list is left alone"
fi

echo
echo "── network shares, which were a stack with its floor missing ──────────"
pkg gvfs-fuse    "a mounted share exists inside GTK and nowhere else: no /run/user/UID/gvfs"
pkg gvfs-gphoto2 "cameras and phones in PTP mode appear nowhere"
pkg gvfs-nfs     "NFS is mountable as root and invisible to the file manager"
pkg gvfs-smb     "no SMB in the file manager"
pkg cifs-utils   "no mount.cifs, so no kernel SMB mount and nothing to put in fstab"
pkg samba-client "no smbclient to diagnose any of the above"
asserts 'test -x /usr/libexec/gvfsd-fuse' \
        "gvfsd-fuse is asserted on the built image" \
        "the package is requested and the binary never checked"
asserts 'test -x /usr/libexec/gvfsd-dav' \
        "WebDAV's backend is asserted on the built image" \
        "gvfsd-dav ships inside base gvfs and nothing pins it"

echo
echo "── networking: VPN, WireGuard, hotspot, captive portal ────────────────"
pkg wireguard-tools           "the kernel speaks WireGuard and there is no wg to configure it with"
pkg NetworkManager-openvpn    "a .ovpn file cannot be imported: NM has no built-in OpenVPN"
pkg NetworkManager-openconnect "no AnyConnect, GlobalProtect or Pulse"
pkg NetworkManager-vpnc       "no legacy Cisco IPsec"
pkg dnsmasq                   "a hotspot activates and hands out no addresses"
asserts 'command -v wg-quick' \
        "wg-quick is asserted on the built image" \
        "the tools are requested and never verified"
asserts '/usr/lib/NetworkManager/VPN/nm-openvpn-service.name' \
        "the OpenVPN plugin is asserted where NM looks for it" \
        "the package could install and register nothing"
# NM spawns its own dnsmasq for a shared connection. A second one enabled as a
# system service would take port 53 and fight it.
if present 'systemctl enable dnsmasq'; then
    bad "dnsmasq is NOT enabled as a system service" \
        "a resident resolver on port 53 would fight the one NM spawns for the hotspot"
else
    ok "dnsmasq is NOT enabled as a system service"
fi
asserts 'files/system/NetworkManager/21-apex-connectivity.conf' \
        "the connectivity check is shipped" \
        "NM has no \`portal\` state, so a captive portal reads as a working connection"
for key in enabled=true uri= response=; do
    if [ -f "$NMCONF" ] && grep -qE "^$key" "$NMCONF"; then
        ok "the connectivity check sets $key"
    else
        bad "the connectivity check sets $key" \
            "without it the check is a no-op that still reads as configured"
    fi
done

echo
echo "── Bluetooth, Thunderbolt and docks ───────────────────────────────────"
pkg bolt              "nothing authorises a Thunderbolt device: a dock sits at authorized=0"
pkg bluez-obexd       "no file transfer to or from a phone over Bluetooth"
pkg pipewire-codec-aptx "aptX headsets fall back to SBC and nobody can tell why"
asserts 'test -x /usr/libexec/boltd' \
        "boltd is asserted on the built image" \
        "the package is requested and the daemon never checked"
asserts 'SUBSYSTEM=="thunderbolt"' \
        "the udev rule that wakes boltd is asserted" \
        "boltd ships D-Bus activated, and APEX runs no desktop that would call it — the udev rule is what starts it"
asserts 'libspa-codec-bluez5-aptx.so' \
        "the aptX codec is asserted on the built image" \
        "RPM Fusion could be skipped and the codec silently absent"
asserts 'libspa-codec-bluez5-ldac.so' \
        "the LDAC codec is asserted on the built image" \
        "LDAC arrived as somebody's dependency and nothing pins it"
asserts 'systemctl enable bluetooth.service' \
        "bluetooth is enabled" \
        "no adapter comes up on first boot"
# bolt.service is Type=dbus and woken by udev. Enabling it would start a
# Thunderbolt daemon on every machine, including the ones with no controller.
if present 'systemctl enable bolt'; then
    bad "bolt.service is NOT enabled" \
        "it is Type=dbus and udev-woken; enabling it runs a Thunderbolt daemon on hardware that has none"
else
    ok "bolt.service is NOT enabled"
fi

echo
printf 'device image: %d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]

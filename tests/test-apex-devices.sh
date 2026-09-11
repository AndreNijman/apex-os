#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  test-apex-devices.sh — that the enumeration tells "there is none" apart from
#  "I could not look", and names the firewall where a device fails because of it.
#
#  ── Why the fixtures are shaped this way ────────────────────────────────────
#  The EACCES sweep across this codebase found the same bug in fourteen places,
#  and it named the reason the tests never caught any of them: the fixtures were
#  owned by whoever ran the tests, so nothing ever hit EACCES. A test whose
#  directories it can always read cannot tell a reader that collapses
#  "permission denied" into "absent" from one that does not.
#
#  So the sealed-directory cases here `chmod 0000` a fixture AND check the seal
#  actually took before asserting anything — root walks through 0000, and a
#  green run as root would be a green run that tested nothing. They skip in that
#  case rather than passing.
#
#  Everything else is a PATH shim. `systemctl`, `nmcli`, `lpstat`, `cupsctl`,
#  `bluetoothctl`, `rfkill` and `scanimage` become small scripts, so the states
#  that matter — a masked unit, a wedged daemon, a scanner list that never
#  arrives — can be produced on a machine that has none of them.
#
#  ── What it will not do ─────────────────────────────────────────────────────
#  It never runs the helper against the real machine's systemd, NetworkManager
#  or bluetoothd. Every case here runs with a shimmed PATH and a fixture tree.
#
#  PASS = each reader distinguishes the three answers, the firewall is named at
#         the point of failure, and nothing waits forever.
#
#  Run from anywhere: ./tests/test-apex-devices.sh
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

HELPER=files/system/libexec/apex-devices
[ -f "$HELPER" ] || { echo "cannot find $HELPER"; exit 2; }
HELPER_ABS="$PWD/$HELPER"

pass=0; fail=0; skip=0
ok()      { printf 'PASS  %-62s\n' "$1"; pass=$((pass+1)); }
bad()     { printf 'FAIL  %-62s %s\n' "$1" "$2"; fail=$((fail+1)); }
skipped() { printf 'SKIP  %-62s %s\n' "$1" "$2"; skip=$((skip+1)); }

WORK=$(mktemp -d /tmp/apex-devtest.XXXXXX) || exit 2
cleanup() { chmod -R u+rwX "$WORK" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

BIN="$WORK/bin"; mkdir -p "$BIN"

# ── shims ───────────────────────────────────────────────────────────────────
# Each reads a file under $WORK/state, so a case sets the world by writing one
# line rather than by rewriting a shim.
STATE="$WORK/state"; mkdir -p "$STATE"
shim() { printf '#!/usr/bin/env bash\n%s\n' "$2" > "$BIN/$1"; chmod +x "$BIN/$1"; }

shim systemctl '
verb=$1; unit=${2:-}
case "$verb" in
  is-enabled) cat "$STATE/enabled.$unit" 2>/dev/null || echo "not-found" ;;
  is-active)  cat "$STATE/active.$unit"  2>/dev/null || echo "inactive" ;;
  *) exit 1 ;;
esac'
shim lpstat      'cat "$STATE/lpstat" 2>/dev/null; exit "$(cat "$STATE/lpstat.rc" 2>/dev/null || echo 0)"'
shim cupsctl     'cat "$STATE/cupsctl" 2>/dev/null'
shim nmcli       'cat "$STATE/nmcli.$*" 2>/dev/null || cat "$STATE/nmcli" 2>/dev/null; exit "$(cat "$STATE/nmcli.rc" 2>/dev/null || echo 0)"'
shim bluetoothctl 'cat "$STATE/bluetoothctl" 2>/dev/null'
shim rfkill      'cat "$STATE/rfkill" 2>/dev/null'
shim scanimage   'if [ -e "$STATE/scanimage.hang" ]; then sleep 300; fi; cat "$STATE/scanimage" 2>/dev/null'
shim rpm         'exit "$(cat "$STATE/rpm.rc" 2>/dev/null || echo 0)"'
shim busctl      'cat "$STATE/busctl" 2>/dev/null'
shim dnsmasq     'true'
shim wg          'true'

# `timeout` and the shell builtins have to stay reachable.
export PATH="$BIN:/usr/bin:/bin"
export STATE

# Run the helper in a fixture world. $1 = area, rest = env assignments.
devices() {
    local area=$1; shift
    env -u XDG_SEAT APEX_DEVICES_SYSFS="$WORK/sys" \
        APEX_DEVICES_ETC="$WORK/etc" \
        APEX_DEVICES_SPA="$WORK/spa" \
        APEX_DEVICES_NMVPN="$WORK/nmvpn" \
        APEX_DEVICES_LIBEXEC="$WORK/libexec" \
        APEX_DEVICES_DEV="$WORK/dev" \
        APEX_DEVICES_MEDIA="$WORK/media" \
        APEX_DEVICES_NMLIB="$WORK/nmlib" \
        APEX_DEVICES_NMCONFD="$WORK/nmconfd" \
        "$@" bash "$HELPER_ABS" "$area" 2>&1
}

says()    { grep -qi -- "$2" <<<"$1"; }
case_says() {  # $1 = name, $2 = output, $3 = substring, $4 = why it matters
    says "$2" "$3" && ok "$1" || bad "$1" "output never said '$3' — $4"
}
case_silent() {  # the opposite: it must NOT say this
    says "$2" "$3" && bad "$1" "output said '$3', and it must not — $4" || ok "$1"
}

reset_world() {
    chmod -R u+rwX "$WORK/sys" "$WORK/dev" "$WORK/media" 2>/dev/null
    rm -rf "$WORK/sys" "$WORK/etc" "$WORK/spa" "$WORK/nmvpn" "$WORK/libexec" \
           "$WORK/dev" "$WORK/media" "$WORK/nmlib" "$WORK/nmconfd" "$STATE"
    mkdir -p "$WORK/sys/class" "$WORK/sys/bus" "$WORK/etc" "$WORK/spa" "$WORK/nmvpn" \
             "$WORK/libexec" "$WORK/dev" "$WORK/media" "$WORK/nmlib/1.54.3" \
             "$WORK/nmconfd" "$STATE"
    # A Wi-Fi-capable NetworkManager unless a case says otherwise: without the
    # plugin the enterprise reader stops before anything else it checks.
    : > "$WORK/nmlib/1.54.3/libnm-device-plugin-wifi.so"
}

# ── absence and refusal are different answers ───────────────────────────────
echo "── a device that is not there, and a device that cannot be looked at ──"

reset_world
out=$(devices bluetooth)
case_says "no bluetooth class reads as none" "$out" "none" \
          "an absent subsystem has to read as an absence"
case_silent "and it does not claim it could not look" "$out" "could not be read" \
            "absent and unreadable are different facts"

reset_world
mkdir -p "$WORK/sys/class/bluetooth"
out=$(devices bluetooth)
case_says "an empty bluetooth class reads as none" "$out" "none" \
          "the subsystem exists with no adapter bound"

# The case the whole file exists for.
reset_world
mkdir -p "$WORK/sys/class/bluetooth/hci0"
chmod 0000 "$WORK/sys/class/bluetooth"
if ls "$WORK/sys/class/bluetooth" >/dev/null 2>&1; then
    chmod 0755 "$WORK/sys/class/bluetooth"
    skipped "an unreadable bluetooth class reads as UNAVAILABLE" \
            "the 0000 seal did not hold — running as root walks through it"
    skipped "and it does not report an absence" "same"
else
    out=$(devices bluetooth)
    chmod 0755 "$WORK/sys/class/bluetooth"
    case_says "an unreadable bluetooth class reads as UNAVAILABLE" "$out" "could not be read" \
              "this is the fourteen-places bug: a refused read is not an empty list"
    case_says "and it says the two are not the same" "$out" "not the same as having no adapter" \
              "the message has to tell the reader which question was answered"
fi

reset_world
mkdir -p "$WORK/sys/bus/thunderbolt/devices/0-1"
chmod 0000 "$WORK/sys/bus/thunderbolt/devices"
if ls "$WORK/sys/bus/thunderbolt/devices" >/dev/null 2>&1; then
    chmod 0755 "$WORK/sys/bus/thunderbolt/devices"
    skipped "an unreadable thunderbolt bus reads as UNAVAILABLE" "the 0000 seal did not hold"
else
    out=$(devices dock)
    chmod 0755 "$WORK/sys/bus/thunderbolt/devices"
    case_says "an unreadable thunderbolt bus reads as UNAVAILABLE" "$out" "could not be read" \
              "a refused bus read would otherwise report a machine with no controller"
fi

# ── the dock nobody authorised ──────────────────────────────────────────────
echo
echo "── a dock that is attached and not authorised ─────────────────────────"
reset_world
mkdir -p "$WORK/sys/bus/thunderbolt/devices/domain0" "$WORK/sys/bus/thunderbolt/devices/0-1"
echo user > "$WORK/sys/bus/thunderbolt/devices/domain0/security"
echo 1    > "$WORK/sys/bus/thunderbolt/devices/domain0/iommu_dma_protection"
echo 0    > "$WORK/sys/bus/thunderbolt/devices/0-1/authorized"
echo "CalDigit TS4" > "$WORK/sys/bus/thunderbolt/devices/0-1/device_name"
out=$(devices dock)
case_says "an unauthorised device is named as such" "$out" "NOT AUTHORISED" \
          "authorized=0 is the state a dock sits in forever with no boltd"
case_says "and the reason nothing will fix it is stated" "$out" "boltd is not" \
          "without boltd there is no client of org.freedesktop.bolt on APEX at all"
case_says "and what the user loses is stated" "$out" "no video, no USB and no network" \
          "'not authorised' means nothing to somebody whose monitor is dark"

# ── the firewall, named where the device fails ──────────────────────────────
echo
echo "── a shared printer that the firewall is silently blocking ────────────"
reset_world
echo active  > "$STATE/active.apex-firewall.service"
echo enabled > "$STATE/enabled.cups.socket"
echo enabled > "$STATE/enabled.avahi-daemon.service"
echo active  > "$STATE/active.avahi-daemon.service"
printf 'printer office is idle\n' > "$STATE/lpstat"
printf '_share_printers=1\n'      > "$STATE/cupsctl"
out=$(devices print)
case_says "sharing a printer behind a closed port names the firewall" "$out" "THE FIREWALL IS WHY" \
          "this is the worst outcome available: a share that fails with no mention of the policy"
case_says "and it gives the exact command" "$out" "apex firewall allow ipp" \
          "naming the cause without the remedy leaves the user where they started"

# With the exception recorded, it must stop blaming the firewall.
mkdir -p "$WORK/etc/apex/firewall.d"; echo "tcp 631" > "$WORK/etc/apex/firewall.d/ipp.conf"
out=$(devices print)
case_silent "an allowed service is not blamed on the firewall" "$out" "THE FIREWALL IS WHY" \
            "ipp is in the exception list, so the policy is not what is wrong"
case_says "and it says the live ruleset needs root" "$out" "needs root" \
          "reading the exception list is not the same as reading the loaded rules"

# And with no firewall running at all, it must not claim one is in the way.
reset_world
echo inactive > "$STATE/active.apex-firewall.service"
printf '_share_printers=1\n' > "$STATE/cupsctl"
out=$(devices print)
case_silent "no running firewall is never blamed" "$out" "THE FIREWALL IS WHY" \
            "claiming a policy blocks something where no policy is loaded is its own lie"

# ── the masked unit, which no image update can fix ──────────────────────────
echo
echo "── avahi masked: discovery gone, and the scanner list never arriving ──"
reset_world
echo masked > "$STATE/enabled.avahi-daemon.service"
out=$(devices print)
case_says "a masked avahi is reported as masked" "$out" "MASKED" \
          "'not running' would send the user to systemctl start, which fails on a mask"
case_says "and it says an update will not fix it" "$out" "survives" \
          "a mask lives in /etc and outlives every image"
case_says "and it gives the unmask command" "$out" "systemctl unmask avahi-daemon" \
          "the remedy for a mask is not the remedy for a stopped unit"

# The scanner list must not be attempted while avahi is masked, because it
# would not come back. Measured: 400+ seconds on a real machine.
reset_world
echo masked > "$STATE/enabled.avahi-daemon.service"
touch "$STATE/scanimage.hang"
t0=$(date +%s); out=$(devices scan); t1=$(date +%s)
if [ $((t1-t0)) -lt 20 ]; then
    ok "a masked avahi short-circuits the scanner probe, in $((t1-t0))s"
else
    bad "a masked avahi short-circuits the scanner probe" \
        "took $((t1-t0))s — it ran scanimage anyway, which is the hang this exists to avoid"
fi
case_says "and it says it could not look, not that there is nothing" "$out" "unavailable" \
          "no scanner and cannot enumerate scanners are different answers"

# With avahi up, a hanging scanimage must be cut off rather than inherited.
reset_world
echo enabled > "$STATE/enabled.avahi-daemon.service"
echo active  > "$STATE/active.avahi-daemon.service"
touch "$STATE/scanimage.hang"
t0=$(date +%s); out=$(devices scan); t1=$(date +%s)
if [ $((t1-t0)) -lt 45 ]; then
    ok "a wedged scanimage is cut off, in $((t1-t0))s"
else
    bad "a wedged scanimage is cut off" "took $((t1-t0))s, so the timeout is not doing its job"
fi
case_says "a cut-off probe reads as unavailable" "$out" "unavailable" \
          "a timeout is a thing that was not measured, not a measurement of zero"
case_says "and it names the backend that does this" "$out" "hpaio" \
          "the cause is specific and nameable, so name it"

# A genuinely empty list is still an absence.
reset_world
echo enabled > "$STATE/enabled.avahi-daemon.service"
echo active  > "$STATE/active.avahi-daemon.service"
printf 'No scanners were identified.\n' > "$STATE/scanimage"
out=$(devices scan)
case_says "an empty scanner list reads as none found" "$out" "none found" \
          "a real absence has to stay a measurement"

# ── NetworkManager: refused is not empty ────────────────────────────────────
echo
echo "── links, portals and the VPN protocols that were not there ───────────"
reset_world
echo 4 > "$STATE/nmcli.rc"
printf 'Error: NetworkManager is not running.\n' > "$STATE/nmcli"
out=$(devices network)
case_says "an unreachable NetworkManager reads as unavailable" "$out" "unavailable" \
          "nmcli failing is not a machine with no network interfaces"
case_says "and it says which question it answered" "$out" "not the same as having no network" \
          "the distinction is the whole point"

reset_world
printf 'wlp3s0:wifi:connected\n' > "$STATE/nmcli"
printf 'portal\n' > "$STATE/nmcli.-t networking connectivity"
out=$(devices network)
case_says "a captive portal is reported as one" "$out" "captive portal" \
          "NM's portal state is the only signal a hotel network gives"

# The one that fires on the shipped image. With no connectivity URI configured,
# NetworkManager answers FULL for any connected link without asking anyone —
# measured on the L16: ConnectivityCheckEnabled false, connectivity full. That is
# "I did not look" arriving as a positive result, which is this suite's subject.
reset_world
printf 'wlp3s0:wifi:connected\n' > "$STATE/nmcli"
printf 'full\n' > "$STATE/nmcli.-t networking connectivity"
printf 'b false\n' > "$STATE/busctl"
out=$(devices network)
case_says "an unchecked 'full' is not passed off as a working connection" "$out" "nothing checked" \
          "a portal looks exactly like this, and nothing else on the machine says so"
case_silent "and it is not described as checked" "$out" "and checked" \
            "NetworkManager assumed; describing that as a check would be the lie"

printf 'b true\n' > "$STATE/busctl"
out=$(devices network)
case_says "a 'full' that was actually checked says so" "$out" "full, and checked" \
          "the distinction is worthless unless the good case is distinguishable too"

reset_world
printf 'wlp3s0:wifi:connected\n' > "$STATE/nmcli"
out=$(devices network)
case_says "an empty VPN directory reads as no VPN protocols" "$out" "VPN protocols" \
          "NM has no built-in VPN beyond WireGuard"
case_says "and it says why that means .ovpn cannot be imported" "$out" "cannot be imported" \
          "'no plugins' does not tell a user what they cannot do"

# The hotspot blockers, which are the P2-006 finding.
reset_world
printf 'wlp3s0:wifi:connected\n' > "$STATE/nmcli"
echo active > "$STATE/active.apex-firewall.service"
rm -f "$BIN/dnsmasq"
out=$(devices network)
shim dnsmasq 'true'
case_says "the hotspot blockers are counted, not summarised" "$out" "would stop it" \
          "a hotspot fails for three separate reasons and a user needs all three"
case_says "and the DHCP direction is named" "$out" "matches replies" \
          "the policy's DHCP rule is the reply direction; a hotspot needs the request one"

# dnsmasq from a system extension is present and impermanent.
reset_world
printf 'wlp3s0:wifi:connected\n' > "$STATE/nmcli"
echo 1 > "$STATE/rpm.rc"
out=$(devices network)
case_says "a dnsmasq no package owns is flagged as impermanent" "$out" "system extension" \
          "a binary from a merged extension goes away when the extension does"

# ── shares outside GTK ──────────────────────────────────────────────────────
echo
echo "── a share that is mounted and still unusable ─────────────────────────"
# Both halves are reachable now that $LIBEXEC is a fixture. They were not: the
# helper hardcoded /usr/libexec, so this case was green only on a machine that
# happened to lack the package, and would have gone permanently unexercised the
# day the package landed in the image.
reset_world
out=$(devices share)
case_says "no gvfs-fuse is called out as mounts existing only inside GTK" "$out" "nowhere else" \
          "the file manager opens the share and nothing else in the session can see it"
case_says "and it names the two states that differ" "$out" "Mounted and usable" \
          "'not mounted' is what a user would otherwise conclude, and it is wrong"

reset_world
: > "$WORK/libexec/gvfsd-fuse"; chmod +x "$WORK/libexec/gvfsd-fuse"
out=$(devices share)
case_silent "with gvfs-fuse present it stops saying mounts are invisible" "$out" "nowhere else" \
            "the warning has to go away when the thing it warns about is installed"

# ── a firewall that meant to be running and is not ──────────────────────────
echo
echo "── a firewall unit that failed, which is not a firewall that is off ────"
reset_world
echo failed  > "$STATE/active.apex-firewall.service"
echo enabled > "$STATE/enabled.apex-firewall.service"
printf '_share_printers=1\n' > "$STATE/cupsctl"
out=$(devices print)
case_says "a failed firewall unit is reported as a failure" "$out" "FAILED to load" \
          "the machine is unfiltered and was configured not to be; 'not running' hides that"
case_silent "and it is not blamed for the printer" "$out" "THE FIREWALL IS WHY" \
            "a policy that never loaded is not what is stopping the print job"

# ── removable media ─────────────────────────────────────────────────────────
echo
echo "── SD cards, USB disks, and the seat that has to exist to mount one ────"
reset_world
out=$(devices media)
case_says "no MMC host reads as no SD slot" "$out" "none" \
          "this laptop genuinely has none, and that is a measurement"
case_says "and a USB card reader is excluded from that claim" "$out" "usb-storage" \
          "'no SD slot' would otherwise tell someone their card reader cannot work"
case_says "no udisks2 means nothing mounts what appears" "$out" "nothing mounts it" \
          "the block device is created either way; the mount is what goes missing"

reset_world
mkdir -p "$WORK/sys/class/mmc_host/mmc0/mmc0:0001"
echo active > "$STATE/active.udisks2.service"
echo enabled > "$STATE/enabled.udisks2.service"
out=$(devices media)
case_says "a card in the slot is counted" "$out" "1 card(s) in" \
          "a host with no card and a host with one are different answers"

reset_world
mkdir -p "$WORK/sys/class/mmc_host/mmc0"
chmod 0000 "$WORK/sys/class/mmc_host"
if ls "$WORK/sys/class/mmc_host" >/dev/null 2>&1; then
    chmod 0755 "$WORK/sys/class/mmc_host"
    skipped "an unreadable mmc_host reads as UNAVAILABLE" "the 0000 seal did not hold"
else
    out=$(devices media)
    chmod 0755 "$WORK/sys/class/mmc_host"
    case_says "an unreadable mmc_host reads as UNAVAILABLE" "$out" "could not be read" \
              "a refused read of the subsystem is not a laptop without a card slot"
fi

reset_world
echo masked > "$STATE/enabled.udisks2.service"
out=$(devices media)
case_says "a masked udisks2 is reported as masked" "$out" "MASKED" \
          "no removable disk appears anywhere in the desktop and nothing says why"

reset_world
out=$(devices media)
case_says "a session with no seat is told mounting will be refused" "$out" "a refusal rather than an absence" \
          "over ssh the disk is present and unmountable, which reads as a dead port"
out=$(devices media XDG_SEAT=seat0)
case_silent "and a seated session is not warned about it" "$out" "a refusal rather than an absence" \
            "the warning is about this session, not about the machine"

# ── enterprise Wi-Fi ────────────────────────────────────────────────────────
echo
echo "── WPA-Enterprise: the plugin, the backend and the certificate ────────"
reset_world
rm -f "$WORK/nmlib"/*/libnm-device-plugin-wifi.so
out=$(devices network)
case_says "a NetworkManager with no Wi-Fi plugin is named" "$out" "libnm-device-plugin-wifi.so" \
          "without it no wireless device appears at all, and nmcli says only 'no device'"

reset_world
mkdir -p "$WORK/etc/NetworkManager/conf.d"
printf '[device]\nwifi.backend=iwd\n' > "$WORK/etc/NetworkManager/conf.d/10-backend.conf"
out=$(devices network)
case_says "a backend selected in conf.d and absent is named" "$out" "IT IS NOT INSTALLED" \
          "NM brings up no wireless connection at all and reports nothing about why"

reset_world
printf '802-11-wireless:eduroam\n' > "$STATE/nmcli.-t -f TYPE,NAME connection show"
printf '802-1x.eap:peap\n802-1x.ca-cert:--\n802-1x.system-ca-certs:no\n' \
    > "$STATE/nmcli.-t -f 802-1x.eap,802-1x.ca-cert,802-1x.system-ca-certs connection show eduroam"
out=$(devices network)
case_says "an 802.1X profile with no CA certificate is counted" "$out" "validating nothing" \
          "it connects and works, so nothing else on the machine will ever mention it"
case_says "and what that costs is stated" "$out" "crackable offline" \
          "'no CA certificate' means nothing to somebody whose Wi-Fi works"

printf '802-1x.eap:peap\n802-1x.ca-cert:--\n802-1x.system-ca-certs:yes\n' \
    > "$STATE/nmcli.-t -f 802-1x.eap,802-1x.ca-cert,802-1x.system-ca-certs connection show eduroam"
out=$(devices network)
case_silent "system-ca-certs=yes is not flagged" "$out" "crackable offline" \
            "the system trust store is a CA certificate; flagging it would be noise"

# ── hotplug ─────────────────────────────────────────────────────────────────
echo
echo "── hotplug, and the USB-C ports a missing controller used to hide ─────"
reset_world
out=$(devices dock)
case_says "a stopped udevd is named before anything is enumerated" "$out" "systemd-udevd is NOT running" \
          "with it down every reader below reports an absence, honestly and uselessly"

reset_world
echo active > "$STATE/active.systemd-udevd.service"
mkdir -p "$WORK/sys/class/typec/port0" "$WORK/sys/class/typec/port0-partner" \
         "$WORK/sys/class/typec/port1"
echo '[host] device' > "$WORK/sys/class/typec/port0/data_role"
echo 'host [device]' > "$WORK/sys/class/typec/port1/data_role"
out=$(devices dock)
case_says "no Thunderbolt controller no longer hides the USB-C ports" "$out" "USB-C ports" \
          "a machine with no TB controller still has ports, and docks live on them"
case_says "a plugged-in Type-C partner is counted" "$out" "1 with something plugged in" \
          "a partner appears on hotplug, so it is where hotplug can be seen working"

echo
printf 'apex-devices: %d passed, %d failed, %d skipped\n' "$pass" "$fail" "$skip"
[ "$fail" -eq 0 ]

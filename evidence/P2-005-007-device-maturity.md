# P2-005 / P2-006 / P2-007 — device maturity evidence

Branch `task/p2-005-device-maturity` in apex-os. Commits `36232f5`, `c823974`,
`f9ab0e7`, `3142bbd`, `5a7d0c6`, `fce0a99`, `5d5441b`, `1adbdb2`.

Four suites, every one run here rather than quoted, and every one
mutation-proved:

| Suite | Result | Mutations |
|---|---|---|
| `tests/test-device-image.sh` | 39 passed, 0 failed | 37/2 at `3142bbd` |
| `tests/test-device-services-netns.sh` | 12 passed, 0 failed | 11/1, 9/3, 9/3, 11/1 |
| `tests/test-apex-devices.sh` | 55 passed, 0 failed | 49/4, 52/1 ×5, 51/2, 54/1, 53/2 |
| `cargo test -p apex` | 315 passed, 0 failed | — |

`tests/run-clippy.sh` (container, `rust:1`, `--all-targets -D warnings`): clean.

## What is demonstrated and what is reasoned

There is no printer, scanner, dock, SD card, enterprise access point, VPN
endpoint or Bluetooth headset attached to either machine. Nothing below claims
one.

| Item | Criterion | Verdict | How |
|---|---|---|---|
| P2-005 | Printing | PARTIAL, demonstrated | Packages asserted at build. Discovery reaches the machine through the shipped firewall (netns, positive control on 5353); IPP inbound stays silent until 631 is in `allowed_tcp`, and `apex devices print` names that where a shared printer fails. No print job has been sent. |
| P2-005 | Scanning | DEMONSTRATED | katana finds `airscan:w1:CANON INC. TR4600 series` over WSD, whole run 6.9 s. The L16, whose avahi is masked, refuses to enumerate and says why. One program, two machines, one image. |
| P2-005 | SMB / NFS / WebDAV / MTP / cameras | PARTIAL, floor demonstrated | `cifs-utils`, `samba-client`, `gvfs-fuse`, `gvfs-nfs`, `gvfs-gphoto2` were missing from the image and are added. `apex devices share` reads each one live on both machines. No share has been mounted from a server. |
| P2-005 | SD | REASONED, one measurement | Neither machine has an MMC host and both report that with the usb-storage caveat. The mount path is measured in the negative on katana: no seat over ssh, so udisks2 would refuse. No card has been inserted. |
| P2-006 | Captive portal | DEMONSTRATED as absent | Measured: `ConnectivityCheckAvailable` false, `ConnectivityCheckEnabled` false, `ConnectivityCheckUri` empty, and `nmcli` answering `full` anyway on both machines. The shipped image cannot see a portal, and its `full` is an assumption. `21-apex-connectivity.conf` turns the check on and is not on either machine yet. No portal network has been joined. |
| P2-006 | WireGuard / OpenVPN | REASONED | `wireguard-tools` and the three NM VPN plugins were missing and are added; the reader reports both. No tunnel has been established. |
| P2-006 | Enterprise Wi-Fi | PARTIAL, demonstrated | Two saved 802.1X profiles on the L16, both EAP PEAP, both with no CA certificate and `system-ca-certs` off, found and named. Plugin, backend and trust-store branches are fixture-tested. No EAP handshake has been run. |
| P2-006 | Hotspot | DEMONSTRATED as blocked | Three namespaces: the policy drops forwarded traffic, its DHCP rule matches the reply direction rather than a client's request, and port 53 is shut. Handed to P1-044. |
| P2-007 | Headsets and codecs | PARTIAL, demonstrated | Codec list read from `/usr/lib64/spa-0.2/bluez5` on both: aac, faststream, g722, lc3, ldac, opus, sbc, no aptX. `pipewire-codec-aptx` is added. No headset has been connected. |
| P2-007 | Thunderbolt | DEMONSTRATED as broken, fix reasoned | L16: `domain0`/`domain1` at `security=user`, `iommu_dma_protection=1`, two host routers authorised, no boltd, so an attached device would sit at `authorized=0` forever. `bolt` is added. No Thunderbolt device has been attached. |
| P2-007 | USB-C docks | PARTIAL, demonstrated | L16 has two Type-C ports and one live partner; katana has no Thunderbolt controller and does have ports, which is why the USB-C reader no longer sits behind that early return. No dock has been attached. |
| P2-007 | Hotplug | REASONED, one live reading | udevd is checked before anything is enumerated, and the Type-C partner is the one thing on these machines that comes and goes with a cable. No plug event has been watched end to end. |

## Refusals a naive reader would have called absences

Five fired on real hardware rather than in a fixture:

1. **avahi masked on the L16.** `scanimage -L` does not return. The reader says
   "unavailable, avahi is masked", gives the unmask command, and skips the
   probe. katana, with avahi running, finds a real scanner.
2. **No seat over ssh on katana.** udisks2 would refuse to mount a card, so the
   reader says that instead of reporting an empty slot.
3. **`apex-firewall.service` is on neither machine.** Neither run blames a
   firewall for anything, because no policy is loaded to blame.
4. **dnsmasq on the L16 belongs to no package.** It arrived with an `apex
   install waydroid` extension, so a hotspot would have worked by coincidence
   and stopped the day that extension went. katana reports it plainly absent.
5. **NetworkManager's `full`.** Both machines report a fully connected link
   that nothing checked, and the first version of this reader passed that
   through as good news.

## The two runs

```
### L16 (Lenovo ThinkPad L16, 21SCCTO1WW) — 2026-09-07T05:07:34+08:00

Printing
  mDNS (avahi)               MASKED
      a printer that announces itself over DNS-SD cannot be found, and a
      .local name resolves to nothing. A mask lives in /etc and survives
      every image update, so no update will put this right. Undo it with:
          sudo systemctl unmask avahi-daemon.service avahi-daemon.socket
          sudo systemctl start avahi-daemon.socket
  cups.socket                enabled
  queues                     none configured
      you add a driverless printer on this network by name; see the mDNS line above
  sharing a printer          off

Scanning
  scanners                   unavailable — avahi is masked
      enumerating scanners will not return while avahi is masked: hplip's hpaio
      backend waits on it. This is not 'no scanner'; it is 'cannot look'.
          sudo systemctl unmask avahi-daemon.service avahi-daemon.socket
      then run this again. Removing hpaio from /etc/sane.d/dll.d/hpaio also
      works and costs HP's USB scanner support.

Network shares
  GIO mounts on disk         NO — gvfs-fuse is not installed
      a share opened in the file manager exists inside GTK and nowhere else:
      no /run/user/1000/gvfs, so a terminal, an editor or an installer
      cannot reach it. Mounted and usable are different states.
  SMB (file manager)         yes
  WebDAV (file manager)      yes
  NFS (file manager)         no — gvfsd-nfs is absent
  MTP (file manager)         yes
  cameras (file manager)     no — gvfsd-gphoto2 is absent
  SMB as a real mount        no — cifs-utils is absent, so nothing in fstab
  NFS as a real mount        yes (mount.nfs)
  this machine serves        nothing (no smbd, no nfs-server)

Removable media
  SD card slot               none — this kernel registered no MMC host
      a USB card reader is not one of these: it arrives over usb-storage
  auto-mount                 udisks2 running
  this session's seat        seat0
  mounted removable media    none
  camera (PTP) tooling       no — gphoto2 is absent

Networking
  links                      4
      wlp3s0         wifi       connected
      lo             loopback   connected (externally)
      p2p-dev-wlp3s0 wifi-p2p   disconnected
      enp1s0f0       ethernet   unavailable
  captive portal             not detected, because nothing checked
      NetworkManager has no connectivity URI here, so it answers 'full' for any
      connected link without asking anyone. A hotel or campus network that has
      addressed you and will answer nothing looks exactly like a working one.
      APEX ships /etc/NetworkManager/conf.d/21-apex-connectivity.conf to turn
      the check on; on this machine it is not in effect.
  VPN protocols              none
      NetworkManager has no built-in VPN beyond WireGuard: each protocol is a
      separate service in /usr/lib/NetworkManager/VPN, so a .ovpn file cannot be imported at all
  WireGuard                  kernel yes, tools no
      NetworkManager has WireGuard built in, but with no wg there is nothing
      to import a provider's .conf with
  Wi-Fi backend              wpa_supplicant
  enterprise Wi-Fi (802.1X)  2 saved profile(s), 2 validating nothing
      WIRELESS-2.4: EAP peap, no CA certificate, system CA certs off
      WIRELESS-5: EAP peap, no CA certificate, system CA certs off
      These connect. They trust whatever answers to the network's name, so an
      access point standing in for the real one collects the MSCHAPv2 exchange,
      and that is crackable offline at leisure. Set 802-1x.ca-cert on the
      profile, or 802-1x.system-ca-certs=yes.
  system CA trust store      readable
  hotspot / tethering        nothing known is in the way
      dnsmasq is here but no system package owns it: it came with a system extension and goes when that does

Bluetooth
  adapters                   1
      hci0
  radio                      not blocked
  paired devices             1
      Device E9:B9:90:99:B3:F9 ProtoArc T1 Plus
  headset codecs             aac faststream g722 lc3 ldac opus-g opus sbc 
      no aptX: a Qualcomm headset will fall back to SBC

Thunderbolt, USB4 and docks
  hotplug (udev)             systemd-udevd is running
  0-0                        authorised — (host router)
  1-0                        authorised — (host router)
  domain0                    security=user  iommu=1
  domain1                    security=user  iommu=1
  boltd                      not installed
      a device attached later will sit at authorized=0 with nothing to act on it
  USB-C ports                2, 1 with something plugged in
      port0    [host] device          something is attached
      port1    host [device]          nothing attached

```

```
### katana (Katana GF76 12UG), over ssh — 2026-09-07T05:07:35+08:00

Printing
  mDNS (avahi)               running
  cups.socket                enabled
  queues                     none configured
      you add a driverless printer on this network by name; see the mDNS line above
  sharing a printer          off

Scanning
  scanners                   2 found
      device `v4l:/dev/video0' is a Noname HD Webcam: HD Webcam virtual device
      device `airscan:w1:CANON INC. TR4600 series' is a WSD CANON INC. TR4600 series ip=192.168.1.121

Network shares
  GIO mounts on disk         NO — gvfs-fuse is not installed
      a share opened in the file manager exists inside GTK and nowhere else:
      no /run/user/1000/gvfs, so a terminal, an editor or an installer
      cannot reach it. Mounted and usable are different states.
  SMB (file manager)         yes
  WebDAV (file manager)      yes
  NFS (file manager)         no — gvfsd-nfs is absent
  MTP (file manager)         yes
  cameras (file manager)     no — gvfsd-gphoto2 is absent
  SMB as a real mount        no — cifs-utils is absent, so nothing in fstab
  NFS as a real mount        yes (mount.nfs)
  this machine serves        nothing (no smbd, no nfs-server)

Removable media
  SD card slot               none — this kernel registered no MMC host
      a USB card reader is not one of these: it arrives over usb-storage
  auto-mount                 udisks2 running
  this session's seat        none
      udisks2 will refuse to mount a card or a USB disk for a session with no
      seat: an ssh login, or anything started by a service. The device is
      present and unmountable, which is a refusal rather than an absence.
  mounted removable media    none
  camera (PTP) tooling       no — gphoto2 is absent

Networking
  links                      4
      wlo1           wifi       connected
      lo             loopback   connected (externally)
      p2p-dev-wlo1   wifi-p2p   disconnected
      enp5s0         ethernet   unavailable
  captive portal             not detected, because nothing checked
      NetworkManager has no connectivity URI here, so it answers 'full' for any
      connected link without asking anyone. A hotel or campus network that has
      addressed you and will answer nothing looks exactly like a working one.
      APEX ships /etc/NetworkManager/conf.d/21-apex-connectivity.conf to turn
      the check on; on this machine it is not in effect.
  VPN protocols              none
      NetworkManager has no built-in VPN beyond WireGuard: each protocol is a
      separate service in /usr/lib/NetworkManager/VPN, so a .ovpn file cannot be imported at all
  WireGuard                  kernel yes, tools no
      NetworkManager has WireGuard built in, but with no wg there is nothing
      to import a provider's .conf with
  Wi-Fi backend              wpa_supplicant
  enterprise Wi-Fi (802.1X)  no saved profile uses it
  hotspot / tethering        1 thing would stop it
      dnsmasq is not installed, so there is no DHCP or DNS server for clients

Bluetooth
  adapters                   1
      hci0
  radio                      not blocked
  paired devices             none
  headset codecs             aac faststream g722 lc3 ldac opus-g opus sbc 
      no aptX: a Qualcomm headset will fall back to SBC

Thunderbolt, USB4 and docks
  hotplug (udev)             systemd-udevd is running
  Thunderbolt                no controller on this machine
  USB-C ports                the kernel exposes no Type-C class
      the ports may still work as plain USB; what is missing is the
      kernel's view of role, power and what is on the other end

timeout 150 bash /tmp/apex-devices-p2005 all  0.12s user 0.12s system 3% cpu 6.747 total
```

---

## 2026-09-13 — the readings retaken in a built image (round: p2-c)

P2-007's evidence said several readers "change their answer once 36232f5's
packages land in a built image". They did. This is the retake.

**The image.** `ghcr.io/andrenijman/apex-os:base-d12d34502f65c4e8704e82e1285ce103a415e4ab`,
14.8 GB, pulled. `d12d3450` is an ancestor of `roadmap/v2.2`, and `95d068c` —
integrate-4's landing of the device work — is an ancestor of `d12d3450`, so the
packages are in it. Checked rather than assumed: the image's
`/usr/libexec/apex-devices` is byte-identical to the tip's
(`ba5b5b4e002beb4db4fb1332fcc5b557fef644365d4170af5aa4fdb474a35e90`).

Two things about the tiers, because both cost time here:

- **The chain is core → base**, not base → core. `Containerfile.base` opens
  `FROM ${CORE}`, so the `base-` image already contains everything the `core-`
  one has. Pulling `core-d12d3450` to look for the device packages finds them
  and finds no `apex-devices`: the APEX tooling is added in the base layer.
- **`36232f5` is not an ancestor of `roadmap/v2.2`.** Integrate-4 landed that
  branch by cherry-pick, so the content is on the tip under different hashes.
  `git branch --contains 36232f5` naming only `task/p2-005-device-maturity` is
  not evidence the work is unlanded; the Containerfiles are.

**Packages, read from the image's own rpmdb** (`rpm -q`, all present):
gvfs 1.58.4, gvfs-fuse, gvfs-mtp, gvfs-smb, gvfs-nfs, gvfs-gphoto2,
samba-client 4.23.12, cifs-utils 7.7, bluez 5.87, bluez-obexd, blueman 2.4.6,
pipewire-codec-aptx 1.4.9, bolt 0.9.11, sane-backends 1.4.0, sane-airscan
0.99.36, simple-scan 49.1, hplip 3.26.4, cups 2.4.19, wireguard-tools
1.0.20260223, NetworkManager-openvpn 1.12.5, NetworkManager-openconnect 1.2.10,
dnsmasq 2.92, libgphoto2 2.5.33, ipp-usb 0.9.34. 1742 packages in total.

`NetworkManager-wireguard` is not a package and never was: WireGuard is built
into NetworkManager, and `wireguard-tools` is what the reader wants.

**Readers that changed.** Left is the reading in the evidence above, taken on
the L16's older image; right is `apex devices all` inside `base-d12d3450`:

| reader | before | after |
|---|---|---|
| SMB / WebDAV / NFS / MTP / cameras (file manager) | missing pieces | `yes` for all five |
| SMB as a real mount | no | `yes (mount.cifs)` |
| NFS as a real mount | no | `yes (mount.nfs)` |
| VPN protocols | `none` | `openconnect openvpn vpnc` |
| WireGuard | `kernel yes, tools no` | `yes (wg, wg-quick)` |
| headset codecs | `aac faststream g722 lc3 ldac opus-g opus sbc` — "no aptX" | `aac aptx faststream …` |
| boltd | not installed | `installed (udev starts it when a device appears)` |
| scanners | hplip's backend hung the enumeration | `none found`, returned promptly, names eSCL/WSD |
| connectivity check enabled | `unknown` — nothing set a uri | `yes`, because `21-apex-connectivity.conf` ships |
| dnsmasq | present, owned by no package (came from an extension) | rpm-owned; the impermanence caveat does not fire |

**What a container cannot move, stated rather than glossed.** There is no D-Bus,
no systemd, no NetworkManager, no bluetoothd, no udevd and no seat in there. So
these readers move from "not installed" to "installed and not running", which is
not the same as working:

- `links` and `connectivity`: `could not reach NetworkManager`.
- `paired devices`: `unavailable — bluetoothd is not running`.
- `hotplug (udev)`: `systemd-udevd is NOT running`, and the reader says every
  reader below it would report an absence and be right to.
- `hotspot / tethering`: `nothing known is in the way` — **because
  `firewall_enforcing` is false with no systemd, not because the mechanism was
  checked.** Do not read that line as a verdict on the hotspot work.
- `sharing a printer`: `could not ask cupsd`.

A reading that moves these needs `apex update` and a reboot on the L16. That is
the boot path, which this unit is not allowed near.

**One reader is right and reads as a gap.** `camera (PTP) tooling  no — gphoto2
is absent` alongside `cameras (file manager) yes`. Both are true: `libgphoto2`
and `gvfs-gphoto2` are installed, so a camera appears in a file manager, and the
`gphoto2` CLI is a separate package that is not. The earlier evidence's
"gvfs-fuse/gphoto2/nfs" meant `gvfs-gphoto2`. Nothing to fix; worth not
re-finding.

**Two defects the built image exposed**, both fixed on `task/p2-c-2`:

1. `apex devices network` stated the firewall's policy as fact in two places —
   that it dropped forwarded traffic, and that its DHCP rule matched replies.
   Both were true when P2-006 measured them and both are false now, so the
   reader reported a machine where a hotspot works as two reasons it cannot.
2. `connectivity` printed nmcli's `Could not create NMClient object` error as
   the state of the network, and then explained that NetworkManager "answers
   '<that error>' for any connected link". `links`, ten lines above, already
   refused to do this; connectivity threw the rc away.

**The live L16 reading with the new reader** (read-only, nothing changed):
`apex-firewall.service` is `inactive` on the machine's current image, so the
hotspot reader correctly adds no blocker and the only note is the dnsmasq-from-
an-extension caveat. The L16 therefore cannot act as a positive control for the
"firewall enforcing, dispatcher missing" branch; the fixture in
`tests/test-apex-devices.sh` covers it. The same run reconfirms both 802.1X
profiles validate nothing.

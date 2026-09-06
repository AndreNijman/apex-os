# P2-005 / P2-006 / P2-007 — device maturity evidence

Branch `task/p2-005-device-maturity` in apex-os, commits `36232f5`, `c823974`,
`f9ab0e7`, `3142bbd`, `5a7d0c6`, `fce0a99`, `5d5441b`.

Three suites, all mutation-proved:

| Suite | Result | Mutations |
|---|---|---|
| `tests/test-device-image.sh` | 41 passed | 40/1 ×5, 39/2, 40/1; restored 41/0 |
| `tests/test-device-services-netns.sh` | 12 passed | 11/1, 9/3, 9/3, 11/1; restored 12/0 |
| `tests/test-apex-devices.sh` | 53 passed | 49/4, 52/1 ×5, 51/2; restored 53/0 |

`cargo test -p apex`: 315 passed. `tests/run-clippy.sh` (container, `rust:1`):
clean.

## What is demonstrated and what is reasoned

There is no printer and no scanner attached to either machine, no dock, no SD
card, no enterprise access point and no VPN endpoint. Nothing below claims one.

| Item | Criterion | Verdict | How |
|---|---|---|---|
| P2-005 | Printing | PARTIAL, demonstrated | The packages are asserted at build. Discovery reaches the machine through the shipped firewall (netns, positive control on 5353); IPP inbound is silent until 631 is in `allowed_tcp`, and `apex devices print` names that at the point a shared printer fails. **No print job has been sent to a printer.** |
| P2-005 | Scanning | DEMONSTRATED | katana finds `airscan:w1:CANON INC. TR4600 series` over WSD in a 6.9 s whole-run. The L16, whose avahi is masked, refuses to enumerate and says why — the same program, two machines, one image. |
| P2-005 | SMB / NFS / WebDAV / MTP / cameras | PARTIAL, demonstrated at the floor | `cifs-utils`, `samba-client`, `gvfs-fuse`, `gvfs-nfs`, `gvfs-gphoto2` were absent from the image and are added. `apex devices share` reads the live state of each on both machines. **No share has been mounted from a server.** |
| P2-005 | SD | REASONED, with one measurement | Neither machine has an MMC host, and both report that as an absence with the usb-storage caveat. The mount path (udisks2 + a seat) is measured in the negative on katana over ssh: no seat, so udisks2 would refuse. **No card has been inserted.** |
| P2-006 | Captive portal | PARTIAL, demonstrated | `21-apex-connectivity.conf` ships, and both machines now report `full` rather than `unknown`. The `portal` branch is fixture-tested. **No hotel network has been joined.** |
| P2-006 | WireGuard / OpenVPN | REASONED | `wireguard-tools` and the three NM VPN plugins were absent and are added; the reader reports both. **No tunnel has been established.** |
| P2-006 | Enterprise Wi-Fi | PARTIAL, demonstrated | Two saved 802.1X profiles on the L16, both EAP PEAP, both with no CA certificate and `system-ca-certs` off, found and named. The plugin, backend and trust-store branches are fixture-tested. **No EAP handshake has been run.** |
| P2-006 | Hotspot | DEMONSTRATED as blocked | Measured in three namespaces: the shipped policy drops forwarded traffic, its DHCP rule matches the reply direction rather than a client's request, and port 53 is shut. Reported to P1-044, not fixed here. |
| P2-007 | Headsets and codecs | PARTIAL, demonstrated | The codec list is read from `/usr/lib64/spa-0.2/bluez5` on both machines: aac, faststream, g722, lc3, ldac, opus, sbc, and no aptX. `pipewire-codec-aptx` is added to the image. **No headset has been connected.** |
| P2-007 | Thunderbolt | DEMONSTRATED as broken, fix reasoned | L16: `domain0`/`domain1` at `security=user`, `iommu_dma_protection=1`, two host routers authorised, and no boltd, so an attached device would sit at `authorized=0` forever. `bolt` is added to the image. **No Thunderbolt device has been attached.** |
| P2-007 | USB-C docks | PARTIAL, demonstrated | L16 has two Type-C ports and one live partner; katana has no Thunderbolt controller at all, which is why the USB-C reader no longer sits behind that early return. **No dock has been attached.** |
| P2-007 | Hotplug | REASONED, with one live reading | udevd is checked before anything is enumerated, and the Type-C partner is the one thing on these machines that appears and disappears with a cable. **No plug event has been watched end to end.** |

## The refusals that a naive reader would have called absences

Four of these fired on real hardware rather than in a fixture:

1. **avahi masked on the L16.** `scanimage -L` does not return; the reader says
   "unavailable — avahi is masked", gives the unmask command, and does not run
   the probe. katana, with avahi running, finds a real scanner.
2. **No seat over ssh on katana.** udisks2 would refuse to mount a card. The
   reader says so instead of reporting an empty slot.
3. **`apex-firewall.service` is not installed on either machine.** Neither run
   blames a firewall for anything, because there is no policy loaded to blame.
4. **dnsmasq on the L16 is owned by no package.** It arrived with an `apex
   install waydroid` system extension, so the hotspot would have worked by
   coincidence and stopped the day that extension went. katana, with no
   extension, reports it plainly absent.

## The two runs

```
### L16 (apex, 21SCCTO1WW) — 2026-09-07T05:01:06+08:00

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
  connectivity               full
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
### katana (Katana GF76 12UG) — 2026-09-07T05:01:07+08:00

Printing
  mDNS (avahi)               running
  cups.socket                enabled
  queues                     none configured
      you add a driverless printer on this network by name; see the mDNS line above
  sharing a printer          off

Scanning
  scanners                   2 found
      device `v4l:/dev/video0' is a Noname HD Webcam: HD Webcam virtual device
      device `airscan:w1:CANON INC. TR4600 series' is a WSD CANON INC. TR4600 series ip=fe80::6e3c:7cff:fea4:d053%3

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
  connectivity               full
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

timeout 150 bash /tmp/apex-devices-p2005 all  0.19s user 0.25s system 6% cpu 6.932 total
```

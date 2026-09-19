# katana — hardware qualification of the roadmap image, 2026-09-19

Unit: `katana-qual`. Machine: MSI Katana GF76 12UG, hostname `apex`, user `andre`.

| | |
|---|---|
| image | `ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2` |
| digest | `sha256:ba263890b69619532b13b9c64e0ac5dcb8eced78bffdc9fd542ccac92607503d` |
| version | `apex (2026-09-18T14:33:05Z)` |
| vendored apex-shell | `b7953e4f` |
| rollback slot | `:gaming-nvidia`, `sha256:308127d9…`, 2026-09-05, `Unlocked: hotfix` |
| kernel | `7.2.5-cachyos1.fc43.x86_64` (`kernel-cachyos`; katana is NOT on the L16's 7.2.3) |
| firmware | `E17L3IMS.110`, 12th Gen Core i7-12700H |
| GPUs | `card1` = i915 (Intel Alder Lake-P Iris Xe) → `eDP-1`; `card2` = nvidia (RTX 3070 Mobile) → `HDMI-A-1` |
| NVIDIA driver | 580.178.04, CUDA 13.0 |

Feeds **P0-001**, **BASE-009**, **P1-038**, **P1-043**, **P2-005/006/007**.

Every command below was run over SSH. Raw logs are on the machine under
`/var/home/andre/qual/`. Where a verdict rests on looking at a screen rather
than on a readable value, the row says **EYES** and says so in the open.

---

## 1. Preconditions — and the thing that contaminated the starting state

### 1.1 The Sep-6 system extension was still merged over the new image

The dispatch note said the `/usr` hotfix overlay that held the hand-repaired
Steam belonged to the old deployment and is gone. That is true of the *hotfix*.
It is **not** true of the system extension, which lives in `/var` and therefore
survived the rebase and merged itself over the new image at boot:

```
$ systemd-sysext status
HIERARCHY EXTENSIONS SINCE
/opt      none       -
/usr      apex-user  Sat 2026-09-19 08:43:05 AWST

$ sudo cat /mnt/apexext/usr/lib/extension-release.d/*   # loop-mounted the raw
APEX_BUILT=2026-09-06T08:54:43Z
```

So before anything was tested, every gaming binary on `PATH` came from an
extension **built by the hand-patched Sep-6 engine on the old image**:

```
$ for b in gamescope steam mangohud gamemoderun; do p=$(command -v $b); echo -n "$b -> $p : "; rpm -qf "$p"; done
gamescope -> /usr/bin/gamescope : file /usr/bin/gamescope is not owned by any package
steam     -> /usr/bin/steam     : file /usr/bin/steam is not owned by any package
mangohud  -> /usr/bin/mangohud  : file /usr/bin/mangohud is not owned by any package
gamemoderun -> /usr/bin/gamemoderun : file /usr/bin/gamemoderun is not owned by any package
```

**Consequence for this report:** `apex gaming` reporting `gamescope: yes /
steam: yes` on a fresh boot of the roadmap image is not evidence about the
image. Everything in §3 and §4 was therefore run only **after** the extension
was rebuilt by the image's own engine (§2).

The Sep-6 raw and pkg state were copied aside first, and are still there:

```
/var/lib/apex/qual-backup-20260919/apex-user.raw.sep06   sha256 85e1739b576891215fb302c2716d1f890c64f40a9bd2f93fa12c32103672d2f6
/var/lib/apex/qual-backup-20260919/state.json.sep06
/var/lib/apex/qual-backup-20260919/requested.sep06
```

`~/apex-pre-rebase-20260919/` was not touched.

### 1.2 The `requested` set was not virgin

```
$ sudo cat /var/lib/apex/pkg/requested
gamemode
gamescope
libSM.i686
libgcc.i686
mangohud
steam
steam-devices
```

`libSM.i686` and `libgcc.i686` were added by hand on 2026-09-06 because RPM
Fusion's `steam` never declares them. They are still in the set, so §2 is **not**
a from-scratch test of `apex install steam` — it is a test of the engine with
two extra packages already requested. Stated rather than buried; a genuinely
virgin run would need the state directory cleared, which would have left the
machine with no Steam if the engine had failed.

### 1.3 Disk

```
$ df -h /var /
/dev/nvme1n1p3  954G  889G   61G  94% /var
composefs        40M   40M     0 100% /
```

61 G free. Enough; worth knowing before a 337 MB rebuild that extracts 219 rpms.

---

## 2. `sudo apex install steam gamescope mangohud gamemode steam-devices`
### — the first run of this path from a built image

**Verdict: PASS.** Exit 0, 54 seconds, 219 packages, 337 MB extension, merged.

Run under `systemd-run` so an SSH drop could not truncate it:

```
$ sudo systemd-run --unit=qual-install --collect \
    --property=StandardOutput=append:/var/tmp/qual-install.log \
    --property=StandardError=append:/var/tmp/qual-install.log \
    /usr/bin/apex install steam gamescope mangohud gamemode steam-devices

$ journalctl -u qual-install
Sep 19 08:56:08 apex systemd[1]: Started qual-install.service …
Sep 19 08:57:02 apex systemd[1]: qual-install.service: Deactivated successfully.
Sep 19 08:57:02 apex systemd[1]: qual-install.service: Consumed 1min 2.612s CPU time, 2.5G memory peak.
```

Tail of the log:

```
apex-pkg: warning: kept your edited /etc/group;   new version saved as /etc/group.apexnew
apex-pkg: warning: kept your edited /etc/passwd;  new version saved as /etc/passwd.apexnew
apex-pkg: warning: kept your edited /etc/shadow;  new version saved as /etc/shadow.apexnew
apex-pkg: activating
Using extensions 'apex-user.raw'.
Unmerged '/usr'.
Merged extensions into '/usr'.
apex-pkg: done — 219 package(s), 337MB extension
```

### 2.1 The app-twin EVR rule fired, from an image, for the first time

This is the rule katana's Sep-6 session wrote by hand into `/var/tmp/apexpkg-claude2`
and that `fix/pkg-multilib-2` folded in. It has never run from a built image
until now:

```
apex-pkg: warning: dropping 'steam-1.0.0.85-1.fc43.i686': a 32-bit copy of the
  application (the set has steam-1.0.0.87-1.fc43.x86_64); a 32-bit package is
  libraries, not a second copy of the program
apex-pkg: warning: allowing 'glibc-2.42-16.fc43.i686': multilib sibling of a
  core package; the image ships none of this architecture
apex-pkg: warning: allowing 'systemd-libs-258.10-1.fc43.i686': multilib sibling
  of a core package; the image ships none of this architecture
apex-pkg: 219 package(s) verified against the repository keys
```

**PASS** — the exact EVR pair that killed the install on 2026-09-06
(`steam .85 i686` vs `steam .87 x86_64`) is now dropped with a correct reason,
and `glibc.i686`/`systemd-libs.i686` are allowed through the `REFUSE_RE` core
guard rather than aborting the run. Both halves of the branch work from an image.

### 2.2 The `/etc` removal pass did not eat image-owned files

The Sep-6 defect deleted 26 image-owned `/etc` files. **It did not recur.**

```
$ sudo diff -rq /usr/etc /etc | grep "^Only in /usr/etc"
Only in /usr/etc/fonts/conf.d: 25-unhint-nonlatin.conf
Only in /usr/etc/krb5.conf.d: crypto-policies
Only in /usr/etc/pki/tls: fips_local.cnf
```

Three, not zero — so the honest reading takes one more step. The removal pass
can only delete paths it recorded in `etc.list`, and **none of these three is in
it**:

```
$ sudo cat /var/lib/apex/pkg/etc.list
group / group- / gshadow / gshadow- / passwd / shadow
profile.d/steam.csh / profile.d/steam.sh / .pwd.lock
security/limits.d/10-gamemode.conf
$ for p in fonts/conf.d/25-unhint-nonlatin.conf krb5.conf.d/crypto-policies pki/tls/fips_local.cnf; do
    sudo grep -c -x "$p" /var/lib/apex/pkg/etc.list; done
0
0
0
```

All three are `crypto-policies`/fontconfig symlinks and are residue of the
**Sep-6** damage-and-restore, which restored 26 files but not these. Today's
install could not have removed them. `etc.list` shrank to the nine paths above,
all of which are genuinely extension content (`steam` profile scripts, the
gamemode limits drop-in) or account files the engine correctly refused to
overwrite.

**PASS** for the removal pass. The six `.apexnew` files are the *correct*
behaviour — the engine refused to overwrite `/etc/passwd`, `/etc/group`,
`/etc/shadow` and their backups because the live copies differ from the package
defaults:

```
$ sudo find /etc -name "*.apexnew"
/etc/group.apexnew  /etc/group-.apexnew  /etc/gshadow.apexnew
/etc/gshadow-.apexnew  /etc/passwd.apexnew  /etc/shadow.apexnew
```

**Open, minor:** three `/etc` image defaults are still missing on this machine
from September 6 and nothing restores them. Worth a `restorecon`-style sweep
before this machine is used as a clean baseline again.

---

## 3. NEW DEFECT — the multilib guard stops at `/usr/bin`, and GStreamer is dead because of it

**Verdict: FAIL. Reproduced from a built image, with a positive control.**

This is the `fc-list` shadowing defect exactly, one directory over, and the
image's own engine reproduces it byte-for-byte.

### 3.1 The measurement

Loop-mount the extension, list its files, and intersect with every path the
image's rpmdb claims to own:

```
$ sudo mount -o ro,loop /var/lib/extensions/apex-user.raw /mnt/apexext
$ sudo find /mnt/apexext/usr -mindepth 1 \( -type f -o -type l \) -printf "/usr/%P\n" | sort > /tmp/ext.txt
$ rpm -qal | sort -u > /tmp/image.txt
$ comm -12 /tmp/ext.txt /tmp/image.txt | wc -l
177
```

Identical before (Sep-6 engine) and after (image engine): **3447 extension
files, 177 of which shadow an image-owned path.** 14 of those are 32-bit ELF
binaries sitting on top of the image's 64-bit ones:

```
32-BIT SHADOW: /usr/libexec/at-spi-bus-launcher                    (image owner: at-spi2-core-2.58.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/at-spi2-registryd                      (image owner: at-spi2-core-2.58.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/dconf-service                          (image owner: dconf-0.49.0-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/gio-launch-desktop                     (image owner: glib2-2.86.5-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/glib-pacrunner                         (image owner: glib-networking-2.80.1-3.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/glycin-loaders/2+/glycin-heif           (image owner: glycin-loaders-2.0.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/glycin-loaders/2+/glycin-image-rs       (image owner: glycin-loaders-2.0.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/glycin-loaders/2+/glycin-jxl            (image owner: glycin-loaders-2.0.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/glycin-loaders/2+/glycin-svg            (image owner: glycin-loaders-2.0.8-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/gstreamer-1.0/gst-completion-helper     (image owner: gstreamer1-1.26.11-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/gstreamer-1.0/gst-hotdoc-plugins-scanner(image owner: gstreamer1-1.26.11-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/gstreamer-1.0/gst-plugin-scanner        (image owner: gstreamer1-1.26.11-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/gstreamer-1.0/gst-ptp-helper            (image owner: gstreamer1-1.26.11-1.fc43.x86_64)
32-BIT SHADOW: /usr/libexec/p11-kit/p11-kit-remote                  (image owner: p11-kit-0.26.5-1.fc43.x86_64)
```

`/usr/bin` and `/usr/sbin` are clean — the August fix works where it was
pointed:

```
/usr/bin/fc-list    ELF 64-bit LSB pie executable, x86-64
/usr/bin/ldconfig   ELF 64-bit LSB pie executable, x86-64
/usr/bin/pipewire   ELF 64-bit LSB pie executable, x86-64
/usr/bin/dconf      ELF 64-bit LSB pie executable, x86-64
/usr/bin/gio        ELF 64-bit LSB pie executable, x86-64
/usr/bin/gsettings  ELF 64-bit LSB pie executable, x86-64
$ fc-list | wc -l    → 538
$ fc-match sans      → NotoSans-Regular.ttf: "Noto Sans" "Regular"
```

### 3.2 The cause, in one line of the engine

`files/system/libexec/apex-pkg`, `extract_rpms`, the second (32-bit) pass:

```bash
rpm --root "$root" -Uvh --nodeps --noscripts --notriggers --noplugins \
    --nosignature --replacefiles --replacepkgs --excludepath /boot \
    --excludepath /usr/bin --excludepath /usr/sbin \
    --excludepath /usr/share --excludepath /etc \
    "${foreign[@]}"
```

The comment above it says "excluding these four leaves exactly the libraries
behind." **It does not.** i686 packages also ship 32-bit *helper executables*
under `/usr/libexec`, and nothing excludes that directory. `at-spi2-core.i686`,
`dconf.i686`, `glib2.i686`, `glib-networking.i686`, `glycin-loaders.i686`,
`gstreamer1.i686` and `p11-kit.i686` are all in the dependency closure of
`steam.i686`, so a plain `apex install steam` is enough to trigger it.

(18 more shadows are text under `/usr/lib` — i686 copies of
`systemd/user/pipewire.service`, `udev/rules.d/*`, `tmpfiles.d/*`,
`sysusers.d/pipewire.conf`, the canberra units. Those are byte-identical to the
image's and are not causing a failure today, but they are the same class and
should be excluded for the same reason.)

### 3.3 What it breaks, measured — GStreamer has 2 usable features instead of 1344

`gst-plugin-scanner` is the out-of-process helper GStreamer uses to probe every
plugin. The shadowed one is i686, so it cannot load a single 64-bit plugin:

```
$ rm -rf ~/.cache/gstreamer-1.0; gst-inspect-1.0 2>&1 | tail -3
(gst-plugin-scanner:9196): GStreamer-WARNING **: Failed to load plugin
  '/usr/lib64/gstreamer-1.0/libgsty4menc.so': wrong ELF class: ELFCLASS64
Total count: 240 plugins (239 blacklist entries not shown), 2 features

$ gst-inspect-1.0 playbin
No such element or plugin 'playbin'
```

**Positive control — the image's own pristine 64-bit scanner, same machine,
same plugins, same moment:**

```
$ PRIST=/sysroot/ostree/deploy/default/deploy/5bd2bf3f…0/usr/libexec/gstreamer-1.0/gst-plugin-scanner
$ file -b $PRIST → ELF 64-bit LSB pie executable, x86-64
$ rm -rf ~/.cache/gstreamer-1.0; GST_PLUGIN_SCANNER=$PRIST gst-inspect-1.0 | tail -1
Total count: 240 plugins, 1344 features
$ GST_PLUGIN_SCANNER=$PRIST gst-inspect-1.0 playbin | head -4
Factory Details:
  Rank                     none (0)
  Long-name                Player Bin 2
  Klass                    Generic/Bin/Player
```

240 → 240 plugins, 2 → 1344 features, `playbin` absent → present, with nothing
changed but which scanner binary ran. The shadow is the cause and nothing else
is.

**Blast radius:** every GStreamer consumer on a machine that has ever run
`apex install steam`. That is sound events, video thumbnailing, and the
media paths several of the P1-038 applications use.

### 3.4 Also live right now: the greeter runs the 32-bit accessibility bus

```
$ sudo file /proc/1707/exe /proc/1723/exe
/proc/1707/exe: symbolic link to /usr/libexec/at-spi-bus-launcher
/proc/1723/exe: symbolic link to /usr/libexec/at-spi2-registryd
$ file -b /usr/libexec/at-spi-bus-launcher
ELF 32-bit LSB pie executable, Intel i386
```

Those are the greeter's own processes (pids under `greetd`). The APEX greeter
ships the accessibility bus deliberately so the login screen can be heard by a
screen reader; it is running the i686 build of it. Not observed to fail today —
recorded because it is the same shadow reaching a load-bearing path.

### 3.5 The fix

Add `/usr/libexec` to the second pass's `--excludepath` list, and `/usr/lib/systemd`,
`/usr/lib/udev`, `/usr/lib/tmpfiles.d`, `/usr/lib/sysusers.d` with it. A
regression test that does not need a compositor: build the extension for a set
containing `gstreamer1.i686`, then assert no file in the extension is both
32-bit ELF and `rpm -qf`-owned by an image package. The measurement in §3.1 is
that test, and it is four commands.

**Not fixed on the machine.** Patching `apex-pkg` live would be a repeat of the
Sep-6 hotfix that this whole exercise exists to stop needing, and it would
invalidate every later row in this report. The defect is left in place and
reproducible.

---

## 4. How the sessions were logged in — and why not through greetd

greetd's own path could not be driven: `apex-session-select` deliberately does
**not** arm autologin ("Switching modes costs one password entry at the greeter"),
and this unit does not have Andre's password. Editing `/etc/greetd/config.toml`
to add an `initial_session` would have changed the login path the report is
meant to qualify.

**What was used instead — a PAM `login` session on VT 2**, which is what greetd
itself creates, and which logind therefore puts on `seat0` and grants DRM master
to. `/var/home/andre/qual/start-session.sh`:

```bash
exec_line="$(sed -n 's/^Exec=//p' /usr/share/wayland-sessions/${id}.desktop | head -1)"
sudo systemd-run --unit="qual-sess-${id}" --collect \
  --property=User=andre --property=Group=andre \
  --property=PAMName=login --property=TTYPath=/dev/tty2 \
  --property=StandardInput=tty \
  --setenv=XDG_SEAT=seat0 --setenv=XDG_VTNR=2 \
  --setenv=XDG_SESSION_DESKTOP="$names" --setenv=XDG_CURRENT_DESKTOP="$names" \
  /bin/sh -lc "$exec_line"
sudo chvt 2
```

`sh -lc "<Exec>"` is exactly how `apex-greet`'s `launch()` runs the session
(`GreetContext.qml:221`), and the Exec line is read from the shipped `.desktop`
rather than retyped.

**Fidelity check — logind classifies the result the same way it classifies the
greeter's own session:**

```
$ loginctl show-session c1 -p Type -p Class -p Seat   # the greeter
Seat=seat0   Type=wayland   Class=greeter
$ loginctl show-session 97 -p Type -p Class -p TTY    # a session started this way
Type=wayland  Class=user  TTY=tty2
```

Known divergences, stated: greetd would run this on VT 1 after tearing the
greeter down, and `sh -lc` under greetd inherits greetd's environment rather
than systemd-run's. One measurable consequence appeared: under the labwc
session, applications saw `XDG_SESSION_TYPE=tty` in their environment even
though logind recorded `Type=wayland`, because `pam_systemd`'s PAM environment
lands after `--setenv`. Under niri it read `wayland`, because `niri --session`
re-exports it itself. Treat the `XDG_SESSION_TYPE` **environment variable** rows
below as a property of this harness, not of the image; the logind `Type` rows
are the real ones.

The greeter (`sway` + `qs` on tty1) stayed alive throughout; every session ran
on VT 2 and was torn down with `systemctl stop qual-sess-<id>`.

---

## 5. The five sessions

Per-session readout script: `/var/home/andre/qual/measure.sh` (re-runnable).
Render proof is `grim` per output plus a distinct-colour/mean-RGB count, so
"it painted" is a number rather than an impression.

| session | starts | eDP-1 (Intel) | HDMI-A-1 (NVIDIA) | DRM nodes held | nvidia-smi | APEX Shell | XWayland | verdict |
|---|---|---|---|---|---|---|---|---|
| APEX Floating (`apex-labwc`) | yes | 1920x1080@144.028 | **1920x1080@239.964** | card1, card2, renderD128, renderD129 | `labwc` listed as `G` | yes | yes | **PASS** |
| APEX Tiling (`hyprland`) | yes | 1920x1080@144.028 | **1920x1080@239.964** | card1, card2, renderD128, renderD129 | `Hyprland` listed as `G` | yes | yes | **PASS** |
| APEX Scrolling (`niri`) | yes | 1920x1080@144.028 | **1920x1080@239.964** | card1, card2, renderD129 | `niri` listed as `G` | yes | on demand | **PASS**, with a defect (§5.4) |
| APEX Safe Graphics | yes | 1920x1080@144.028 | **NOT LIT** | card1, card2 | absent (software renderer) | n/a by design | no | **PARTIAL FAIL** (§5.5) |
| APEX Gaming Mode | starts, then exits | lit, then released | **NOT LIT** | card1 only | — | n/a by design | yes (gamescope's own) | **FAIL** (§6) |

Render proof, both outputs, each session:

```
labwc   eDP-1 1920x1080 distinct_colours=216008 mean_rgb=(156.3,165.0,178.3)
        HDMI-A-1 1920x1080 distinct_colours=216008 mean_rgb=(156.3,165.0,178.3)
hyprland eDP-1 1920x1080 distinct_colours=216021 mean_rgb=(156.1,164.8,178.1)
        HDMI-A-1 1920x1080 distinct_colours=216021 mean_rgb=(156.1,164.8,178.2)
niri    eDP-1 1920x1080 distinct_colours=165218 mean_rgb=(131.8,140.5,151.6)
        HDMI-A-1 1920x1080 distinct_colours=165218 mean_rgb=(131.8,140.5,151.6)
safegfx eDP-1 1920x1080 distinct_colours=709    mean_rgb=(7.7,7.7,7.7)
        HDMI-A-1 grim FAILED rc=1 unknown output 'HDMI-A-1'
```

The two per-output captures being byte-identical in the desktop sessions is not
a `grim` bug — it is the same wallpaper and the same per-output bar on both
screens. Settled by giving HDMI a different mode and re-capturing:

```
$ wlr-randr --output HDMI-A-1 --mode 1280x720@59.943
$ grim /tmp/w5.png          → 3200x1080      (one extended desktop, 1920+1280)
$ grim -o eDP-1 /tmp/e5.png → 1920x1080
$ grim -o HDMI-A-1 …        → 1280x720
```

### 5.1 The external monitor is driven by the discrete GPU in all three desktop sessions

`nvidia-smi` lists the compositor itself as a graphics client, which is the
assertion that matters for P1-043:

```
| Processes:                                                              |
|    0   N/A  N/A   1637   G   sway        2MiB |   <- the greeter, on tty1
|    0   N/A  N/A   9481   G   labwc       2MiB |
|    0   N/A  N/A  20342   G   Hyprland    3MiB |
|    0   N/A  N/A  21604   G   niri        3MiB |
```

and the kernel agrees, with no compositor in the loop:

```
card1-eDP-1      status=connected  enabled=enabled  dpms=On  mode=1920x1080
card2-HDMI-A-1   status=connected  enabled=enabled  dpms=On  mode=1920x1080
```

(`card2-HDMI-A-1` does expose `dpms`; an earlier truncated `ls` in this session
suggested otherwise and was wrong — checked by reading the attribute directly.)

**PASS** — P1-043's "a hybrid laptop reporting the wrong card" does not happen
in the desktop sessions. It happens in Gaming Mode; see §6.

### 5.2 APEX Shell responds in every desktop session

```
$ apex shell launcher|dashboard|notifications|clipboard|power|menu   → rc=0 (labwc)
$ apex shell launcher|dashboard|power                                → rc=0 (hyprland)
$ apex shell launcher|dashboard|power                                → rc=0 (niri)
```

Hyprland's own IPC and the screenshot path both work:

```
$ HYPRLAND_INSTANCE_SIGNATURE=… hyprctl monitors
Monitor eDP-1 (ID 0):    1920x1080@144.02800 at 0x0   scale: 1  transform: 0  focused: yes  dpmsStatus: 1  vrr: false
Monitor HDMI-A-1 (ID 1): 1920x1080@239.96400 at 1920x0 scale: 1 transform: 0  focused: no   dpmsStatus: 1  vrr: false
$ grimblast save output ~/qual/shots/hypr-grimblast.png
grim -t png -o eDP-1 /var/home/andre/qual/shots/hypr-grimblast.png   → rc=0, PNG 1920x1080
```

### 5.3 The session auto-locks, and the lock is compositor-enforced

The labwc session locked itself 5½ minutes into the run. That is
`~/.config/hypr/hypridle.conf` doing its job, and its timings are worth having
on record because they are why unattended work on this machine dies:

```
300s  brightnessctl -s set 10%      (dim)
330s  loginctl lock-session          (LOCK)
360s  DpmsControl.sh off             (displays off)
900s  loginctl suspend               (SUSPEND)
```

**The lock holds when its client is killed** — tested, because a lock screen
that a shell command can dismiss is not a lock:

```
$ grim -o eDP-1 lock-1-locked.png     → 1920x1080 colours=23859 mean=(59.9,63.9,71.1)  (the APEX lock screen)
$ pkill -x quickshell ; sleep 5
$ pgrep -c -x quickshell              → 0
$ grim -o eDP-1 lock-2-after-kill.png → 1920x1080 colours=1     mean=(0.0,0.0,0.0)     (black)
$ pgrep -c -x labwc                   → 1   (the compositor is still running)
```

**PASS.** labwc keeps the `ext-session-lock-v1` surface in its abandoned state
and paints black; the desktop is never revealed. 1 970 435 of 2 073 600 pixels
changed between the two captures — the lock screen was replaced by black, not by
the desktop.

**Finding, minor: `grim` hangs forever on a DPMS-off output.** Two `grim`
processes sat in `poll_schedule_timeout` for 283 s and 150 s while
`card1-eDP-1` read `enabled=disabled dpms=Off`. `wlr-screencopy` has no timeout
and neither does `grim`, so the APEX screenshot keybind pressed as the display
blanks will hang rather than fail. Every `grim` in this report is wrapped in
`timeout` because of it.

**Finding, minor: `apex shell` exposes no Caffeine target.** `apex shell list`
has 19 targets; none of them is caffeine or idle-inhibit. Whatever the shell's
Caffeine toggle is bound to, the CLI cannot reach it, so the "keep this machine
awake" affordance has no scriptable form. hypridle had to be killed outright to
finish this run — recorded as a test-harness change; it is session-local and
died with the session.

### 5.4 DEFECT — the niri session runs two bars

```
$ pgrep -a -u andre waybar
21723 waybar
$ pgrep -a -u andre quickshell
21730 quickshell -c /usr/share/apex-shell
```

Both are running. The cause is that **APEX ships no niri configuration at all**:

```
$ find /usr/share /usr/etc -name config.kdl 2>/dev/null
        (nothing — readable directories, genuinely absent)
$ ls -la ~/.config/niri/
-rw-r--r--. 1 andre andre 28144 Sep  6 09:07 config.kdl
-rw-r--r--. 1 andre andre 27800 Jul 30 19:16 config.kdl.pre-include.bak
```

`~/.config/niri/config.kdl` is **niri's own upstream default**, written by niri
on first run, and line 271 of it is upstream's stock example:

```
// This line starts waybar, a commonly used bar for Wayland compositors.
spawn-at-startup "waybar"
```

`apex-shell-firstrun` only appends two `include` lines
(`ApexShellInput.kdl`, `ApexShellKeybinds.kdl`) to whatever is already there —
it never removes the stock waybar spawn. So every niri user who lets niri write
its own default gets waybar **and** the APEX bar. Contrast Hyprland and labwc,
both of which APEX seeds from `/usr/share/apex/hypr` and `/usr/share/apex/labwc`.

### 5.5 DEFECT — APEX Safe Graphics cannot light a display on the discrete GPU

This is the recovery session, so the failure mode matters more than most.

```
$ /usr/libexec/apex-safe-graphics check
config      /usr/share/apex/safe-graphics
compositor  /usr/bin/labwc
terminal    foot
renderer    pixman (software)
```

It starts, and the internal panel works — `foot` is up and the desktop paints
(709 distinct colours, mean 7.7 — a dark recovery desktop). **The external
monitor is never enabled:**

```
$ wlr-randr
HDMI-A-1 "Lenovo Group Limited R25f-30 …"   Enabled: no
eDP-1    "AU Optronics 0x978F"              Enabled: yes  1920x1080@144.028
$ cat /sys/class/drm/card2-HDMI-A-1/{enabled,dpms}
disabled
Off
$ grim -o HDMI-A-1 …
grim: unknown output 'HDMI-A-1'
```

It cannot be turned on by hand either — `wlr-randr --output HDMI-A-1 --on`
returns and the output stays `Enabled: no`.

**Cause, from the session's own log:**

```
[ERROR] [backend/drm/renderer.c:23] Renderer did not support importing DMA-BUFs
[ERROR] [types/output/swapchain.c:109] Swapchain for output 'HDMI-A-1' failed test   (x60)
```

Zero such errors for `eDP-1`. The pixman software renderer the session forces
(`WLR_RENDERER=pixman`, `LIBGL_ALWAYS_SOFTWARE=1`) cannot import DMA-BUFs, and
wlroots' multi-GPU path needs exactly that to feed a secondary GPU's connector.

**Why it matters:** Safe Graphics exists for "the normal desktop will not
start". A machine whose panel is dead, a laptop in a dock with the lid shut, or
any hybrid machine whose only working screen hangs off the discrete GPU gets a
recovery session that shows nothing at all — and its own comment says the other
route in is "Ctrl+Alt+F2, log in, run `apex-safe-graphics`", which lands on the
same blank screen. The session script should either allow hardware rendering on
the secondary GPU as a fallback, or say out loud which outputs it could not
light.

---

## 6. Gaming Mode — FAIL, for two independent reasons, both fixable

`apex gaming` says it is ready, and after the engine rebuild it is honest about
the machine rather than about the stale extension:

```
$ apex gaming
ready           : yes
greeter entry   : yes    session script : yes
gamescope       : yes    steam          : yes    mangoapp : yes
realtime limit  : yes
switch helper   : yes
sudoers rule    : not measured — could not read the path: Permission denied (os error 13)
gamepads        : none attached
```

**That last row is a refusal, not an absence** — re-asked with privilege:

```
$ sudo apex gaming
sudoers rule    : yes
```

Worth fixing in the tool: an unprivileged read that cannot see
`/etc/sudoers.d` reports the same string whether the rule is missing or
unreadable, and the two are different answers.

### 6.1 DEFECT — Gaming Mode runs on the Intel iGPU and the laptop panel, never the monitor

This is the substance of "the gaming modes should properly boot and on the
monitor". It does not. From the session's own log, first attempt, unmodified:

```
[apex-gaming-session] starting: gamescope -e -f --expose-wayland --rt --mangoapp -- steam -gamepadui
[gamescope] vulkan: Intel device detected, forcing general queue family instead of compute-only queue
[gamescope] vulkan: selecting physical device 'Intel(R) Iris(R) Xe Graphics (ADL GT2)': queue family 0
[gamescope] drm:    opening DRM node '/dev/dri/card1'
[gamescope] drm:    Connectors:
[gamescope] drm:      eDP-1 (connected)
[gamescope] drm:    selecting connector eDP-1
[gamescope] drm:    selecting mode 1920x1080@144Hz
[gamescope] Error drm: drmModeAddFB2WithModifiers failed: Invalid argument
```

On a machine with an RTX 3070 and the only external monitor wired to it,
gamescope picks the **Intel Iris Xe**, opens **card1**, and never sees
`HDMI-A-1` at all — it is on card2. `apex-gaming-session` passes no device
preference, so gamescope takes its default, which is the first DRM node.

**The fix is one flag, and it was measured.** Third attempt, identical session
script, with `APEX_GAMESCOPE_ARGS="--prefer-output HDMI-A-1 --prefer-vk-device 10de:249d"`
(`10de:249d` from `lspci -nn`: `NVIDIA Corporation GA104M [GeForce RTX 3070 Mobile]`):

```
[gamescope] vulkan: selecting physical device 'NVIDIA GeForce RTX 3070 Laptop GPU': queue family 2
[gamescope] drm:    opening DRM node '/dev/dri/card2'
[gamescope] drm:    Connectors:
[gamescope] drm:      HDMI-A-1 (connected)
[gamescope] drm:    selecting connector HDMI-A-1
[gamescope] drm:    selecting mode 1920x1080@240Hz
```

Right GPU, right node, right connector, and the monitor's full 240 Hz. Note
`--prefer-vk-device` is what moves the **DRM node** too — `WLR_DRM_DEVICES=/dev/dri/card2`
was tried first (second attempt) and gamescope ignored it completely, still
opening card1, because gamescope's DRM backend is its own and not wlroots'.

So `apex-gaming-session` needs to choose a device rather than accept the
default. It already probes `/sys/class/drm/card*-*/vrr_capable`; the same kind
of probe can find which card owns a connected connector and pass
`--prefer-vk-device` for that card's PCI id.

### 6.2 DEFECT — `--rt` is requested and never granted

```
[apex-gaming-session] soft RLIMIT_RTPRIO is 20 — requesting realtime scheduling
[gamescope] No CAP_SYS_NICE, falling back to regular-priority compute and threads.
             Performance will be affected.
```

The session script's `ulimit -Sr` check is careful and correct as far as it
goes, and `30-apex-gaming-rtprio.conf` did its job — the soft limit is 20. But
gamescope wants **CAP_SYS_NICE**, not just `RLIMIT_RTPRIO`, for its compositing
and its own threads, and nothing grants it. The log line the script prints
("requesting realtime scheduling") is therefore true and the outcome is not.
This happened on every one of the three attempts.

### 6.3 Steam never reaches a UI inside gamescope — two causes observed

`gamescope exited with 0` on attempt one and **139** (SIGSEGV) on attempt three,
both preceded by `launch: Primary child shut down!` — Steam died first each
time. Steam's own log (`~/.local/share/Steam/logs/console-linux.txt`) carries
the reasons; they were not in the session log because Steam redirects its
output through `srt-logger`:

```
[09:21:26] steam-runtime-check-requirements: W: Child process exited with code 1:
             bwrap: Unexpected capabilities but not setuid, old file caps config?
[09:21:26] steam.sh: Error: Steam now requires user namespaces to be enabled.
...
[09:23:37] Unable to open X11 display, exiting
```

The user-namespace message is **not** a machine misconfiguration — checked
rather than assumed:

```
$ sysctl kernel.unprivileged_userns_clone     → 1
$ cat /proc/sys/user/max_user_namespaces      → 254518
$ unshare --user --map-root-user echo ok      → ok
$ getcap /usr/bin/bwrap                       → (no capabilities, correct for a setuid-free bwrap)
$ flatpak run --command=/bin/true org.blender.Blender   → rc=0
```

Namespaces work and flatpak's bwrap works. The failing bwrap is the one in
Steam's own runtime, and it complains about **unexpected capabilities in its
permitted set** — which points at the environment the session is started in
rather than at the kernel. Recorded as observed; not root-caused further,
because §6.1 has to be fixed first and may change this.

### 6.4 Steam itself is fine — PASS in a desktop session, on the monitor

To separate "Steam is broken" from "Gaming Mode is broken", Steam was run in
the labwc session with `DISPLAY=:0`:

```
$ pgrep -a -u andre -x Xwayland
25681 Xwayland :0 -rootless -core -terminate 10 …
$ DISPLAY=:0 setsid /usr/bin/steam &
$ ps -u andre -o pid,comm | grep -i steam
26638 steam
26860 steam-runtime-l
26916 steamwebhelper   (+5 more)
$ tail ~/.local/share/Steam/logs/console-linux.txt
Desktop state changed: desktop: { pos: 0,0 size: 3840,1080 } primary: { pos: 0,0 size: 1920,1080 }
Fossilize INFO: Setting autogroup scheduling.
```

Render proof, and this is the row that answers "does the client render on the
monitor":

```
$ python3 ~/qual/shot.py steam-running
eDP-1    1920x1080 distinct_colours=216174 mean_rgb=(156.3,165.0,178.3)   (unchanged wallpaper)
HDMI-A-1 1920x1080 distinct_colours=321700 mean_rgb=(121.4,118.9,117.8)   (the Steam client)
```

**EYES, stated as such:** the HDMI capture was also looked at. It is the full
Steam client — store page loaded, signed in, the library/community chrome
correct, text rendering clean with no missing-font boxes (the Sep-6 fontconfig
symptom does not recur), and the APEX Shell bar along the top. That part is an
observation, not an assertion; the numbers above are the assertion.

**PASS** for `apex install steam` → a working Steam client on the
NVIDIA-connected monitor. **The `-gamepadui`/gamescope path is the part that fails.**

### 6.5 DEFECT — `apex install steam` produces a Steam with no 32-bit Vulkan at all

Steam's log, on the successful desktop run:

```
CVulkanTopology: failed create vulkan instance: -9        (VK_ERROR_INCOMPATIBLE_DRIVER)
CVulkanTopology: failed to create vulkan instance — Failed to query vulkan gpu topology
Vulkan missing requested extension 'VK_KHR_surface'.
Vulkan missing requested extension 'VK_KHR_xlib_surface'.
BInit - Unable to initialize Vulkan!
```

Steam's client binary is `ubuntu12_32/steam` — 32-bit — so it needs 32-bit
Vulkan. The 32-bit loader and the 32-bit drivers are both installed:

```
$ ls /usr/lib/libvulkan.so.1 /usr/lib/libGLX_nvidia.so.0 /usr/lib/libvulkan_intel.so
/usr/lib/libvulkan.so.1  /usr/lib/libGLX_nvidia.so.0  /usr/lib/libvulkan_intel.so
$ sudo grep -o 'xorg-x11-drv-nvidia-libs[^"]*' /var/lib/apex/pkg/state.json | sort -u
xorg-x11-drv-nvidia-libs-3:580.178.04-1.fc43.i686
```

**The manifests that connect them are not:**

```
$ ls /usr/share/vulkan/icd.d/ | grep -c i686
0
$ ls /usr/share/vulkan/icd.d/
asahi_icd.x86_64.json … nvidia_icd.x86_64.json … virtio_icd.x86_64.json   (13, all x86_64)
```

And the rpms in the set do ship them:

```
$ dnf5 repoquery -l xorg-x11-drv-nvidia-libs-3:580.178.04-1.fc43.i686 | grep vulkan/icd
/usr/share/vulkan/icd.d/nvidia_icd.i686.json
$ dnf5 repoquery -l mesa-vulkan-drivers-25.3.6-3.fc43.i686 | grep vulkan/icd
/usr/share/vulkan/icd.d/asahi_icd.i686.json … (12 more)
$ ls /mnt/apexext/usr/share/vulkan/icd.d/          # the built extension
ls: cannot access '/mnt/apexext/usr/share/vulkan/icd.d/': No such file or directory
```

**Cause — the same line of `apex-pkg` as §3, failing the other way.** The
32-bit pass carries `--excludepath /usr/share`, so every `*_icd.i686.json` is
thrown away while the `.so` files they name are kept. A 32-bit Vulkan
application on an APEX machine therefore sees **zero** ICDs and falls back to
nothing.

This is why the 2026-09-17 live recovery had to hand-write
`/usr/share/vulkan/icd.d/nvidia_icd.i686.json`. The image does not ship it —
it is `.i686`-suffixed and comes only from the i686 rpm — and `apex install`
deletes it.

**Fix:** the `/usr/share` exclusion is too blunt. The reason it exists is that
i686 packages duplicate `/usr/share/doc`, `/usr/share/man`, icons and locale,
which would shadow the image's. Arch-suffixed files like `*_icd.i686.json`,
`*.i686.json` implicit layers and `/usr/share/glvnd/egl_vendor.d/*.i686.json`
cannot shadow anything — nothing else has that name. Either keep paths whose
basename carries a non-host arch token, or replace the blanket
`--excludepath /usr/share` with the `rpm -qf` image-owner test §3 wants,
which handles both defects with one rule.

### 6.6 Gaming Mode cleanup does work

```
$ apex game status          # after every attempt
active    : false
$ pgrep -c -u andre -x gamescope   → 0
$ systemctl is-active qual-sess-apex-gaming → inactive
```

The `trap cleanup EXIT HUP INT TERM` released `apex game` every time, including
after the SIGSEGV exit, and `[apex-gaming-session] apexd game mode released` is
in the log. The fail-safe design holds: a broken Gaming Mode exits non-zero and
hands the display back rather than leaving a black screen. **PASS.**

### 6.7 The Desktop ↔ Gaming switch

`apex-session-select` is the whole mechanism, and it is deliberately only half
of a switch:

```
apex-session-select <id>            # remember it
apex-session-select <id> --switch   # remember it, then `loginctl terminate-user`
```

Its own header says why there is no autologin: "the end state for both machines
is LUKS2 + TPM2 full-disk encryption, and quietly adding a passwordless
graphical login to that is a security change nobody asked for. Switching modes
costs one password entry at the greeter."

**Verified — run, not read:**

```
$ sudo cat /var/lib/apex-greet/last-session                 → hyprland
$ sudo /usr/libexec/apex-session-select --list
apex-gaming   apex-labwc   apex-safe-graphics   hyprland   niri
$ sudo /usr/libexec/apex-session-select apex-gaming
apex-session-select: next session: apex-gaming
$ sudo cat /var/lib/apex-greet/last-session                 → apex-gaming
$ apex gaming | grep "boots to game"
boots to game   : yes — the greeter will preselect Gaming Mode
$ sudo /usr/libexec/apex-session-select 'bogus;id'
apex-session-select: refusing suspicious session id 'bogus;id'          rc=1
$ sudo /usr/libexec/apex-session-select '../../etc/passwd'
apex-session-select: refusing suspicious session id '../../etc/passwd'  rc=1
$ sudo /usr/libexec/apex-session-select 'not-a-session'
apex-session-select: 'not-a-session' is not an installed session        rc=1
$ sudo cat /var/lib/apex-greet/last-session                 → apex-gaming  (unchanged by the three refusals)
$ sudo /usr/libexec/apex-session-select hyprland            # RESTORED
$ apex gaming | grep "boots to game"
boots to game   : no — the greeter will preselect 'hyprland'
```

**PASS** for the preselect half: it writes, `apex gaming` reads the change back,
all three hostile ids are refused without touching the state file, and the
sudoers-safe validation the header claims is the validation that runs. Andre's
preselect was restored to `hyprland` before this unit finished.
**COULD NOT RUN:** the half that needs a password at the greeter — this unit
does not have Andre's password, and `--switch` calls `loginctl terminate-user`,
which the script's own comment warns "ends EVERY session belonging to the
caller — including an SSH login." Running it would have ended this unit's own
connection to the machine. Marked COULD-NOT-RUN with the reason, not FAIL.

---

## 7. The P1-038 labwc matrix — all 18 rows, with what stopped each one

Run in the `apex-labwc` session on VT 2. Applications were launched with
`~/qual/apptest.sh`, which screenshots before and after, diffs the two in
PIL to get a painted-pixel count and a bounding box, and resolves Wayland vs
XWayland by matching **unix-socket peer inodes** in `ss -xp` rather than
guessing from environment variables (`~/qual/who-connects.py`).

| # | row | verdict | evidence |
|---|---|---|---|
| 1 | Firefox screen sharing | **PARTIAL — see §7.1** | Firefox 155.0 runs as a **native Wayland client** (socket-peer match, not env). The portal offers **monitors only** |
| 2 | Chromium screen sharing | **COULD NOT RUN** | no Chromium on the machine; `command -v chromium chromium-browser google-chrome` all empty, and none is a flatpak here |
| 3 | OBS | **COULD NOT RUN** | OBS is not installed. The capture primitive underneath it was tested instead — see row "recording" |
| 4 | Discord — Go Live | **COULD NOT RUN** | Discord is not installed. `~/.local/bin/equibop` exists as a launcher stub but the app does not |
| 5 | Flatpak portals | **PARTIAL PASS** | four sandboxed flatpaks launched and painted (rows 9–12 below); the portal's file-chooser and openURI paths need a human clicking and were not exercised |
| 6 | Steam desktop + Big Picture | **desktop PASS / Big Picture FAIL** | §6.4 and §6.3 |
| 7 | gamescope | **FAIL** | §6.1 — wrong GPU, wrong output |
| 8 | Wine / XWayland | **PARTIAL PASS** | Wine is not installed, but three real XWayland clients ran under labwc and painted: `steam`, `steamwebhelper` (x2) and `FreeCAD`. FreeCAD's window bbox is `(6,6,1808,1074)` — it starts 6 px down, i.e. **not** under the bar mask |
| 9 | VS Code (Electron/Ozone) | **COULD NOT RUN as such** | no VS Code. Closest proxy run: **Spotify** (Electron flatpak) — it started but mapped **no window** in 35 s; the only screen change was `129 px` at bbox `(1864,13)-(1875,27)`, which is its tray icon appearing in the APEX bar. Recorded as a StatusNotifierItem PASS and an Electron-window NOT-ESTABLISHED |
| 10 | JetBrains | **COULD NOT RUN as such** | no JetBrains IDE. Qt proxy run: **FreeCAD** — painted 89.4 % of the screen, via **XWayland** |
| 11 | LibreOffice (gtk3 / qt6 VCL) | **COULD NOT RUN as such** | no LibreOffice. GTK3 proxy run: **darktable** — **native Wayland**, painted 100.0 % (bbox `(0,0,1920,1080)`) |
| 12 | Blender | **PASS** | flatpak `org.blender.Blender`, **native Wayland**, painted 95.2 % (bbox `(6,6,1914,1074)`), own decorations, `bwrap→bwrap→blender` |
| 13 | fractional scaling | **PASS** | `wlr-randr --output HDMI-A-1 --scale 1.25 / 1.5 / 1.0` — each applied and read back; `quickshell` survived all three |
| 14 | rotation 90/180/270 | **PASS** | transform applied and read back at each; capture geometry follows (`1080x1920` at 90 and 270, `1920x1080` at 180 and normal) and the content is not black — 207 714 / 215 988 distinct colours |
| 15 | refresh-rate switching | **PASS** | HDMI-A-1 239.964 Hz → 60.000 Hz → 239.964 Hz on the NVIDIA output; the bar survived without a restart (`pgrep -c quickshell` = 1 throughout) |
| 16 | VRR | **COULD NOT RUN** | see §7.2 — no connector on this machine exposes `vrr_capable` at all |
| 17 | suspend / resume | see §8 | |
| 18 | dock / output hotplug | **PARTIAL — see §7.3** | |
| + | screenshots | **PASS** | `grimblast save output` under Hyprland dispatched to `grim -t png -o eDP-1` and wrote a valid 1920x1080 PNG |
| + | recording | **PASS** | §7.4 |
| + | multi-monitor | **PASS** | §5, and the differing-mode capture that settled per-output targeting |
| + | lock / idle | **PASS** | §5.3 — the lock is compositor-enforced |

### 7.1 Screen sharing under labwc can share an output, never a window

```
$ busctl --user get-property org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop org.freedesktop.portal.ScreenCast AvailableSourceTypes
u 1
```

`1` is `MONITOR`. `WINDOW` is `2` and `VIRTUAL` is `4`; neither is offered. That
is `xdg-desktop-portal-wlr`, which the image selects for labwc:

```
$ cat /usr/share/xdg-desktop-portal/labwc-portals.conf
[preferred]
default=wlr;*
org.freedesktop.impl.portal.Inhibit=none
```

So rows 1–4 all share the same ceiling on this session: a user can share a
**screen**, and the "share a window" option Firefox, Chromium, Discord and Teams
all present will have nothing behind it. Whether that is acceptable is a product
call, but it should be a decided one rather than a discovered one.

`ScreenCast` version 5 and `Screenshot` version 2 are both exported and the
portal service is up.

### 7.2 VRR cannot be tested on this machine, and Gaming Mode's probe can never fire

```
$ for p in /sys/class/drm/*/vrr_capable; do [ -e "$p" ] && echo "$p = $(cat $p)"; done
        (nothing)
$ ls /sys/class/drm/card2-HDMI-A-1/
edid  enabled  modes  power  status  subsystem  uevent
$ wlr-randr --output HDMI-A-1 --adaptive-sync enabled
failed to apply configuration
$ wlr-randr | grep -i adaptive
  Adaptive Sync: disabled
```

The NVIDIA connector exposes **seven** sysfs attributes and `vrr_capable` is not
among them (the Intel connector exposes fifteen and also lacks it). This is a
`COULD NOT RUN`, not a FAIL — the monitor is a 240 Hz Lenovo R25f-30, but
nothing in this driver stack publishes the property.

**Consequence worth recording for BASE-009:** `apex-gaming-session` decides
whether to pass `--adaptive-sync` by globbing exactly this path —

```bash
for p in /sys/class/drm/card*-*/vrr_capable; do
    [ -r "$p" ] || continue
    if [ "$(cat "$p" 2>/dev/null)" = "1" ]; then VRR_ARGS=(--adaptive-sync); …
```

— so on this machine the branch is unreachable and the log line "VRR-capable
output found" can never print. The probe is correct and safe; it simply cannot
answer on an NVIDIA output. If VRR matters for Gaming Mode on NVIDIA, the
property has to come from somewhere else.

### 7.3 Output hotplug — what could and could not be done

A real hotplug needs a hand on the cable and there was nobody at the machine, so
the physical row is **COULD NOT RUN**. What was done instead, and what it does
and does not prove:

- The compositor was made to reconfigure the NVIDIA output repeatedly —
  three scale changes, four transforms, three mode changes, one 1280x720
  resize that changed the whole desktop from 3840x1080 to 3200x1080. The bar
  process survived every one (`pgrep -c -u andre quickshell` = 1 throughout)
  and the per-output capture followed the new geometry each time.
- That covers "the bar survives an output reconfiguration". It does **not**
  cover connector removal — no `hotplug` uevent was generated, per-output
  surfaces were never destroyed, and the dashboard was not open on an output
  that went away. Those three remain untested.

### 7.4 Recording works, on the NVIDIA output

`ScreenRecService` reaches the compositor through `wf-recorder`, which is
wlr-screencopy:

```
$ wf-recorder -o HDMI-A-1 -f /tmp/qual-rec.mp4 -x yuv420p   # ~6 s, then SIGINT
$ ffprobe -show_entries format=duration,size -show_entries stream=codec_name,width,height /tmp/qual-rec.mp4
codec_name=vp9   width=1920   height=1080   duration=5.452756   size=53354
```

A real 5.45-second 1920x1080 VP9 file off the discrete-GPU output. **PASS** —
the P1-038 "read but not run" recording row is now run.

### 7.5 Harness caveat — the portal rows are labwc-only evidence

This unit's SSH login kept the user manager (`systemd --user`, pid 1563) alive
across every session change, so `xdg-desktop-portal` (pid 9692), started under
**labwc** with the `wlr` backend selected, outlived labwc and went on serving
Hyprland and niri with the wrong backend:

```
labwc   portals: xdg-desktop-portal, -gtk, -wlr, -hyprland
hyprland portals: xdg-desktop-portal        (only — the backends died with labwc)
niri     portals: xdg-desktop-portal        (same)
```

Under greetd this cannot happen: `loginctl terminate-user` takes the user
manager with it and the next session starts its own portal. **Do not read §7.1
as a statement about Hyprland or niri** — it is labwc's number, measured in
labwc's session. The Hyprland and niri portal rows are untested.

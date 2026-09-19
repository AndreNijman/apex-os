# katana — hardware qualification of the roadmap image, 2026-09-19

Unit: `katana-qual`. Machine: MSI Katana GF76 12UG, hostname `apex`, user `andre`.

| | |
|---|---|
| image | `ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2` |
| digest | `sha256:ba263890b69619532b13b9c64e0ac5dcb8eced78bffdc9fd542ccac92607503d` |
| version | `apex (2026-09-18T14:33:05Z)` |
| vendored apex-shell | `b7953e4f` |
| rollback slot | `:gaming-nvidia`, `sha256:308127d9…`, 2026-09-05, `Unlocked: hotfix` |
| kernel | 7.2.3 |
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

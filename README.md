# APEX-OS

APEX-OS is an atomic Linux distribution built on **Fedora bootc** (OCI-native,
image-based, transactional updates with rollback). Its desktop is
[APEX Shell](https://github.com/AndreNijman/apex-shell) on Hyprland. The image
build vendors the shell into `/usr/share/apex-shell` instead of cloning it into
your home directory, and **apexd**, a first-party system daemon, manages the
machine.

## One image, every laptop

APEX-OS used to ship three editions (Daily, Gaming Mesa and Gaming NVIDIA), and
you had to pick one at install time without the information to pick well.
Someone with a gaming laptop who played a game now and then picked Daily and
found their GPU had no driver.

Now there is one image. CI builds the NVIDIA driver and the Xbox controller
modules (xone, xpadneo) into it and signs them with the APEX Machine Owner Key.
The gaming userspace stays out of the image and installs when you want it:

```sh
sudo apex install steam gamescope mangohud gamemode
```

The line between the two is technical. A kernel module has to be signed by a
key that only CI holds, so nobody can add one to a running machine under Secure
Boot; userspace has no such limit. Every kernel module ships in the image, and
everything else is a package.

`ghcr.io/andrenijman/apex-os:apex` is the image. `:daily`, `:gaming-mesa` and
`:gaming-nvidia` resolve to the same digest, so machines installed before the
merge keep updating and their owners have nothing to do.

The "spark" logo is the mark, in chartreuse, with mono (black/white) variants
for neutral contexts. See [docs/branding.md](docs/branding.md).

## The desktop

You log in to Hyprland running APEX Shell. The login screen also offers niri
(**APEX Scrolling**), labwc (**APEX Floating**) and **APEX Safe Graphics**;
**APEX Gaming Mode** appears once you install gamescope.

- **APEX Shell** is the redesigned shell from apex-shell's `main` branch. Its
  panels open and close on springs, and the Reduce Motion setting removes the
  movement. Settings grows out of the notch at the top centre of the screen,
  and the Dashboard's last tab opens it. Volume and brightness changes show
  inside that notch. The workspace indicator shows only the workspaces in use,
  each with its number.
- **Lock screen.** Locking pours the notch down over a picture of your desktop,
  and unlocking runs the same motion backwards. Each character you type in the
  password field appears as a shape, so the screen shows how many characters
  you have typed and never which ones.
- **Login screen.** The password field draws the lock screen's shapes, moving
  with the last user's motion settings (speed, scale and Reduce Motion). If the
  shapes cannot load, it falls back to plain dots.
- **Windows.** Hyprland opens, moves and switches workspaces on springs, and a
  closing window shrinks away over 200 ms. SUPER + left-drag moves a window and
  SUPER + right-drag resizes it. The settings live in
  `files/desktop/hypr/apex/appearance.lua`.
- **Boot splash.** On a black screen the APEX spark comes into focus out of its
  own glow and the wordmark fades in. On an encrypted disk the passphrase prompt
  appears on the splash, with a dot for each character you type and the
  keyboard layout in use.

All of it ships in the image. Each build of `main` takes APEX Shell from
apex-shell's `main` branch and records the commit in
`/usr/share/apex-shell/.apex-shell-commit`. The shell, the login screen and the
splash update together with `sudo apex update` and roll back together with
`sudo apex rollback`; APEX Shell has no updater of its own.

## Installing

You need a USB stick of **4 GB or more** (it will be erased), a machine with at
least **16 GB** of disk, and **internet on that machine while installing**: the
installer downloads APEX-OS during the install.

The installer stages the download on disk before it installs, and that needs
room:

- With a second drive or USB stick that has **32 GB free**, the installer stages
  the download there and does not erase it.
- With nothing else to stage on, it stages the download on the disk you install
  to, and that disk needs **about 53 GB**.

Allow about 30 minutes start to finish, most of it waiting.

---

### Step 1: Download

From the [Releases page](https://github.com/AndreNijman/apex-os/releases), take
the ISO plus its `.sha256` file:

| File | What it installs |
|------|------------------|
| `apex-os-netinstall-x86_64.iso` | APEX-OS. One ISO, because there is one image. |

Releases before v1.0.0 published one ISO per edition
(`apex-os-daily-netinstall.iso`, `apex-os-gaming-nvidia-netinstall.iso`). Those
names are gone; take the newest release.

Each ISO downloads the exact APEX-OS build it was tested with, not whatever was
published last. The ISO records that image digest at
`/usr/lib/apex-installer/image-digest`, and a release should quote it in its
notes. The installed machine follows the normal `:apex` update channel from then
on, so the first `sudo apex update` brings it current.

Check that the download is intact. A truncated ISO fails much later, in ways
that look like hardware problems.

**Linux / macOS**

```sh
sha256sum -c apex-os-netinstall-x86_64.iso.sha256     # macOS: shasum -a 256 -c
```

**Windows** (PowerShell): compare the output to the contents of the `.sha256`
file.

```powershell
Get-FileHash .\apex-os-netinstall-x86_64.iso -Algorithm SHA256
```

---

### Step 2: Write it to the USB stick

> **This erases the whole stick.** On Linux, naming the wrong device erases that
> device instead, with no confirmation and no undo. Check twice.

**Windows: use [Rufus](https://rufus.ie/)** (portable, no install):

1. Plug in the stick and open Rufus.
2. **Device**: select your stick. Confirm the size looks right.
3. **Boot selection** → SELECT → choose the `.iso`.
4. Leave everything else alone and click **START**.
5. If asked *ISOHybrid image detected*, choose **Write in DD Image mode**.
6. Confirm the erase warning and wait.

[balenaEtcher](https://etcher.balena.io/) also works and asks fewer questions:
select image, select drive, Flash.

**Linux**

```sh
lsblk                       # identify the stick — check SIZE, not just the name
sudo dd if=apex-os-netinstall-x86_64.iso of=/dev/sdX bs=4M oflag=direct status=progress
sync
```

Use the **whole disk** (`/dev/sdX`), never a partition (`/dev/sdX1`).

**macOS**

```sh
diskutil list                          # find the disk, e.g. /dev/disk4
diskutil unmountDisk /dev/diskN
sudo dd if=apex-os-netinstall-x86_64.iso of=/dev/rdiskN bs=4m
```

---

### Step 3: Only if you are keeping Windows on the same machine

Skip this if APEX is taking the whole disk.

The installer can install into an existing partition, but it will **not** shrink
Windows for you. Do that from Windows first:

1. **Suspend BitLocker**: Control Panel → BitLocker → *Suspend protection*.
   If you change the boot configuration with BitLocker active, Windows demands a
   48-digit recovery key on the next boot.
2. **Turn off Fast Startup**: Control Panel → Power Options → *Choose what the
   power buttons do* → uncheck **Turn on fast startup**. Fast Startup leaves the
   Windows partition in a half-hibernated state that is unsafe to resize.
3. **Shrink C:**: right-click Start → Disk Management → right-click `C:` →
   *Shrink Volume*. Give APEX at least 53 GB: with no second drive plugged in,
   the installer stages the download on this partition. With a second drive or
   USB stick that has 32 GB free, 40 GB is enough.
4. **Create a partition in the free space**: right-click the unallocated space
   → *New Simple Volume* → accept the defaults. The installer needs a real
   partition to select; unallocated space will not appear.
5. Reboot into Windows once, cleanly, before installing.

---

### Step 4: Boot the stick

Restart and open the **one-time boot menu**: usually <kbd>F12</kbd>, sometimes
<kbd>F9</kbd>, <kbd>F10</kbd> or <kbd>Esc</kbd> (ThinkPad F12, Dell F12, HP F9,
Acer F12, MSI F11, ASUS Esc). Pick the USB entry.

If the stick is not listed, go into firmware setup and disable **Fast Boot**.
The stick boots both UEFI and legacy BIOS machines, so either mode is fine.

At the APEX menu:

| Entry | Use it when |
|-------|-------------|
| **Install APEX-OS** | Always start here |
| **Safe graphics** | The screen goes black after the menu |
| **Troubleshoot** | The stick is not found (drops to a debug shell) |

The graphical installer appears after about 30-60 seconds.

---

### Step 5: Work through the installer

The installer has seven numbered steps. Three of them have an extra page that
appears only when it applies: picking a partition, disk encryption and Secure
Boot.

**1 · Welcome**: read and continue.

**2 · Keyboard and time zone**: pick your layout (and variant) and your time
zone. Type into the test field and check the characters that come out. The
disk passphrase prompt uses this layout at every boot, long before anything
configurable has loaded.

**3 · Network**: choose your Wi-Fi and enter the password. Enterprise networks
(school, university, work) also ask for a username. For a network that does not
broadcast its name, type it in the *hidden network* field. On Ethernet the page
reports that you are connected.
**Do not skip this.** The download needs it, and the installer copies the
connection into the installed system so it is online at first login.

**4 · Disk**: choose the target. The installer never offers the stick you booted
from. An empty list usually means the drive is in RAID/RST mode in firmware:
switch it to **AHCI** and rescan.

**5 · Use**: the whole disk, or the single partition you prepared in Step 3.

**6 · Account**: the username must be **lowercase**, start with a letter or
underscore, and contain no spaces. Set a password and a computer name.

**6 · Encrypt this disk** *(whole-disk installs only)*: LUKS2 over the whole
root, **ticked by default**, and a plain "no" if you do not want it. The
passphrase field has an eye icon; use it, because you type this passphrase again
at a boot prompt with one keyboard layout and no way back. The page names the
layout you chose on page 2 for the same reason. An install into an existing
partition cannot be encrypted: the engine refuses that combination rather than
inventing somewhere to put an unencrypted `/boot`.

**6 · Secure Boot** *(UEFI machines)*: choose a one-time password to enrol the
APEX signing key, or skip. Enrol even if Secure Boot is off now, so you can
switch it on later without reinstalling.

**7 · Confirm**: the page lists every partition as **ERASED**, **KEPT** or
**SHARED**. The installer has written nothing up to this point. Type `ERASE`
and start the install. You confirm the disk itself, not its name: the installer
records each device's serial, size and partition IDs on this page and checks
them again immediately before the first write, so it cannot erase a USB drive
swapped in during the download in place of the one you confirmed.

If you encrypted the disk, the final screen prints a **recovery key** and will
not let you reboot until you tick that you have written it down. The key opens
the disk when the passphrase will not, including when the keyboard produces the
wrong characters, and its letters sit in the same place on nearly every layout.
The installer also tries to drop a copy on the USB stick. That stick travels in
the same bag as the laptop, so move the key somewhere else and delete the file.

---

### Step 6: First boot

Installation takes 10-25 minutes depending on your connection. When it finishes,
remove the stick and reboot.

If you set a Secure Boot password, a blue **MOK management** screen appears
first. The firmware uses it to confirm a person is physically present, and it
appears only once:

> **Enroll MOK → Continue → Yes →** type that password **→ Reboot**

Then log in with the account you created. The first login needs no network:
the shell and its setup ship in the image. Once the machine is online, it adds
the Flathub remote and installs the Zen browser from it in the background. Boot
does not wait for either.

---

### If something goes wrong

The installer never leaves you at a blank screen: it prints what failed and
drops to a root shell. Photograph the screen; that is usually enough to
diagnose the failure.

- <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>F2</kbd> gives a login: `root` / `apex`
- Logs: `/var/log/apex-install.log` and `/var/log/apex-installer-launch.log`
- The installer writes nothing to any disk until you type `ERASE`, so a failure
  before that point has changed nothing

Please open an issue with the photograph or the log.

### Known limitations

- **USB Wi-Fi adapters needing out-of-tree drivers** (RTL8812AU / 88x2bu /
  8188eu) do not work in the installer. Use Ethernet or phone USB tethering.
- **Captive-portal Wi-Fi** (hotel/airport sign-in pages) cannot be completed:
  the installer has no browser.
- **Tablets with no physical keyboard** cannot complete the account step; the
  installer has no on-screen keyboard yet.

### Building the ISOs yourself

You need podman, about 90 GB free and about 40 minutes (the netinstall needs
far less space, because it embeds no OS image).

```sh
cd installer

# small ISO that downloads the OS during the install — this is the published one
NETINSTALL=1 EDITION=apex WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-os-netinstall-x86_64.iso sudo -E bash build-live-iso.sh

# fat ISO with the whole OS embedded — installs with no network at all
sudo skopeo copy containers-storage:localhost/apex-os:apex \
  oci-archive:/var/tmp/apex-iso/apex.oci:apex-os-apex
EDITION=apex WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-os-x86_64.iso sudo -E bash build-live-iso.sh
```

`EDITION` names the tag the installed machine records as its update origin, so
it has to match a published tag. `daily`, `gaming-mesa` and `gaming-nvidia`
still work and resolve to the same image, but a production build
(`PRODUCTION=1`, the default) refuses anything but `apex`.

The netinstall build resolves `:apex` to a digest once, refuses it unless
`build-image.yml` on `main` signed it (cosign), and stamps it into the ISO. Pass
`RELEASE_DIGEST=sha256:…` to pin a specific build instead. Use the digest the
boot-tested build printed, so the ISO you publish downloads the image you
tested.

Before you publish a netinstall ISO, pin that digest. The digest loses its tag
when `:apex` moves on, and a registry cleanup removes untagged versions, so
every copy of the ISO would fail at its first download:

```sh
gh workflow run pin-netinstall-image.yml -f digest=sha256:… -f release=v2.1.0
```

The workflow checks the digest's signature and gives it a write-once
`netinstall-<release>` tag. The build prints the exact command at the end.

To build the OS images with a signed kernel, use `./build-local.sh`. It passes
the Secure Boot signing key and refuses to produce an unsigned image by
accident. `./build-local.sh kernel` builds the kernel tier on its own (about 45
minutes); `core`, `base` and `apex` build the tiers above it.


## Updating

APEX-OS is image-based, so an update replaces the whole OS atomically and you
can roll it back:

```sh
sudo apex update          # pull the newest image, then check firmware
sudo apex update --check  # report what is available, download nothing
sudo systemctl reboot     # boot into it
sudo apex rollback        # go back to the previous image if anything broke
```

`apex update`, `apex rollback` and `apex pin` change the booted system and
refuse to run without root. Each prints the exact `sudo` line to use instead of
failing somewhere inside `bootc`. Everything else (`apex status`, `tier`,
`battery`, `fan`, `doctor`) stays usable as your normal user, because the
desktop drives those.

Updates are incremental. CI builds the image in four tiers (kernel, core, base,
image) so that a typical release moves only the thin top ones.
[docs/update-cost.md](docs/update-cost.md) explains how that works and why it
matters: every update used to cost 5.3 GB.

There are four channels: `edge`, `beta`, `candidate` and `stable`. `:apex` (and
its three aliases) moves on every successful build of `main`, so a machine you
have never moved is on **edge**:

```sh
apex channel status          # which one this machine follows, and how the last update went
apex channel list            # what the four mean
sudo apex channel set beta   # from the next update onwards
```

CI enforces each promotion: it refuses a digest that is not already on the
channel above, and one that this repository's workflow on `main` has not
cosign-signed. Moving *toward* `stable` usually deploys an older image, so that
direction pins the current deployment first and says what your persistent state
will and will not roll back with it. See
[docs/update-channels.md](docs/update-channels.md).

The images are public, at `ghcr.io/andrenijman/apex-os`.

## Installing software

```sh
sudo apex install android-tools   # any Fedora package
sudo apex install org.gimp.GIMP   # a reverse-DNS id installs the Flatpak
sudo apex install ~/app.rpm       # a path installs that RPM file
sudo apex remove  android-tools
apex search wireshark
apex resolve obs-studio           # which source APEX would use, and why
apex pkg list
```

A bare name can be an RPM, a Flatpak or something that belongs inside a
container, so APEX ranks the sources. `apex resolve` shows the ranking, what
vouches for each source, and the exact command for the alternatives; it is
read-only and needs no root. `--source rpm|flatpak|capsule` overrides the
ranking for one install. `sudo apex repo enable-copr OWNER/PROJECT` adds a COPR
to search, install and upgrades.

APEX builds packages into a systemd system extension overlaid on `/usr`; it does
**not** layer them with `rpm-ostree`. A single `rpm-ostree` layer puts the deployment
into "local modifications" state, and `bootc upgrade` refuses to run from then
on, so installing one CLI tool used to stop the machine updating without a
word. Extensions leave the deployment untouched, so software installs and OS
updates no longer exclude each other, and programs still land in the real
`/usr/bin` with working `.desktop` files, units and udev rules.

A local `.rpm` file goes through the same pipeline, so it lands in the launcher
with its icons and MIME types like any other application. APEX copies the file
into `/var/lib/apex/pkg/local` and every later rebuild uses that copy, so
`apex update` and the rebuild after an OS upgrade keep working once the original
file is gone; its dependencies still come from the repositories. APEX refuses
any RPM it cannot verify against a trusted key. To accept one anyway you pass an
explicit `--allow-unsigned` for that file, and `apex pkg list` says so
afterwards. APEX does not run `%post` scriptlets.

Already have layered packages? `sudo apex pkg adopt` converts them and restores
updates. See [docs/packages.md](docs/packages.md).

## Coding agents as an OS workload

`claude`, `opencode`, `codex`, `gemini` and anything else you already run keep
working as they do. APEX adds what sits underneath: the terminal they run on,
the confinement they run inside, and the project state around them. It is off
until you turn it on.

```sh
apex agent enable               # per-user; works for any user, root included
a                               # start an agent here
a "fix the failing tests"       # with an opening instruction
al                              # what is running
aa                              # reattach
ad                              # what it changed
```

APEX creates the PTY and then execs the ordinary agent binary inside it, so the
agent needs no changes. A daemon owns the terminal instead of your shell, so
closing the window does not kill the work. Detach with **ctrl-]** and reattach
from anywhere, including from the phone app.

Two daemons, split on purpose: `apex-agentd` is per-user and unprivileged and
handles untrusted model output; `apex-secretd` is the only root piece, holds
credentials, and has no verb that returns one. Sessions run in a bubblewrap
sandbox with `/` read-only and the home masked; the working directory is
writable. [docs/agent-runtime.md](docs/agent-runtime.md) is the reference.

## The AI desktop apps

The ChatGPT and Claude desktop applications are part of APEX-OS. The image
carries both, so they are there on a fresh install and reach an existing machine
through the ordinary `sudo apex update`. Neither self-updates and neither runs
an auto-update timer of its own: a new version is a new image. The Claude Code
CLI ships alongside them.

The build checks both packages' signatures against fingerprints pinned in this
repository, never against a key taken from the package being installed.

## Closing the lid without stopping the work

A laptop with live work in it keeps going when you shut the lid, VPN and all. A
laptop with nothing running suspends as it always did. You do not pick between
those; the machine measures which case it is in. Three things still take it
down with the lid shut (heat, a battery floor, or you), and each says which one
fired; the first two checkpoint first.

```sh
apex lid status     # what the policy sees, and what it would do now
apex lid explain    # the same decision with every input that produced it
apex lid report     # what the last closed period actually did
apex lid pin on     # keep working on a close, whatever is running
apex lid pin auto   # hand the decision back to the measurement
```

[docs/lid.md](docs/lid.md) has the full verb list and the guard thresholds.

## The phone app

APEX Remote pairs a phone with one of your machines, over your network or
through a relay when you are away from it, and lets you watch and drive what is
running on it: agent sessions, approvals, and a real terminal. It talks only to
machines you have paired by scanning a QR code off their screen. There is no
account and no server of ours in the middle.

The APK goes to the same
[Releases page](https://github.com/AndreNijman/apex-os/releases) as the ISO,
under its own `android-v<version>` tags, with a `.sha256` beside it. Each release
explains, for somebody who has never sideloaded an app, what Android will ask
and how to answer it.

**No APK release has been cut yet.** The signing key exists, the fingerprint
below is real, and `.github/workflows/release-android.yml` is what publishes one;
nobody has pushed an `android-v*` tag.

One certificate signs every APEX Remote APK, and this is its SHA-256
fingerprint:

<!-- fingerprint:begin -->
9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67
<!-- fingerprint:end -->

`apksigner verify --print-certs apex-remote-<version>.apk` prints the
certificate that signed your download; it must be that value. (`UNSET` in that
block would mean no signing key exists and no release can be cut.) From then on
Android enforces the same thing: it will not install an update signed by any
other key over it.

Once installed, the app keeps itself current: it checks the Releases page and
offers the update, and Android still shows its own install prompt before it
replaces anything. On the machine, `apex remote status` says whether the
service is running and which protocol version it speaks. Pairing is
`apex remote pair`.

[docs/android-app.md](docs/android-app.md) covers how the release is built, how
the version is derived, and what happens when the app and the machine are
different ages. [docs/android-signing.md](docs/android-signing.md) covers the
signing key: who holds it, why GitHub is not its backup, and how to rotate it.
[docs/remote.md](docs/remote.md) covers the app itself.

## Repository layout

| Path | Contents |
|------|----------|
| `Containerfile.kernel` | The kernel tier: APEX compiles its own kernel from pinned sources |
| `Containerfile.core` | Slow-moving foundation: kernel install + MOK signing, desktop stack, apps (bootc) |
| `Containerfile.base` | Thin per-commit tier on top of core: apexd, files/**, shell |
| `Containerfile.apex` | The published image: variant stamp, splash, final initramfs |
| `kernel/` | Kernel spec and `kernel.pin`: every input that decides what the kernel is |
| `installer/` | The live ISO build, the install engine and its GTK front end |
| `signing/` | The M0 Secure Boot signing-chain proof scripts (no private keys; production signing is in `Containerfile.core`) |
| `files/branding/` | Logos, Plymouth boot themes, wallpapers |
| `files/system/` | System-level files baked into the image |
| `files/desktop/` | Desktop / APEX Shell integration files |
| `files/scripts/` | Build and runtime helper scripts |
| `apexd/` | The Rust workspace: apexd, the `apex` CLI, and the agent/secret/backup/remote daemons |
| `config/sysprofiles/` | Per-machine hardware tuning profiles |
| `android/` | APEX Remote, the Android client (`:core` protocol, `:app` UI) |
| `relay/` | The Cloudflare Worker APEX Remote rendezvouses through when off-network |
| `tests/` | Image and integration tests |
| `docs/` | Project documentation |
| `.github/workflows/` | CI (image build, sign, publish) |

## Status

The latest release is **v2.1.0** (2026-09-26), on the
[Releases page](https://github.com/AndreNijman/apex-os/releases).

**The image and CI.** `Containerfile.kernel` → `Containerfile.core` →
`Containerfile.base` → `Containerfile.apex`. APEX compiles its own CachyOS-based
kernel in the first tier from the inputs pinned in `kernel/kernel.pin`. Core
installs it, signs it with the APEX MOK, builds the NVIDIA and controller akmods
against that exact kernel, and carries the desktop and greeter stack, scx (with
`scx_lavd` patched for an upstream stall while COPR ships 1.1.3) and Bazaar.
`.github/workflows/build-image.yml` builds, cosign-signs (keyless), pushes, and
verifies that `:apex`, `:daily`, `:gaming-mesa` and `:gaming-nvidia` all resolve
to the one digest.

**apexd.** The `apexd/` cargo workspace ships `apexd-core` (fingerprint, layered
profile selection, tier engine, `SysWriter`), the `apexd` daemon (frozen
`org.apexos.Apexd1` D-Bus API, AC/battery auto-switch, gated RyzenAdj EC-defeat
loop, Prometheus metrics on 127.0.0.1:9723), the `apex` control CLI, and the
agent, secret, backup and remote daemons beside them. The six system profiles
live in `config/sysprofiles/`. The frozen D-Bus contract is in
[docs/apexd-dbus.md](docs/apexd-dbus.md).

The development notes for each milestone record what happened then; they do
not describe the tree today:
[m0](docs/m0-results.md) (spikes) ·
[m1](docs/m1-notes.md) (first production image; its per-edition tables predate
the one-image merge) ·
[m3](docs/m3-notes.md) (apexd v1) ·
[m6](docs/m6-notes.md) (real fan control, game orchestration) ·
[p1](docs/p1-progress.md) · [p2](docs/p2-progress.md) ·
[p3](docs/p3-progress.md) · [p4](docs/p4-progress.md) (three editions become
one) · [experiments](docs/experiments.md).

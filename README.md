# APEX-OS

APEX-OS is an atomic Linux distribution built on **Fedora bootc** (OCI-native,
image-based, transactional updates with rollback). It ships
[APEX Shell](https://github.com/AndreNijman/apex-shell) as its native desktop —
vendored into the image at `/usr/share/apex-shell` rather than cloned into a
home directory — and is managed by **apexd**, a first-party system daemon.

## One image, every laptop

There used to be three editions — Daily, Gaming Mesa, Gaming NVIDIA — and
choosing between them at install time was a decision nobody had the information
to make. Someone with a gaming laptop who plays occasionally picked Daily and
found their GPU had no driver.

There is one image now. The NVIDIA driver and the Xbox controller modules
(xone, xpadneo) are built into it and signed with the APEX Machine Owner Key.
The gaming userspace is not, and installs when you want it:

```sh
sudo apex install steam gamescope mangohud gamemode
```

The line between the two is not a product decision, it is a technical one: a
kernel module has to be signed by a key that only CI holds, so it cannot be
added to a running machine under Secure Boot. Userspace can. Everything that is
a kernel module ships in the image; everything else is a package.

`ghcr.io/andrenijman/apex-os:apex` is the image. `:daily`, `:gaming-mesa` and
`:gaming-nvidia` still resolve to exactly the same digest, so machines installed
before the merge keep updating with nothing to do.

The "spark" logo is the mark, in chartreuse, with mono (black/white) variants
for neutral contexts. See [docs/branding.md](docs/branding.md).

## Installing

You need a USB stick of **4 GB or more** (it will be erased), a machine with at
least **16 GB** of disk, and **internet on that machine while installing** — the
download is done during the install, not before.

Roughly 30 minutes start to finish, most of it waiting.

---

### Step 1 — Download

From the [Releases page](https://github.com/AndreNijman/apex-os/releases), take
the ISO plus its `.sha256` file:

| File | What it installs |
|------|------------------|
| `apex-os-netinstall-x86_64.iso` | APEX-OS. One ISO, because there is one image. |

Releases before v1.0.0 published one ISO per edition
(`apex-os-daily-netinstall.iso`, `apex-os-gaming-nvidia-netinstall.iso`). Those
names are gone; take the newest release.

Check the download is intact. A truncated ISO fails much later, in ways that
look like hardware problems.

**Linux / macOS**

```sh
sha256sum -c apex-os-netinstall-x86_64.iso.sha256     # macOS: shasum -a 256 -c
```

**Windows** (PowerShell) — compare the output to the contents of the `.sha256`
file:

```powershell
Get-FileHash .\apex-os-netinstall-x86_64.iso -Algorithm SHA256
```

---

### Step 2 — Write it to the USB stick

> **This erases the whole stick.** On Linux, naming the wrong device erases that
> device instead, with no confirmation and no undo. Check twice.

**Windows — use [Rufus](https://rufus.ie/)** (portable, no install):

1. Plug in the stick and open Rufus.
2. **Device** — select your stick. Confirm the size looks right.
3. **Boot selection** → SELECT → choose the `.iso`.
4. Leave everything else alone and click **START**.
5. If asked *ISOHybrid image detected*, choose **Write in DD Image mode**.
6. Confirm the erase warning and wait.

[balenaEtcher](https://etcher.balena.io/) also works and asks fewer questions —
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

### Step 3 — Only if you are keeping Windows on the same machine

Skip this if APEX is taking the whole disk.

The installer can install into an existing partition, but it will **not** shrink
Windows for you. Do that from Windows first:

1. **Suspend BitLocker** — Control Panel → BitLocker → *Suspend protection*.
   Changing the boot configuration with BitLocker active makes Windows demand a
   48-digit recovery key on the next boot.
2. **Turn off Fast Startup** — Control Panel → Power Options → *Choose what the
   power buttons do* → uncheck **Turn on fast startup**. Fast Startup leaves the
   Windows partition in a half-hibernated state that is unsafe to resize.
3. **Shrink C:** — right-click Start → Disk Management → right-click `C:` →
   *Shrink Volume*. Give APEX at least 40 GB.
4. **Create a partition in the free space** — right-click the unallocated space
   → *New Simple Volume* → accept the defaults. The installer needs a real
   partition to select; unallocated space will not appear.
5. Reboot into Windows once, cleanly, before installing.

---

### Step 4 — Boot the stick

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
| **Troubleshoot** | The stick is not found — drops to a debug shell |

The graphical installer appears after about 30–60 seconds.

---

### Step 5 — Work through the installer

Seven numbered steps. Three of them have an extra page that appears only when
it applies — picking a partition, disk encryption, Secure Boot.

**1 · Welcome** — read and continue.

**2 · Keyboard and time zone** — pick your layout (and variant) and your time
zone. There is a test field: type into it and check the characters that come
out. This is not cosmetic — it is the layout the disk passphrase prompt will use
at every boot, long before anything configurable has loaded.

**3 · Network** — choose your Wi-Fi and enter the password. Enterprise networks
(school, university, work) also ask for a username. For a network that does not
broadcast its name, type it in the *hidden network* field. On Ethernet it simply
reports that you are connected.
**Do not skip this.** The download needs it, and the connection is copied into
the installed system so it is online at first login.

**4 · Disk** — choose the target. The stick you booted from is never offered. An
empty list usually means the drive is in RAID/RST mode in firmware — switch it
to **AHCI** and rescan.

**5 · Use** — the whole disk, or the single partition you prepared in Step 3.

**6 · Account** — the username must be **lowercase**, start with a letter or
underscore, and contain no spaces. Set a password and a computer name.

**6 · Encrypt this disk** *(whole-disk installs only)* — LUKS2 over the whole
root, **ticked by default**, and a plain "no" if you do not want it. The
passphrase field has an eye icon; use it, because this passphrase is typed again
at a boot prompt with one keyboard layout and no way back. The page names the
layout you chose on page 2 for the same reason. Installing into an existing
partition cannot be encrypted — the engine refuses that combination rather than
inventing somewhere to put an unencrypted `/boot`.

**6 · Secure Boot** *(UEFI machines)* — choose a one-time password to enrol the
APEX signing key, or skip. Worth doing even if Secure Boot is currently off, so
you can switch it on later without reinstalling.

**7 · Confirm** — every partition is listed as **ERASED**, **KEPT** or
**SHARED**. This is the last point at which nothing has been written. Type
`ERASE` and start the install.

If you encrypted the disk, the final screen prints a **recovery key** and will
not let you reboot until you tick that you have written it down. It opens the
disk when the passphrase will not — including when the keyboard is producing the
wrong characters — and its letters sit in the same place on nearly every layout.
The installer also tries to drop a copy on the USB stick; that stick travels in
the same bag as the laptop, so move the key somewhere else and delete the file.

---

### Step 6 — First boot

Installation takes 10–25 minutes depending on your connection. When it finishes,
remove the stick and reboot.

If you set a Secure Boot password, a blue **MOK management** screen appears
first. This is the firmware confirming a person is physically present, and it
happens only once:

> **Enroll MOK → Continue → Yes →** type that password **→ Reboot**

Then log in with the account you created. The desktop completes its setup on
first login and **needs network to do it**, which is why the network page
mattered.

---

### If something goes wrong

The installer never leaves you at a blank screen: it prints what failed and
drops to a root shell. Photograph the screen — that is usually enough to
diagnose it.

- <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>F2</kbd> gives a login: `root` / `apex`
- Logs: `/var/log/apex-install.log` and `/var/log/apex-installer-launch.log`
- Nothing is written to any disk until you type `ERASE`, so a failure before
  that point has changed nothing

Please open an issue with the photograph or the log.

### Known limitations

- **USB Wi-Fi adapters needing out-of-tree drivers** (RTL8812AU / 88x2bu /
  8188eu) do not work in the installer. Use Ethernet or phone USB tethering.
- **Captive-portal Wi-Fi** (hotel/airport sign-in pages) cannot be completed —
  there is no browser in the installer.
- **Tablets with no physical keyboard** cannot complete the account step; there
  is no on-screen keyboard yet.

### Building the ISOs yourself

Needs podman, about 90 GB free and roughly 40 minutes.

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
it has to match a published tag; `daily`, `gaming-mesa` and `gaming-nvidia` still
work and still resolve to the same image.

To build the OS images with a signed kernel, use `./build-local.sh` — it passes
the Secure Boot signing key and refuses to produce an unsigned image by accident.
`./build-local.sh kernel` builds the kernel tier on its own (about 45 minutes);
`core`, `base` and `apex` build the tiers above it.


## Updating

APEX-OS is image-based, so updates replace the whole OS atomically and can be
rolled back:

```sh
sudo apex update          # pull the newest image, then check firmware
sudo apex update --check  # report what is available, download nothing
sudo systemctl reboot     # boot into it
sudo apex rollback        # go back to the previous image if anything broke
```

`apex update`, `apex rollback` and `apex pin` change the booted system and
refuse to run without root — they tell you the exact `sudo` line to use instead
of failing somewhere inside `bootc`. Everything else (`apex status`, `tier`,
`battery`, `fan`, `doctor`) stays usable as your normal user, because the
desktop drives those.

Updates are incremental. The image is built in four tiers — kernel, core, base,
image — so that a typical release only moves the thin top ones; see
[docs/update-cost.md](docs/update-cost.md) for how that works and why it
matters (it used to be 5.3 GB, every time).

There are four channels — `edge`, `beta`, `candidate`, `stable`. `:apex` (and
its three aliases) moves on every successful build of `main`, so a machine that
has never been moved is on **edge**:

```sh
apex channel status          # which one this machine follows, and how the last update went
apex channel list            # what the four mean
sudo apex channel set beta   # from the next update onwards
```

A promotion is enforced in CI rather than promised: it refuses a digest that is
not already on the channel above, and one that is not cosign-signed by this
repository's workflow on `main`. Moving *toward* `stable` usually deploys an
older image, so that direction pins the current deployment first and says what
your persistent state will and will not roll back with it. See
[docs/update-channels.md](docs/update-channels.md).

Images are published to `ghcr.io/andrenijman/apex-os` and are public.

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
container, so APEX ranks the sources and `apex resolve` shows the ranking, what
vouches for each one, and the exact command for the alternatives. It is
read-only and needs no root. `--source rpm|flatpak|capsule` overrides the
ranking for one install. `sudo apex repo enable-copr OWNER/PROJECT` adds a COPR to
search, install and upgrades.

Packages are built into a systemd system extension overlaid on `/usr`, **not**
layered with `rpm-ostree`. That distinction is the whole point: a single
`rpm-ostree` layer puts the deployment into "local modifications" state and
`bootc upgrade` refuses to run from then on, so installing one CLI tool used to
silently stop the machine updating. Extensions leave the deployment untouched,
so software and OS updates stop being mutually exclusive — while programs still
land in the real `/usr/bin` with working `.desktop` files, units and udev rules.

A local `.rpm` file goes through that same pipeline, so it lands in the launcher
with its icons and MIME types like any other application. The file is copied into
`/var/lib/apex/pkg/local` and the copy is what every later rebuild uses, so
`apex update` and the rebuild after an OS upgrade keep working once the original
file is gone; its dependencies still come from the repositories. APEX refuses any
RPM it cannot verify against a trusted key — accepting one anyway takes an
explicit `--allow-unsigned` for that file, and `apex pkg list` says so afterwards.
`%post` scriptlets are not executed.

Already have layered packages? `sudo apex pkg adopt` converts them and restores
updates. See [docs/packages.md](docs/packages.md).

## Coding agents as an OS workload

`claude`, `opencode`, `codex`, `gemini` and anything else you already run keep
working exactly as they do. APEX adds what sits underneath: the terminal they
run on, the confinement they run inside, and the project state around them. It
is off until you turn it on.

```sh
apex agent enable               # per-user; works for any user, root included
a                               # start an agent here
a "fix the failing tests"       # with an opening instruction
al                              # what is running
aa                              # reattach
ad                              # what it changed
```

APEX creates the PTY and then execs the ordinary agent binary inside it, so
nothing about the agent has to change — and because a daemon owns the terminal
rather than your shell, closing the window does not kill the work. Detach with
**ctrl-]** and reattach from anywhere, including from the phone app.

Two daemons, split on purpose: `apex-agentd` is per-user and unprivileged and
handles untrusted model output; `apex-secretd` is the only root piece, holds
credentials, and has no verb that returns one. Sessions run in a bubblewrap
sandbox with `/` read-only and the home masked; the working directory is
writable. [docs/agent-runtime.md](docs/agent-runtime.md) is the reference.

## The AI desktop apps

The ChatGPT and Claude desktop applications are part of APEX-OS rather than
optional extras. Both are baked into the image, so they are there on a fresh
install and arrive on an existing machine through the ordinary `sudo apex
update`. Neither self-updates and neither runs an auto-update timer of its own:
a new version is a new image. The Claude Code CLI ships alongside them.

Both packages' signatures are checked at build time against fingerprints pinned
in this repository — not against a key taken from the package being installed.

## Closing the lid without stopping the work

A laptop with live work in it keeps going when you shut the lid, VPN and all. A
laptop with nothing running suspends exactly as it always did. You do not pick
between those; the machine measures which one it is. Three things still take it
down with the lid shut — heat, a battery floor, or you — and each says which one
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

APEX Remote pairs a phone with one of your machines — over your network, or
through a relay when you are away from it — and lets you watch and drive what is
running on it: agent sessions, approvals, and a real terminal. It talks only to
machines you have paired by scanning a QR code off their screen; there is no
account and no server of ours in the middle.

The APK goes to the same
[Releases page](https://github.com/AndreNijman/apex-os/releases) as the ISO,
under its own `android-v<version>` tags, with a `.sha256` beside it. Each release
explains, for somebody who has never sideloaded an app, exactly what Android will
ask and how to answer it.

**No APK release has been cut yet.** The signing key exists, the fingerprint
below is real, and `.github/workflows/release-android.yml` is what publishes one;
no `android-v*` tag has been pushed.

Every APEX Remote APK is signed by one certificate, and this is its SHA-256
fingerprint:

<!-- fingerprint:begin -->
9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67
<!-- fingerprint:end -->

`apksigner verify --print-certs apex-remote-<version>.apk` prints the
certificate that actually signed your download; it must be that value. (`UNSET`
means no release has been signed yet.) Android enforces the same thing from then
on: an update signed by any other key will not install over it.

Once installed the app keeps itself current — it checks the Releases page and
offers, and Android still shows its own install prompt before anything is
replaced. On the machine, `apex remote status` says whether the service is
running and which protocol version it speaks. Pairing is `apex remote pair`.

[docs/android-app.md](docs/android-app.md) covers how the release is built, how
the version is derived, and what happens when the app and the machine are
different ages. [docs/android-signing.md](docs/android-signing.md) covers the
signing key: who holds it, why GitHub is not its backup, and how it can be
rotated. [docs/remote.md](docs/remote.md) covers the app itself.

## Repository layout

| Path | Contents |
|------|----------|
| `Containerfile.kernel` | The kernel tier: APEX compiles its own kernel from pinned sources |
| `Containerfile.core` | Slow-moving foundation: kernel install + MOK signing, desktop stack, apps (bootc) |
| `Containerfile.base` | Thin per-commit tier on top of core: apexd, files/**, shell |
| `Containerfile.apex` | The published image: variant stamp, splash, final initramfs |
| `kernel/` | Kernel spec and `kernel.pin` — every input that decides what the kernel is |
| `installer/` | The live ISO build, the install engine and its GTK front end |
| `signing/` | MOK / Secure Boot signing tooling (no private keys) |
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

There is a stable release — **v1.0.0** — on the
[Releases page](https://github.com/AndreNijman/apex-os/releases).

**The image and CI.** `Containerfile.kernel` → `Containerfile.core` →
`Containerfile.base` → `Containerfile.apex`. APEX compiles its own CachyOS-based
kernel in the first tier from the inputs pinned in `kernel/kernel.pin`; core
installs it, signs it with the APEX MOK and builds the NVIDIA and controller
akmods against that exact kernel, and carries the desktop / greeter stack, scx
and Bazaar. `.github/workflows/build-image.yml` builds, cosign-signs (keyless),
pushes, and verifies that `:apex`, `:daily`, `:gaming-mesa` and `:gaming-nvidia`
all resolve to the one digest.

**apexd.** The `apexd/` cargo workspace ships `apexd-core` (fingerprint, layered
profile selection, tier engine, `SysWriter`), the `apexd` daemon (frozen
`org.apexos.Apexd1` D-Bus API, AC/battery auto-switch, gated RyzenAdj EC-defeat
loop, Prometheus metrics on 127.0.0.1:9723), the `apex` control CLI, and the
agent, secret, backup and remote daemons beside them. The six system profiles
live in `config/sysprofiles/`. The frozen D-Bus contract is in
[docs/apexd-dbus.md](docs/apexd-dbus.md).

Development notes are kept per milestone and are a record rather than a
statement of where the tree is today:
[m0](docs/m0-results.md) (spikes) ·
[m1](docs/m1-notes.md) (first production image; its per-edition tables predate
the one-image merge) ·
[m3](docs/m3-notes.md) (apexd v1) ·
[m6](docs/m6-notes.md) (real fan control, game orchestration) ·
[p1](docs/p1-progress.md) · [p2](docs/p2-progress.md) ·
[p3](docs/p3-progress.md) · [p4](docs/p4-progress.md) (three editions become
one) · [experiments](docs/experiments.md).

# APEX-OS

APEX-OS is a two-edition, atomic Linux distribution built on **Fedora bootc**
(OCI-native, image-based, transactional updates with rollback). It ships
[Brain_Shell](https://github.com/AndreNijman/brain-shell-void) as its native
desktop and is managed by **apexd**, a first-party system daemon.

## Editions

| Edition | Accent | Meaning | Target use |
|---------|--------|---------|------------|
| **APEX-OS Gaming** | Gold | Power | Performance-tuned desktop for gaming |
| **APEX-OS Daily**  | Chartreuse | Everyday | General-purpose daily driver |

Both editions share a common base image and diverge in packages, tuning, and
branding. The "spark" logo is the shared mark; gold and chartreuse colorways
distinguish the editions, with mono (black/white) variants for neutral
contexts. See [docs/branding.md](docs/branding.md).

## Installing

You need a USB stick of **4 GB or more** (it will be erased), a machine with at
least **16 GB** of disk, and **internet on that machine while installing** — the
download is done during the install, not before.

Roughly 30 minutes start to finish, most of it waiting.

---

### Step 1 — Download

From the [Releases page](https://github.com/AndreNijman/apex-os/releases), take
the edition you want plus its `.sha256` file:

| File | Edition |
|------|---------|
| `apex-os-daily-netinstall.iso` | **Daily** — general-purpose desktop |
| `apex-os-gaming-nvidia-netinstall.iso` | **Gaming** — performance-tuned, NVIDIA |

Check the download is intact. A truncated ISO fails much later, in ways that
look like hardware problems.

**Linux / macOS**

```sh
sha256sum -c apex-os-daily-netinstall.iso.sha256      # macOS: shasum -a 256 -c
```

**Windows** (PowerShell) — compare the output to the contents of the `.sha256`
file:

```powershell
Get-FileHash .\apex-os-daily-netinstall.iso -Algorithm SHA256
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
sudo dd if=apex-os-daily-netinstall.iso of=/dev/sdX bs=4M oflag=direct status=progress
sync
```

Use the **whole disk** (`/dev/sdX`), never a partition (`/dev/sdX1`).

**macOS**

```sh
diskutil list                          # find the disk, e.g. /dev/disk4
diskutil unmountDisk /dev/diskN
sudo dd if=apex-os-daily-netinstall.iso of=/dev/rdiskN bs=4m
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

**1 · Welcome** — read and continue.

**2 · Network** — choose your Wi-Fi and enter the password. Enterprise networks
(school, university, work) also ask for a username. For a network that does not
broadcast its name, type it in the *hidden network* field. On Ethernet it simply
reports that you are connected.
**Do not skip this.** The download needs it, and the connection is copied into
the installed system so it is online at first login.

**3 · Disk** — choose the target. The stick you booted from is never offered. An
empty list usually means the drive is in RAID/RST mode in firmware — switch it
to **AHCI** and rescan.

**4 · Use** — the whole disk, or the single partition you prepared in Step 3.

**5 · Account** — the username must be **lowercase**, start with a letter or
underscore, and contain no spaces. Set a password and a computer name.

**5 · Secure Boot** *(UEFI machines)* — choose a one-time password to enrol the
APEX signing key, or skip. Worth doing even if Secure Boot is currently off, so
you can switch it on later without reinstalling.

**6 · Confirm** — every partition is listed as **ERASED**, **KEPT** or
**SHARED**. This is the last point at which nothing has been written. Type
`ERASE` and start the install.

---

### Step 6 — First boot

Installation takes 10–25 minutes depending on your connection. When it finishes,
remove the stick and reboot.

If you set a Secure Boot password, a blue **MOK management** screen appears
first. This is the firmware confirming a person is physically present, and it
happens only once:

> **Enroll MOK → Continue → Yes →** type that password **→ Reboot**

Then log in with the account you created. The desktop completes its setup on
first login and **needs network to do it**, which is why Step 5.2 mattered.

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
- The installer assumes a **US keyboard layout** when you type your password.

### Building the ISOs yourself

Needs podman, about 90 GB free and roughly 40 minutes.

```sh
cd installer

# small ISO that downloads the OS during the install
NETINSTALL=1 EDITION=daily WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-daily-netinstall.iso sudo -E bash build-live-iso.sh

# fat ISO with the whole OS embedded — installs with no network at all
sudo skopeo copy containers-storage:localhost/apex-os:daily \
  oci-archive:/var/tmp/apex-iso/apex.oci:apex-os-daily
EDITION=daily WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-daily.iso sudo -E bash build-live-iso.sh
```

To build the OS images with a signed kernel, use `./build-local.sh` — it passes
the Secure Boot signing key and refuses to produce an unsigned image by accident.


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

Updates are incremental. The image is built in three tiers so that a typical
release only moves the thin top ones; see
[docs/update-cost.md](docs/update-cost.md) for how that works and why it
matters (it used to be 5.3 GB, every time).

Images are published to `ghcr.io/andrenijman/apex-os` and are public.

## Installing software

```sh
sudo apex install android-tools   # any Fedora package
sudo apex remove  android-tools
apex search wireshark
apex pkg list
```

Packages are built into a systemd system extension overlaid on `/usr`, **not**
layered with `rpm-ostree`. That distinction is the whole point: a single
`rpm-ostree` layer puts the deployment into "local modifications" state and
`bootc upgrade` refuses to run from then on, so installing one CLI tool used to
silently stop the machine updating. Extensions leave the deployment untouched,
so software and OS updates stop being mutually exclusive — while programs still
land in the real `/usr/bin` with working `.desktop` files, units and udev rules.

Already have layered packages? `sudo apex pkg adopt` converts them and restores
updates. See [docs/packages.md](docs/packages.md).

## Repository layout

| Path | Contents |
|------|----------|
| `Containerfile.core` | Slow-moving foundation: kernel, desktop stack, apps (bootc) |
| `Containerfile.base` | Thin per-commit tier on top of core: apexd, files/**, shell |
| `Containerfile.daily` | Daily edition image (chartreuse) |
| `Containerfile.gaming` | Gaming edition image (gold) |
| `kernel/` | Custom kernel config / build inputs |
| `signing/` | MOK / Secure Boot signing tooling (no private keys) |
| `files/branding/` | Logos, Plymouth boot themes, wallpapers |
| `files/system/` | System-level files baked into the image |
| `files/desktop/` | Desktop / Brain_Shell integration files |
| `files/scripts/` | Build and runtime helper scripts |
| `apexd/` | apexd system daemon source |
| `config/sysprofiles/` | System tuning profiles (gaming/daily) |
| `tests/` | Image and integration tests |
| `docs/` | Project documentation |
| `.github/workflows/` | CI (image build, sign, publish) |

## Status

**M3 — apexd v1 (power engine).** The `apexd/` cargo workspace ships
`apexd-core` (fingerprint, layered profile selection, tier engine, `SysWriter`),
the `apexd` daemon (frozen `org.apexos.Apexd1` D-Bus API, AC/battery
auto-switch, gated RyzenAdj EC-defeat loop, Prometheus metrics on :9723), and
the `apex` control CLI. The six system profiles live in `config/sysprofiles/`.
See [docs/m3-notes.md](docs/m3-notes.md) and the frozen contract in
[docs/apexd-dbus.md](docs/apexd-dbus.md).

**M1 — production images + CI.** The three images build locally (shared
`Containerfile.base` → `daily` / `gaming`, with the CachyOS kernel, desktop /
greeter stack, scx, and Bazaar), the DIY NVIDIA akmod builds against the shared
kernel, and `.github/workflows/build-image.yml` builds, cosign-signs (keyless),
and pushes all three to GHCR. See [docs/m1-notes.md](docs/m1-notes.md) for build
results, package drift, and open items (greeter render still needs a GL target).
Earlier: [docs/m0-results.md](docs/m0-results.md) (spikes) and
[docs/experiments.md](docs/experiments.md).

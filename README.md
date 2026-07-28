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

Download the installer, write it to a USB stick, boot it, and follow six
screens. The installer is graphical, it never leaves you at a text prompt, and
nothing on any disk is touched until you type `ERASE` on the confirmation
screen.

### 1. Get the installer

**Netinstall (recommended, ~1.8 GB)** — grab the newest ISO from
[Releases](https://github.com/AndreNijman/apex-os/releases). It contains the
installer only and downloads APEX-OS itself (~5 GB) while you watch, so you
need a working internet connection during the install.

**Offline ISO (~12 GB)** — the whole OS is embedded, so the install needs no
network at all. Too large to publish here; build it yourself (see below) or get
it from someone who has one.

### 2. Write it to a USB stick

Any 4 GB or larger stick. **This erases the stick.**

```sh
# Linux/macOS — check the device name first, this is unforgiving
lsblk                                   # find your stick, e.g. /dev/sdb
sudo dd if=apex-os-daily-netinstall.iso of=/dev/sdb bs=4M oflag=direct status=progress
sync
```

On Windows use [Rufus](https://rufus.ie/) or
[balenaEtcher](https://etcher.balena.io/) in DD/image mode.

The image is a hybrid ISO: the same stick boots on **UEFI and legacy BIOS**
machines, and works with Secure Boot on (it ships the signed shim chain).

### 3. Boot it

Restart and pick the stick from your firmware's boot menu — usually **F12**,
sometimes F9/F10/Esc. You may need to disable Fast Boot, and on some machines
put the firmware in UEFI (non-CSM) mode.

The boot menu offers:

| Entry | When to use it |
|-------|----------------|
| **Install APEX-OS** | Normal. Start here. |
| **Safe graphics** | The screen goes black after the menu. Uses the firmware framebuffer instead of a GPU driver — the graphical installer still works. |
| **Troubleshoot** | Drops to a dracut shell if the live image cannot be found. |

### 4. Install

Six screens: **network → disk → whole disk or one partition → your account →
Secure Boot → confirm.**

- **Network** is optional but worth doing: the connection is copied into the
  installed system, so first boot already has Wi-Fi. It is *required* for the
  netinstall ISO, which downloads the OS.
- **Disk** lets you take the whole disk, or install into a single partition
  alongside an existing OS. The stick you booted from is never offered.
- **Secure Boot** appears on UEFI machines when the build ships a signed
  kernel. APEX uses a custom kernel that Microsoft does not sign, so its own
  key must be enrolled once — the installer queues this and a blue "MOK
  management" screen appears on the next boot. Skippable.
- **Confirm** lists every partition and whether it is ERASED, KEPT, or SHARED,
  and requires you to type `ERASE`.

Then reboot, remove the stick, and log in as the account you created.

### If something goes wrong

The installer writes `/var/log/apex-install.log` and
`/var/log/apex-installer-launch.log`. If the graphical installer cannot start it
prints a diagnostic on the console and leaves you at a root shell rather than a
black screen — photograph it and open an issue. `Ctrl+Alt+F2` gives a login
(`root` / `apex`) in the live environment.

### Building the ISOs yourself

Needs podman, ~90 GB free, and about 40 minutes.

```sh
# export the OS image the ISO will carry (offline ISO only)
sudo skopeo copy containers-storage:localhost/apex-os:daily \
  oci-archive:/var/tmp/apex-iso/apex.oci:apex-os-daily

cd installer
# small ISO that downloads the OS at install time
NETINSTALL=1 EDITION=daily WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-daily-netinstall.iso sudo -E bash build-live-iso.sh

# fat offline ISO
EDITION=daily WORK=/var/tmp/apex-iso \
  OUT=/var/tmp/apex-iso/apex-daily.iso sudo -E bash build-live-iso.sh
```

## Updating

APEX-OS is image-based, so updates replace the whole OS atomically and can be
rolled back:

```sh
sudo bootc upgrade      # fetch and stage the newest image
sudo systemctl reboot   # boot into it
sudo bootc rollback     # go back to the previous image if anything broke
```

Images are published to `ghcr.io/andrenijman/apex-os` and are public.

## Repository layout

| Path | Contents |
|------|----------|
| `Containerfile.base` | Shared base image (bootc) |
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

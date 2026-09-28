# Rime OS M2 notes (shell provisioner + edition branding + greeter finalization)

M2 wires the public **Rime Shell** (`github.com/AndreNijman/rime-shell`) into
the image as a per-user first-login clone, drives the boot-splash branding from
the edition stamp, and finalizes the rime-greet display manager's session
picker.

Everything M2 adds lives in the **flavor** Containerfiles (`Containerfile.daily`
/ `Containerfile.gaming`). M2 leaves the shared base untouched on purpose, so
the base owner can fold in the package additions listed at the bottom without
conflict. The provisioner + session-curation blocks are byte-identical across
both flavors; only the Plymouth theme differs per edition.

## Files added / changed

| Path | What |
|------|------|
| `files/system/units/rime-shell-firstrun.service` | **New.** Per-user systemd USER unit; runs the provisioner once per user (run-once gate + retry-on-failure). |
| `files/system/libexec/rime-shell-firstrun` | **New.** The provisioner script (installed to `/usr/libexec/`). Clones Rime Shell + seeds per-user config. |
| `files/desktop/wayland-sessions/niri.desktop` | **New.** niri wayland-session for the greeter picker. |
| `Containerfile.daily` | **Edited.** Adds Plymouth (chartreuse) + provisioner wiring + session curation. |
| `Containerfile.gaming` | **Edited.** Adds Plymouth (gold) + provisioner wiring + session curation (after the GPU stage). |
| `docs/m2-notes.md` | **New.** This file. |

## 1. First-boot provisioner

### Why a per-user clone (not /etc/skel)

Rime Shell is a **live git checkout** the user updates in place: its own
auto-updater pulls `~/.local/src/rime-shell`, and Quickshell hot-reloads from
there. An `/etc/skel` copy would be a detached, non-git snapshot the updater and
hot-reload could not drive. Every user therefore gets their own clone at first
login.

### Why it replicates the shell's install.sh instead of running it

The public repo's `install.sh` (+ `dots-extra/install-arch.sh`, inspected during
this work) **cannot** run on Rime OS:

- it `die`s at once on any non-Arch distro (Rime OS is `fedora-bootc:43`);
- step 4 runs `sudo pacman`/AUR installs, which an unprivileged user service
  cannot do and which are pointless because the image already carries every
  dependency;
- it requires a **pre-existing** `~/.config/hypr` config (else it `die`s).

`/usr/libexec/rime-shell-firstrun` instead reproduces the installer's per-user
*seeding* steps (not package installation), and keeps them close to the
original so the shell's updater + hot-reload keep working:

1. **Wait for network**: polls `git ls-remote` for up to ~30 s (first login can
   beat NetworkManager online). Still offline → exit non-zero, marker NOT written.
2. **Clone** `https://github.com/AndreNijman/rime-shell` shallow (`--depth 1`)
   into `~/.local/src/rime-shell` (or `fetch`/`reset --hard` an existing clone;
   scrubs a partial dir from a failed prior run).
3. **Render matugen**: `sed`s `@SRCDIR@`/`@HOME@` in the repo's
   `src/config/matugen.toml.in` into `~/.config/rime-shell/matugen.toml`
   (matugen needs absolute paths; this is install.sh step 5).
4. **Seed config dirs/defaults** (install-arch.sh step 6):
   `~/.config/rime-shell/src/user_data/{config_Provider.json=conf, keybinds.json={}}`,
   `~/.config/hypr/shaders`, `~/.config/matugen/templates`,
   `~/.cache/rime-shell/colors.json`, `hypridle.conf`, and
   `~/Pictures/Wallpapers` (shell's shipped wallpapers + the system Rime
   wallpaper).
5. **Hyprland autostart** (install-arch.sh step 5, self-seeding): if no
   `~/.config/hypr/hyprland.conf` exists it seeds one (distro default if present,
   else a minimal usable base), then appends the Rime Shell `exec-once` block
   (guarded by a marker so re-runs never duplicate it).
6. **niri autostart** (Rime OS addition: the shell's installer covers only
   Hyprland, but the shell auto-detects niri and the greeter offers it): if niri
   is installed, seed `~/.config/niri/config.kdl` with `spawn-at-startup` for the
   shell.
7. **Marker**: `touch ~/.config/rime-shell/.provisioned` **only on full
   success** (`set -e`). Its presence makes systemd skip the unit on every
   future login.

### Idempotency & failure tolerance

- The unit's `ConditionPathExists=!%h/.config/rime-shell/.provisioned` skips the
  whole unit once provisioned.
- The script no-ops if the marker exists, and a guard protects every seeding
  step (`-n` / marker greps / `|| true` on non-critical copies).
- A failure (no network, clone error, timeout) leaves the marker **absent**, so
  the next login retries. `Restart=no` + `TimeoutStartSec=300` guarantee a
  failed run can never wedge the session.

### Enablement

`systemctl --global enable rime-shell-firstrun.service` at image-build time
(in the flavor Containerfiles) symlinks the unit at
`/usr/lib/systemd/user/rime-shell-firstrun.service` into every user's
`default.target.wants`. The unit uses `WantedBy=default.target` (not
`graphical-session.target`) because the user manager always reaches
default.target when it starts at login, while a bare Hyprland/greetd session
with no uwsm handoff does not reach graphical-session.target.

### Known first-login timing caveat

The user manager starts our unit in parallel with the greetd-exec'd compositor,
and nothing orders the two. On a user's **first** login the compositor can start
before seeding finishes, so the shell may not autostart in that session; it runs
from the next login (or at once if seeding won the race). Later logins are
unaffected (marker present, config already seeded). We documented this instead
of fixing it because tightening the ordering would mean coupling to a
compositor-specific systemd handoff the base does not yet use.

## 2. Branding flow: VARIANT_ID → Plymouth / greeter / shell

The flavor image stamps the edition once, and everything downstream reads it:

```
Containerfile.daily   → /usr/lib/os-release: VARIANT="Daily"  VARIANT_ID=daily
Containerfile.gaming  → /usr/lib/os-release: VARIANT="Gaming" VARIANT_ID=gaming
        │
        ├─ rime-greet (greeter)  GreetContext.qml reads /etc/rime-greet/edition
        │     override → else /etc/os-release VARIANT_ID → else "mono".
        │     daily → chartreuse spark + #d9f99d accent
        │     gaming → gold spark + #fde047 accent      ← VERIFIED, no change
        │
        ├─ Plymouth (boot splash)  set per-flavor in the Containerfile:
        │     daily  → rime-os-chartreuse
        │     gaming → rime-os-gold
        │
        └─ Wallpaper  /usr/share/backgrounds/rime/default.jpg (base installs it)
              greeter → hardcoded to that path (GreetSurface.qml) ← VERIFIED
              shell   → provisioner copies it into ~/Pictures/Wallpapers
```

**rime-greet edition resolution: verified, no change needed.** `GreetContext.qml`
resolves the edition from `/etc/rime-greet/edition` → `/etc/os-release`
`VARIANT_ID` → `mono`. The flavor stamps (`VARIANT_ID=daily` / `=gaming`) are
the same strings the greeter special-cases, so the greeter picks the
chartreuse/gold spark + accent with no extra configuration.

**Plymouth.** Each flavor:
1. `COPY files/branding/plymouth/rime-os-<color> /usr/share/plymouth/themes/…`
2. `dnf5 install plymouth plymouth-scripts plymouth-plugin-script` (own layer).
   The theme needs `plymouth-plugin-script` because its `.plymouth` declares
   `ModuleName=script`.
3. `plymouth-set-default-theme rime-os-<color>` (sets the default; **no** `-R`).
4. **Rebuild the initramfs ourselves**, the bootc-correct step. bootc boots
   from `/usr/lib/modules/<kver>/initramfs.img`, not a host-side `/boot`
   initrd, so `-R`'s host dracut path is wrong here. We run
   `dracut --force --no-hostonly --reproducible --zstd --add plymouth --kver
   <kver>` (kver from `/usr/lib/rime-cachyos-kver`, which the base stamps),
   mirroring the base's Stage-1 flags, which bakes the theme into the shipped
   initramfs. In gaming this step sits **after** the NVIDIA akmod stage so it is
   the last initramfs regeneration.
5. Kernel args: `/usr/lib/bootc/kargs.d/20-rime-plymouth.toml` → `quiet splash`
   (per `files/branding/plymouth/README.md`). **Tradeoff:** `quiet` raises the
   console loglevel, trimming the verbose serial output the base's
   `10-rime-serial.toml` enabled for CI/QEMU observability (warnings/errors still
   print). Drop `quiet` if you want noisier boot logs; bootc merges all
   `kargs.d` files.

## 3. rime-greet DM finalization (session picker)

`GreetContext.qml` enumerates `/usr/share/wayland-sessions/*.desktop` (parses
`Name` + `Exec`) into the greeter's `‹ Session ›` picker. The task: offer
**Hyprland + niri only**, never the greeter's own host compositor.

- **greetd as DM + graphical.target default**: the base does this
  (`systemctl enable greetd.service` + `set-default graphical.target`); verified,
  no change.
- **sway / labwc removed as user sessions.** The base installs `sway` + `labwc`
  as the greeter's *host* compositor (see `sway-greet.conf`); both packages also
  ship a `/usr/share/wayland-sessions/*.desktop`, which would show up as bogus
  logins. The flavors `rm -f` `sway.desktop` + `labwc.desktop`. (Removing the
  session files does not touch the sway binary the greeter host uses.)
  > **SUPERSEDED for labwc.** The greeter now offers labwc as a first-class user
  > session via `rime-labwc.desktop`, with its own Rime config seeded per user.
  > The *stock* `labwc.desktop` still gets removed for the reason above (it
  > launches labwc bare, with no Rime Shell), so both statements hold: the stock
  > entry stays deleted, and a separate Rime entry goes in after it.
- **Hyprland session**: the `hyprland` package ships its own
  `hyprland.desktop`; the flavors keep it and, as a defence, write a minimal one
  if it is missing.
- **niri session**: the flavors copy in `files/desktop/wayland-sessions/niri.desktop`
  (`Exec=niri --session`). **The base package list must add niri** (below) for
  this session to launch; without it the greeter offers the session but it
  fails to start.

## 4. Base-image package additions the base owner must fold into `Containerfile.base`

M2 does **not** edit the base. M2 and the shell need these packages to
function, and the base owner must add them to `Containerfile.base` (each heavy
transaction in its own `RUN … && dnf5 clean all` layer, per the base's layering
discipline):

### Required for the M2 deliverables

| Package (Fedora) | Why | Source |
|---|---|---|
| `git-core` | The provisioner clones/fetches Rime Shell at first login. `git-core` is sufficient (no need for the full `git` metapackage). | Fedora |
| `niri` | The greeter offers a niri session; the binary must exist for it to launch (and for the provisioner to seed niri autostart). Fedora's `niri` also ships its own `/usr/share/wayland-sessions/niri.desktop` (ours overrides it). | Fedora |

For now each flavor installs `plymouth` / `plymouth-scripts` /
`plymouth-plugin-script` itself (**per-flavor**), so the branch builds
standalone. The base owner could hoist them into the base to dedupe (both
editions install them). If you hoist them, keep the per-flavor
`plymouth-set-default-theme` + initramfs rebuild in the flavors, since the theme
differs per edition and the initramfs regeneration has to follow the theme
change.

### Required for Rime Shell to run

These come from the shell's `flake.nix` + `dots-extra/install-arch.sh`. The
base's desktop stack has `hyprland`, `quickshell`, `qt6*`, `foot`,
`xwayland-satellite`, `pipewire`/`wireplumber` via deps, and fonts, but
otherwise lacks the shell's runtime.

| Need | Fedora package | Notes |
|---|---|---|
| **Matugen** (REQUIRED, theming) | none | **Not in Fedora repos.** Needs a COPR or a built RPM; the shell "will not function correctly without it." The provisioner renders `matugen.toml` but the `matugen` binary must be present. |
| **Wallpaper daemon** | none | The shell autostarts **`awww-daemon`**. `awww` is not in Fedora (it is a fork of `swww`); either package `awww`, or install `swww` and provide an `awww-daemon` shim, or patch the shell's autostart. **Flag for a decision.** |
| Qt theming | `qt6ct` | |
| Media / MPRIS | `playerctl`, `mpv-mpris`, `mpd-mpris` | `mpd-mpris` may need COPR. |
| Backlight | `brightnessctl` | |
| Clipboard / input | `wl-clipboard`, `slurp`, `wtype`, `cliphist` | |
| XDG | `xdg-user-dirs`, `xdg-desktop-portal-hyprland` | **Checked, and the guess was wrong: see below.** The image installs `xdg-user-dirs` explicitly. |
| Power / sensors | `upower`, `libnotify`, `lm_sensors`, `rfkill` | |
| Visualizer | `cava` | |
| Screen record | `wf-recorder` | |
| Images | `ImageMagick` | |
| Hyprland ecosystem | `hyprlock`, `hypridle`, `hyprsunset`, `hyprland-polkit-agent` (a.k.a. `hyprpolkitagent`) | from `solopasha/hyprland` COPR (already enabled during base build). |
| Power menu / screenshot | `hyprshutdown`, `grimblast` | AUR-only upstream; need a Fedora build or a substitute (the shell calls these). |
| Bluetooth | `bluez`, `bluez-tools` | `bluez` likely present. |
| Laptop/GPU (optional, daily) | `envycontrol`, `auto-cpufreq`, `nbfc-linux` | COPR/pip; optional: the shell degrades without them. |

The base already installs the Nerd Font the shell wants
(`JetBrainsMono Nerd Font`) in Stage 3.

#### hyprland does NOT pull in `xdg-desktop-portal-hyprland`

The XDG row above used to say "portal likely already pulled by hyprland", and
nobody ever checked it, so a guess was the only thing behind Hyprland's portal
support. Measured 2026-09-04, against the package the image itself uses:

```
$ rpm -q hyprland                       # sdegler/hyprland COPR, the one
hyprland-0.56.2-1.fc43.x86_64           # Containerfile.core enables

$ rpm -q --requires   hyprland | grep -i portal    # nothing
$ rpm -q --recommends hyprland | grep -i portal    # nothing
$ rpm -q --whatrequires xdg-desktop-portal-hyprland
no package requires xdg-desktop-portal-hyprland
```

No Requires, no Recommends, and nothing else in the transaction depends on it,
so `install_weak_deps` does not change the answer either. `Containerfile.core`'s
desktop-stack install list does not name it, so the image does not install it
any other way. The portals the image DOES get come from elsewhere: `labwc`
Requires `xdg-desktop-portal-wlr`, and `Containerfile.base` asserts that
`wlr.portal` and `gtk.portal` exist at build time. It makes no assertion about
`hyprland.portal`, which is why nobody noticed the absence.

Two caveats limit this. The measurement used the version installed on the
developer's machine from that COPR, not a fresh image build, so a future package
could add the dependency. And nobody tested here what a Hyprland session's
screen capture falls back to without that backend; the claim is only that
nothing pulls the package in. Adding it is a separate decision, and this note
does not make it.

## 5. Testing in a VM (best-effort, not run here)

M2 made no live-desktop changes on this host and built no image (this
environment has no rootful podman / KVM). The intended verification, following
the M1 recipe (`docs/m1-notes.md`):

```sh
# 1. Build base, then a flavor (rootful; kernel %posttrans + akmod need device access)
sudo podman build --isolation=chroot -f Containerfile.base -t rime-os-base:latest .
sudo podman build --isolation=chroot \
  --build-arg BASE=localhost/rime-os-base:latest \
  -f Containerfile.daily -t rime-os:daily .
#   (gaming: -f Containerfile.gaming --build-arg GPU=mesa|nvidia)
#   NOTE: builds require the base package additions in §4 (git-core, niri, …).

# 2. Produce a bootable qcow2 (needs the flavor image locally)
mkdir -p output
sudo podman run --rm --privileged \
  --security-opt label=type:unconfined_t \
  -v "$(pwd)/output":/output \
  -v /var/lib/containers/storage:/var/lib/containers/storage \
  quay.io/centos-bootc/bootc-image-builder:latest \
  --type qcow2 --rootfs xfs --local rime-os:daily

# 3. Boot it (KVM host)
qemu-system-x86_64 -m 4096 -smp 4 -enable-kvm \
  -bios /usr/share/OVMF/OVMF_CODE.fd \
  -drive file=output/qcow2/disk.qcow2,if=virtio \
  -serial mon:stdio
```

Checks to make in the VM:

- **Plymouth:** the correct spark (chartreuse=daily / gold=gaming) shows during
  boot. `plymouth-set-default-theme --list` inside the image should list
  `rime-os-<color>`; `lsinitrd /usr/lib/modules/<kver>/initramfs.img | grep -i
  plymouth` should show the theme + script plugin baked in.
- **Greeter:** rime-greet paints with the edition spark/accent; the `‹ Session ›`
  picker offers **Hyprland + niri** and **not** sway/labwc.
  > **SUPERSEDED.** The picker now also offers **labwc (Rime)**. sway remains
  > greeter-host only.
  (The base's open item, live layer-shell render under sway on real GL, is
  unchanged and still a HW-verify item; see `docs/m1-notes.md`.)
- **Provisioner:** on first login the provisioner clones `~/.local/src/rime-shell`,
  and `~/.config/rime-shell/{matugen.toml,.provisioned}` + `~/.config/hypr/
  hyprland.conf` (with the Rime autostart block) exist; `systemctl --user status
  rime-shell-firstrun` shows a clean oneshot. Log out and in once if the shell
  did not autostart in the first session (timing caveat above). Re-login does
  no work (Condition gate).
- **SELinux (carried from rime-greet README):** if `last-user`/`last-session`
  prefill is missing, check `ausearch -m avc -ts recent` for a denied write to
  `/var/lib/rime-greet` by the `greetd` domain.

### Static validation done on this branch (no image/systemd/GL available here)

- `bash -n files/system/libexec/rime-shell-firstrun`: clean.
- `rime-shell-firstrun.service` parsed as INI; `%h` specifier + all
  `[Unit]/[Service]/[Install]` keys intact. (`systemd-analyze verify` was not
  available on this host; run it in the Fedora build container.)
- Cloned the public `rime-shell` shallow and read its `install.sh` +
  `dots-extra/install-arch.sh` end to end, to model the provisioner's seeding on
  them.
- Cross-checked the Plymouth theme dirs, wallpaper, and greeter QML paths
  against the base's `COPY` targets and `GreetContext`/`GreetSurface` QML.

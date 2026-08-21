# APEX-OS — M2 notes (shell provisioner + edition branding + greeter finalization)

M2 wires the public **APEX Shell** (`github.com/AndreNijman/apex-shell`) into
the image as a per-user first-login clone, drives the boot-splash branding from
the edition stamp, and finalizes the apex-greet display manager's session
picker.

Everything M2 adds lives in the **flavor** Containerfiles (`Containerfile.daily`
/ `Containerfile.gaming`) — the shared base is intentionally left untouched so
the base owner can fold in the package additions listed at the bottom without
conflict. The provisioner + session-curation blocks are byte-identical across
both flavors; only the Plymouth theme differs per edition.

## Files added / changed

| Path | What |
|------|------|
| `files/system/units/apex-shell-firstrun.service` | **New.** Per-user systemd USER unit; runs the provisioner once per user (run-once gate + retry-on-failure). |
| `files/system/libexec/apex-shell-firstrun` | **New.** The provisioner script (installed to `/usr/libexec/`). Clones APEX Shell + seeds per-user config. |
| `files/desktop/wayland-sessions/niri.desktop` | **New.** niri wayland-session for the greeter picker. |
| `Containerfile.daily` | **Edited.** Adds Plymouth (chartreuse) + provisioner wiring + session curation. |
| `Containerfile.gaming` | **Edited.** Adds Plymouth (gold) + provisioner wiring + session curation (after the GPU stage). |
| `docs/m2-notes.md` | **New.** This file. |

## 1. First-boot provisioner

### Why a per-user clone (not /etc/skel)

APEX Shell is a **live git checkout** the user updates in place: its own
auto-updater pulls `~/.local/src/apex-shell`, and Quickshell hot-reloads from
there. An `/etc/skel` copy would be a detached, non-git snapshot the updater and
hot-reload could not drive. So every user gets their own clone the first time
they log in.

### Why it replicates the shell's install.sh instead of running it

The public repo's `install.sh` (+ `dots-extra/install-arch.sh`, inspected during
this work) **cannot** run on APEX-OS:

- it `die`s immediately on any non-Arch distro (APEX-OS is `fedora-bootc:43`);
- step 4 runs `sudo pacman`/AUR installs — impossible from an unprivileged user
  service, and pointless because every dependency is baked into the image;
- it requires a **pre-existing** `~/.config/hypr` config (else it `die`s).

So `/usr/libexec/apex-shell-firstrun` reproduces the parts of the installer that
are per-user *seeding* (not package installation), keeping them faithful so the
shell's updater + hot-reload keep working:

1. **Wait for network** — polls `git ls-remote` up to ~30 s (first login can beat
   NetworkManager online). Still offline → exit non-zero, marker NOT written.
2. **Clone** `https://github.com/AndreNijman/apex-shell` shallow (`--depth 1`)
   into `~/.local/src/apex-shell` (or `fetch`/`reset --hard` an existing clone;
   scrubs a partial dir from a failed prior run).
3. **Render matugen** — `sed`s `@SRCDIR@`/`@HOME@` in the repo's
   `src/config/matugen.toml.in` into `~/.config/apex-shell/matugen.toml`
   (matugen needs absolute paths; this is install.sh step 5).
4. **Seed config dirs/defaults** (install-arch.sh step 6):
   `~/.config/apex-shell/src/user_data/{config_Provider.json=conf, keybinds.json={}}`,
   `~/.config/hypr/shaders`, `~/.config/matugen/templates`,
   `~/.cache/apex-shell/colors.json`, `hypridle.conf`, and
   `~/Pictures/Wallpapers` (shell's shipped wallpapers + the system APEX
   wallpaper).
5. **Hyprland autostart** (install-arch.sh step 5, self-seeding): if no
   `~/.config/hypr/hyprland.conf` exists it seeds one (distro default if present,
   else a minimal usable base), then appends the APEX Shell `exec-once` block
   (guarded by a marker so re-runs never duplicate it).
6. **niri autostart** (APEX-OS addition — the shell's installer only covers
   Hyprland, but the shell auto-detects niri and the greeter offers it): if niri
   is installed, seed `~/.config/niri/config.kdl` with `spawn-at-startup` for the
   shell.
7. **Marker** — `touch ~/.config/apex-shell/.provisioned` **only on full
   success** (`set -e`). Its presence makes systemd skip the unit on every
   future login.

### Idempotency & failure tolerance

- The unit's `ConditionPathExists=!%h/.config/apex-shell/.provisioned` skips the
  whole unit once provisioned.
- The script no-ops if the marker exists, and every seeding step is guarded
  (`-n` / marker greps / `|| true` on non-critical copies).
- A failure (no network, clone error, timeout) leaves the marker **absent**, so
  the next login retries. `Restart=no` + `TimeoutStartSec=300` guarantee a
  failed run can never wedge the session.

### Enablement

`systemctl --global enable apex-shell-firstrun.service` at image-build time
(in the flavor Containerfiles) symlinks the unit at
`/usr/lib/systemd/user/apex-shell-firstrun.service` into every user's
`default.target.wants`. `WantedBy=default.target` (not `graphical-session.target`)
because default.target is always reached when the user manager starts at login,
whereas graphical-session.target is not reached under a bare Hyprland/greetd
session with no uwsm handoff.

### Known first-login timing caveat

The user manager starts our unit in parallel with the greetd-exec'd compositor;
there is no ordering guarantee between them. On the **very first** login the
compositor can start before seeding finishes, so the shell may not autostart that
first session — it is live from the next login (or immediately if seeding won
the race). Subsequent logins are unaffected (marker present, config already
seeded). This is documented rather than fixed because tightening the ordering
would require coupling to a compositor-specific systemd handoff the base does not
yet use.

## 2. Branding flow: VARIANT_ID → Plymouth / greeter / shell

The edition is stamped once, in the flavor image, and everything downstream
reads it:

```
Containerfile.daily   → /usr/lib/os-release: VARIANT="Daily"  VARIANT_ID=daily
Containerfile.gaming  → /usr/lib/os-release: VARIANT="Gaming" VARIANT_ID=gaming
        │
        ├─ apex-greet (greeter)  GreetContext.qml reads /etc/apex-greet/edition
        │     override → else /etc/os-release VARIANT_ID → else "mono".
        │     daily → chartreuse spark + #d9f99d accent
        │     gaming → gold spark + #fde047 accent      ← VERIFIED, no change
        │
        ├─ Plymouth (boot splash)  set per-flavor in the Containerfile:
        │     daily  → apex-os-chartreuse
        │     gaming → apex-os-gold
        │
        └─ Wallpaper  /usr/share/backgrounds/apex/default.jpg (base installs it)
              greeter → hardcoded to that path (GreetSurface.qml) ← VERIFIED
              shell   → provisioner copies it into ~/Pictures/Wallpapers
```

**apex-greet edition resolution — verified, no change needed.** `GreetContext.qml`
resolves the edition from `/etc/apex-greet/edition` → `/etc/os-release`
`VARIANT_ID` → `mono`. The flavor stamps (`VARIANT_ID=daily` / `=gaming`) match
exactly the strings the greeter special-cases, so the chartreuse/gold spark +
accent are selected automatically.

**Plymouth.** Each flavor:
1. `COPY files/branding/plymouth/apex-os-<color> /usr/share/plymouth/themes/…`
2. `dnf5 install plymouth plymouth-scripts plymouth-plugin-script` (own layer).
   `plymouth-plugin-script` is required because the theme's `.plymouth` declares
   `ModuleName=script`.
3. `plymouth-set-default-theme apex-os-<color>` (sets the default; **no** `-R`).
4. **Rebuild the initramfs ourselves** — the bootc-correct step. On bootc the
   initrd bootc boots from is `/usr/lib/modules/<kver>/initramfs.img`, not a
   host-side `/boot` initrd, so `-R`'s host dracut path is wrong here. We run
   `dracut --force --no-hostonly --reproducible --zstd --add plymouth --kver
   <kver>` (kver from `/usr/lib/apex-cachyos-kver`, stamped by the base),
   mirroring the base's Stage-1 flags, so the theme is baked into the shipped
   initramfs. In gaming this is placed **after** the NVIDIA akmod stage so it is
   the last initramfs regeneration.
5. Kernel args: `/usr/lib/bootc/kargs.d/20-apex-plymouth.toml` → `quiet splash`
   (per `files/branding/plymouth/README.md`). **Tradeoff:** `quiet` raises the
   console loglevel, trimming the verbose serial output the base's
   `10-apex-serial.toml` enabled for CI/QEMU observability (warnings/errors still
   print). Drop `quiet` if noisier boot logs are wanted; bootc merges all
   `kargs.d` files.

## 3. apex-greet DM finalization (session picker)

`GreetContext.qml` enumerates `/usr/share/wayland-sessions/*.desktop` (parses
`Name` + `Exec`) into the greeter's `‹ Session ›` picker. The task: offer
**Hyprland + niri only**, never the greeter's own host compositor.

- **greetd as DM + graphical.target default** — done by the base
  (`systemctl enable greetd.service` + `set-default graphical.target`); verified,
  no change.
- **sway / labwc removed as user sessions.** The base installs `sway` + `labwc`
  as the greeter's *host* compositor (see `sway-greet.conf`); both packages also
  ship a `/usr/share/wayland-sessions/*.desktop`, which would wrongly appear as
  logins. The flavors `rm -f` `sway.desktop` + `labwc.desktop`. (Removing the
  session files does not touch the sway binary the greeter host uses.)
  > **SUPERSEDED for labwc.** labwc is now offered as a first-class user session
  > via `apex-labwc.desktop`, with its own APEX config seeded per-user. The
  > *stock* `labwc.desktop` is still removed for exactly the reason above — it
  > launches labwc bare, with no APEX Shell — so both statements hold: the stock
  > entry stays deleted, and a separate APEX entry is added after it.
- **Hyprland session** — the `hyprland` package ships its own
  `hyprland.desktop`; the flavors keep it and, defensively, write a minimal one
  if it is somehow absent.
- **niri session** — `files/desktop/wayland-sessions/niri.desktop`
  (`Exec=niri --session`) is copied in. **niri must be added to the base package
  list** (below) for this session to actually launch; without it the session is
  offered but fails to start.

## 4. Base-image package additions the base owner must fold into `Containerfile.base`

M2 does **not** edit the base. These packages are needed for M2 + the shell to
function and must be added to `Containerfile.base` (each heavy transaction in its
own `RUN … && dnf5 clean all` layer, per the base's layering discipline):

### Required for the M2 deliverables

| Package (Fedora) | Why | Source |
|---|---|---|
| `git-core` | The provisioner clones/fetches APEX Shell at first login. `git-core` is sufficient (no need for the full `git` metapackage). | Fedora |
| `niri` | The greeter offers a niri session; the binary must exist for it to launch (and for the provisioner to seed niri autostart). Fedora's `niri` also ships its own `/usr/share/wayland-sessions/niri.desktop` (ours overrides it). | Fedora |

`plymouth` / `plymouth-scripts` / `plymouth-plugin-script` are currently
installed **per-flavor** so the branch builds standalone. They could be hoisted
into the base to dedupe (both editions install them); if hoisted, keep the
per-flavor `plymouth-set-default-theme` + initramfs rebuild in the flavors, since
the theme differs per edition and the initramfs must be regenerated after the
theme is set.

### Required for APEX Shell to actually run (from the shell's `flake.nix` +
`dots-extra/install-arch.sh` — the base's desktop stack has `hyprland`,
`quickshell`, `qt6*`, `foot`, `xwayland-satellite`, `pipewire`/`wireplumber` via
deps, and fonts, but is otherwise missing the shell's runtime)

| Need | Fedora package | Notes |
|---|---|---|
| **Matugen** (REQUIRED — theming) | — | **Not in Fedora repos.** Needs a COPR or a built RPM; the shell "will not function correctly without it." The provisioner renders `matugen.toml` but the `matugen` binary must be present. |
| **Wallpaper daemon** | — | The shell autostarts **`awww-daemon`**. `awww` is not in Fedora (it is a fork of `swww`); either package `awww`, or install `swww` and provide an `awww-daemon` shim, or patch the shell's autostart. **Flag for a decision.** |
| Qt theming | `qt6ct` | |
| Media / MPRIS | `playerctl`, `mpv-mpris`, `mpd-mpris` | `mpd-mpris` may need COPR. |
| Backlight | `brightnessctl` | |
| Clipboard / input | `wl-clipboard`, `slurp`, `wtype`, `cliphist` | |
| XDG | `xdg-user-dirs`, `xdg-desktop-portal-hyprland` | portal likely already pulled by hyprland. |
| Power / sensors | `upower`, `libnotify`, `lm_sensors`, `rfkill` | |
| Visualizer | `cava` | |
| Screen record | `wf-recorder` | |
| Images | `ImageMagick` | |
| Hyprland ecosystem | `hyprlock`, `hypridle`, `hyprsunset`, `hyprland-polkit-agent` (a.k.a. `hyprpolkitagent`) | from `solopasha/hyprland` COPR (already enabled during base build). |
| Power menu / screenshot | `hyprshutdown`, `grimblast` | AUR-only upstream; need a Fedora build or a substitute (the shell calls these). |
| Bluetooth | `bluez`, `bluez-tools` | `bluez` likely present. |
| Laptop/GPU (optional, daily) | `envycontrol`, `auto-cpufreq`, `nbfc-linux` | COPR/pip; optional — the shell degrades without them. |

The Nerd Font the shell wants (`JetBrainsMono Nerd Font`) is already installed by
the base (Stage 3).

## 5. Testing in a VM (best-effort — not run here)

No live-desktop changes were made on this host, and no image was built (this
environment has no rootful podman / KVM). The intended verification, following
the M1 recipe (`docs/m1-notes.md`):

```sh
# 1. Build base, then a flavor (rootful; kernel %posttrans + akmod need device access)
sudo podman build --isolation=chroot -f Containerfile.base -t apex-os-base:latest .
sudo podman build --isolation=chroot \
  --build-arg BASE=localhost/apex-os-base:latest \
  -f Containerfile.daily -t apex-os:daily .
#   (gaming: -f Containerfile.gaming --build-arg GPU=mesa|nvidia)
#   NOTE: builds require the base package additions in §4 (git-core, niri, …).

# 2. Produce a bootable qcow2 (needs the flavor image locally)
mkdir -p output
sudo podman run --rm --privileged \
  --security-opt label=type:unconfined_t \
  -v "$(pwd)/output":/output \
  -v /var/lib/containers/storage:/var/lib/containers/storage \
  quay.io/centos-bootc/bootc-image-builder:latest \
  --type qcow2 --rootfs xfs --local apex-os:daily

# 3. Boot it (KVM host)
qemu-system-x86_64 -m 4096 -smp 4 -enable-kvm \
  -bios /usr/share/OVMF/OVMF_CODE.fd \
  -drive file=output/qcow2/disk.qcow2,if=virtio \
  -serial mon:stdio
```

What to check in the VM:

- **Plymouth:** the correct spark (chartreuse=daily / gold=gaming) shows during
  boot. `plymouth-set-default-theme --list` inside the image should list
  `apex-os-<color>`; `lsinitrd /usr/lib/modules/<kver>/initramfs.img | grep -i
  plymouth` should show the theme + script plugin baked in.
- **Greeter:** apex-greet paints with the edition spark/accent; the `‹ Session ›`
  picker offers **Hyprland + niri** and **not** sway/labwc.
  > **SUPERSEDED.** The picker now also offers **labwc (APEX)**. sway remains
  > greeter-host only.
  (The base's open item — live layer-shell render under sway on real GL — is
  unchanged and still a HW-verify item; see `docs/m1-notes.md`.)
- **Provisioner:** on first login `~/.local/src/apex-shell` is cloned,
  `~/.config/apex-shell/{matugen.toml,.provisioned}` + `~/.config/hypr/
  hyprland.conf` (with the APEX autostart block) exist; `systemctl --user status
  apex-shell-firstrun` shows a clean oneshot. Log out/in once if the shell did
  not autostart on the very first session (timing caveat above). Re-login does
  no work (Condition gate).
- **SELinux (carried from apex-greet README):** if `last-user`/`last-session`
  prefill is missing, check `ausearch -m avc -ts recent` for a denied write to
  `/var/lib/apex-greet` by the `greetd` domain.

### Static validation done on this branch (no image/systemd/GL available here)

- `bash -n files/system/libexec/apex-shell-firstrun` — clean.
- `apex-shell-firstrun.service` parsed as INI; `%h` specifier + all
  `[Unit]/[Service]/[Install]` keys intact. (`systemd-analyze verify` was not
  available on this host — recommend running it in the Fedora build container.)
- The public `apex-shell` `install.sh` + `dots-extra/install-arch.sh` were cloned
  shallow and read end-to-end to model the provisioner's seeding faithfully.
- Plymouth theme dirs, wallpaper, and greeter QML paths cross-checked against the
  base's `COPY` targets and `GreetContext`/`GreetSurface` QML.

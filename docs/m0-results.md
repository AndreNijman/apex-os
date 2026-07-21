# APEX-OS — M0 spike results

Findings from the M0 de-risking spikes. Each spike is a self-contained
section; a spike that fails with a clear diagnosis is still a valid result.

> Note: this file may be authored in parallel by more than one M0 spike
> branch. Spike A owns the "Spike A" section below; other sections merge in
> from their own branches.

## Spike A — base image + greeter

**Goal.** Prove the base-image stack — fedora-bootc + Hyprland (COPR) +
quickshell + greetd/cage + the `apex-greet` greeter — builds, produces a
bootable disk image, boots in a VM, renders the greeter, and completes a
real login through `apex-greet`.

### Result at a glance

| Sub-goal | Result |
|---|---|
| `Containerfile.base` builds | **PASS** (2.88 GB image) |
| qcow2 via bootc-image-builder | **PASS** (1.7 GB qcow2, `--rootfs xfs`) |
| VM boots (UEFI/OVMF, kernel 7.1.4, graphical.target) | **PASS** |
| greetd runs (display-manager alias, session on seat0/tty1) | **PASS** |
| `apex-greet` *launches* (cage+qs, loads shell.qml, greetd IPC) | **PASS** |
| Greeter *visually renders* (clock / pill / spark) | **FAIL in this headless QEMU** — QML bug fixed, but blocked by compositor/layer-shell + headless-GL (see below) |
| Login (`test`/`test`) auth path | **infra PASS** — PAM reachable, serial login `test`/`test` works; full auth-through-the-greeter not visually confirmed (greeter never painted) |

**One-line verdict:** the base stack builds, the qcow2 boots, greetd runs, and
`apex-greet` launches and is parse/logic-correct — but live pixel rendering
could not be confirmed in this headless QEMU setup. Defer visual confirmation
to real hardware or a GL-capable runner, and revisit the greeter's compositor
host (cage 0.2.0 does not serve quickshell's layer-shell).

### Exact refs that worked

- **Base image:** `quay.io/fedora/fedora-bootc:43` (VERSION_ID=43, image
  created 2026-07-21). See the F44 note below for why not 44.
- **Hyprland COPR:** `solopasha/hyprland` → `hyprland 0.51.1-3.fc43`,
  `aquamarine 0.9.5-2.fc43`, `hyprlang`, `hyprutils`, `hyprgraphics`,
  `hyprcursor`.
- **Quickshell COPR:** `errornointernet/quickshell` → `quickshell
  0.3.0-3.fc43` — the exact version `apex-greet` was written against
  (Fedora proper only carries `quickshell 0.2.1`, so the COPR is worth
  keeping even though quickshell is now packaged).
- **From stock Fedora 43:** `greetd 0.10.3`, `cage 0.2.0`, `foot 1.25.0`,
  `xwayland-satellite 0.8.1` (packaged — no COPR needed),
  `jetbrains-mono-fonts 2.304`, `qt6-qtbase/qtdeclarative 6.10.3`,
  `wlroots0.18 0.18.3`, `mesa 25.3.6`.
- **Nerd font:** `JetBrainsMono.tar.xz` from
  `ryanoasis/nerd-fonts` release **v3.4.0**, unpacked to
  `/usr/share/fonts/jetbrains-mono-nerd/` (provides the
  `JetBrainsMono Nerd Font` family the greeter QML uses for both text and
  its person/lock/caps glyphs).

### Base image versions (installed)

- hyprland **0.51.1**, aquamarine **0.9.5**
- quickshell **0.3.0**
- greetd **0.10.3**, cage **0.2.0**
- xwayland-satellite **0.8.1**, foot **1.25.0**
- Qt **6.10.3**

### Build

- `podman build` run **rootful** (`sudo podman`) so the image lands in
  root's containers-storage, where `bootc-image-builder` can read it with
  `--local` (rootless build + rootful bib would miss each other's storage).
- Image size: **2.88 GB** (container image); **1.7 GB** qcow2.
- Build time: dnf transaction ≈ 185 packages / 183 MiB; whole build a few
  minutes (dominated by the dnf install + a concurrent spike build
  contending for CPU on the same host).

### Problems + workarounds

1. **Fedora 44 is currently broken for this COPR stack (base-tag
   decision).** F44 is the newer stable, but `solopasha/hyprland` ships
   `aquamarine-0.9.5` linked against `libdisplay-info.so.2`, while F44
   already moved to `libdisplay-info 0.3` (soname `.so.3`) — so
   `dnf install hyprland` fails to resolve (`nothing provides
   libdisplay-info.so.2`). F43 ships `libdisplay-info 0.2` (`.so.2`) and
   the whole set resolves cleanly (185 pkgs). **Chose fedora-bootc:43**;
   revisit F44 once the COPR rebuilds aquamarine against `.so.3`.
2. **`dnf5 copr` needs a plugin.** The `copr` subcommand is not in the base
   image — `dnf5 install -y dnf5-plugins` must run before
   `dnf5 copr enable`. (The greeter README's example omitted this.)
3. **greetd session user.** Fedora's `greetd` package provisions a
   **`greetd`** system user via sysusers.d (UID/GID 977) — there is **no
   `greeter` user**. The greeter's `greetd-config.toml` used
   `user = "greeter"`, which would stop greetd from starting the session.
   Fixed to `user = "greetd"` and aligned the `/var/lib/apex-greet`
   state-dir ownership to `greetd:greetd`. (Committed as a `fix(greeter)`.)
4. **Greeter had no keyboard path to the username field.** On a fresh boot
   (`/var/lib/apex-greet/last-user` empty) the password field was always
   force-focused and there was no key to reach the username pill, so a
   keyboard-only login could never enter a username. Fixed: focus the
   username field first when nothing is prefilled, and add Tab / Shift+Tab
   between the two pills. (Committed as a `fix(greeter)`; validated with an
   offscreen `qs` parse-check against host quickshell 0.3.0 — clean, only
   the expected "No PanelWindow backend loaded".)
5. **SELinux scriptlet noise during build.** `greetd-selinux`'s
   `semodule` scriptlet logs `libsemanage ... Error while renaming
   /etc/selinux/...; /usr/sbin/semodule: Failed!` inside the container
   build (no live SELinux to commit policy against). Non-fatal — the build
   continues and completes. Worth re-checking the greetd SELinux module is
   actually active on a real (enforcing) install.
6. **Host cgroups broke podman/crun mid-spike.** The host kernel is cgroup
   **v2**, but `/sys/fs/cgroup` was left mounted as plain `sysfs` in this
   namespace (a concurrent agent's teardown unmounted it), so any
   `podman run`/`build` RUN failed with `crun: invalid file system type on
   /sys/fs/cgroup`. Fixes: `sudo mount -t cgroup2 none /sys/fs/cgroup`
   restores it for `podman run` (needed for bib); and
   **`podman build --isolation=chroot`** builds without needing cgroups at
   all (used for the image build). `--cgroups=disabled` does NOT help.
7. **bootc-image-builder specifics.** Needs an explicit rootfs
   (`--rootfs xfs`, else `missing required info: DefaultRootFs`). It writes
   the qcow2 as **root**, so `chown` it before a user-run qemu can open it
   (`Could not open … Permission denied`).

### Greeter debugging (what the VM revealed)

The first boot reached `graphical.target`, `greetd.service` started, and
`loginctl` showed a **greeter session `c1` (uid 977 `greetd`) on
seat0/tty1** with `cage` (pid) + `qs` running — but the screen was solid
black. Diagnosis method: bib `[customizations.kernel] append =
console=ttyS0,115200`, qemu `-serial` as an interactive unix socket, log in
on the serial getty as `test`/`test`, then read
`/run/user/977/quickshell/by-id/*/log.qslog`. That log showed the real
faults:

1. **`theme` / `ctx` undefined in `GreetSurface` (fixed).** `shell.qml`
   bound `theme: theme` / `ctx: ctx`, but `GreetSurface` declares
   `required property var theme`/`ctx`, so each RHS resolved to the
   surface's own unset property (self-reference), not the outer `QtObject`
   / `GreetContext`. Result: dozens of `TypeError: Cannot read property
   'background'/'username'/... of undefined` and nothing painted. Fixed by
   renaming the ids to `greetTheme` / `greetCtx`
   (`fix(greeter): bind GreetSurface theme/ctx to renamed ids, not self`).
2. **`Failed to initialize layershell integration` → `eglSwapBuffers
   failed 0x300d, surface: 0x0` (open risk, NOT a GL cascade).** After fix
   #1 the theme/ctx errors were gone (confirmed — the quickshell log no
   longer prints any `Cannot read property ... of undefined`), **but this
   layershell error persisted** and the screen stayed black. quickshell's
   `PanelWindow` needs `wlr-layer-shell`; under `cage 0.2.0` (F43) that
   integration never initialises, so the surface is never created. cage
   itself was healthy (its processes stayed up and it held the display —
   just painting black), so this is a **layer-shell protocol gap between
   cage 0.2.0 and quickshell 0.3.0**, not a compositor-GL failure.
   (`gtkgreet` works under cage because it falls back to an xdg-toplevel;
   quickshell's `PanelWindow` does not.)

**Render attempts (all headless QEMU on this host — none produced a visible
greeter):**

| Config | Compositor | Outcome | Shot |
|---|---|---|---|
| `virtio-vga`, `-display none` | cage (llvmpipe GL) | cage runs + holds display, **black** — quickshell layershell init fails | `greeter-fixed.png` (432 B, black) |
| `virtio-vga` + `env WLR_RENDERER=pixman …` | cage | cage **crash-loops** (greetd `res=failed`); tty0 console shown | `render-1/2.png` (console) |
| `virtio-vga` + Hyprland host (`LIBGL_ALWAYS_SOFTWARE=1`) | Hyprland | **crashes** headless (kernel `virtio_gpu` trace, greetd exit 1) | `greeter-hypr.png` (crash console) |
| `virtio-vga-gl` + `-display egl-headless` (host AMD GPU/virgl) | cage | cage not up; QMP `screendump` → `no surface` (GL scanout not capturable) | — |

So the black screen is a **combination**: (a) [fixed] the theme/ctx QML bug;
(b) cage 0.2.0 not serving quickshell's layer-shell; and (c) the headless
QEMU display path (software `virtio-vga` gives no usable GL scanout for
`screendump` once GL is involved; `egl-headless` doesn't expose a 2D surface
to `screendump`; forcing pixman crashes cage; Hyprland needs real GL).

**Recommendation (greeter owner / M1):**
- Revisit the greeter's compositor host. cage 0.2.0 does not serve
  quickshell's `PanelWindow`. Options: a newer cage, **sway 1.11** or
  **labwc 0.9.6** (both packaged in F43, both do layer-shell v4), or the
  production **Hyprland** — and confirm on a GL-capable target.
- Do the visual + login verification on **real hardware** or a runner with
  working GPU/virgl (e.g. `-device virtio-vga-gl -display egl-headless` on a
  host whose qemu can present a capturable surface, or a GTK/SDL display).

Also noted, non-fatal: quickshell warns that the two `Connections` blocks
in `GreetSurface.qml` (`onFailed`, `onUsernameChanged`) "no signal of the
target matches the name" — the signals exist on `GreetContext`, so these
are just scanner warnings, but worth confirming the handlers fire.

### Brain_Shell dependency gaps noticed

- **matugen** (Material-You color generation used by Brain_Shell): not
  checked into this image (greeter uses an inlined palette, so it does not
  need matugen). Packaging status to confirm for the full shell — likely a
  COPR or `cargo`/binary drop.
- **awww / wallpaper daemon:** not built here (spike scope explicitly
  excludes building it from source). Package availability to be surveyed
  separately; the greeter uses a static wallpaper via Qt `Image`, so it has
  no runtime wallpaper-daemon dependency.
- The greeter deliberately depends on **nothing** from Brain_Shell at login
  time (palette, clock, wallpaper all inlined), which this spike confirms:
  quickshell 0.3.0 alone renders it.

### Screenshots

Under `/home/andre/apex-os-m0-work/spike-a/shots/` (NOT committed):

- `greeter-fixed.png` — cage, post theme/ctx fix: 432 B solid black (cage up,
  quickshell layershell init failed → no surface painted).
- `render-1.png`, `render-2.png` — `WLR_RENDERER=pixman` attempts: tty0
  console (cage crash-looped under the pixman renderer).
- `greeter-hypr.png` — Hyprland-host attempt: crash console (headless GL).
- (no shot for the `egl-headless` attempt — `screendump` returned
  `no surface`.)

### Verification method

- Build: rootful `podman build --isolation=chroot` (chroot isolation was
  required — see gotcha #2 below); image in root containers-storage.
- Disk: `bootc-image-builder … --type qcow2 --rootfs xfs --local` (rootful,
  privileged, storage bind-mounted); `chown` the root-owned qcow2 to the
  user before qemu.
- Boot: `qemu-system-x86_64 -enable-kvm -m 4G -cpu host -smp 4 -machine q35`,
  OVMF `OVMF_CODE.4m.fd` + a writable `OVMF_VARS.4m.fd` copy, `virtio-vga`,
  QMP unix socket, `-display none`.
- Serial: bib `[customizations.kernel] append = "console=ttyS0,115200
  console=tty0"`; qemu serial as an interactive unix socket
  (`-chardev socket … -serial chardev:ser0`) so the serial getty is
  drivable — logged in `test`/`test` and read `systemctl status greetd`,
  `journalctl -u greetd`, and `/run/user/977/quickshell/by-id/*/log.qslog`
  (the decisive evidence). `/etc` is writable in bootc, so greetd's command
  was re-wrapped live (env / alternate compositor) and greetd restarted
  without rebuilding.
- Graphical capture: QMP `screendump` (PPM) → `magick` → PNG, read back.

**Evidence that `apex-greet` launched (even though it did not paint):**
`loginctl` showed greeter session `c1`, uid 977 `greetd`, seat0, tty1, with
`cage` and `qs` processes; the quickshell log shows `Launching config
"/usr/share/apex-greet/shell.qml"`, successful edition detection, and
`quickshell.service.greetd  Connected to greetd socket` — i.e. the greetd
IPC/PAM conversation is reachable. `greetd`'s own PAM stack authenticated
`test` on the serial getty. So the greeter, its greetd backend, and auth are
all wired correctly; only the on-screen surface is missing.

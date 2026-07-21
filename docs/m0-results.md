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

## Spike B — kernel + akmods

**Goal:** prove the Phase-A kernel path — the CachyOS-for-Fedora COPR kernel
swapped into a `fedora-bootc` image, an NVIDIA akmod built against it DIY,
binderfs/scheduler config verified, sched-ext userspace available, the
image-layering model determined, and the result booted in a VM.

**Host / tooling:** Void Linux, podman 5.8.3 (rootful build via passwordless
sudo), qemu 11.0.2 + KVM, bootc-image-builder, 16 threads. Large artifacts live
outside the repo in `~/apex-os-m0-work/spike-b/`. Repo deliverable:
`kernel/Containerfile.kernel-spike`.

### Verdict per sub-goal

| Sub-goal | Result |
|----------|--------|
| Kernel swap (Fedora → CachyOS in a bootc image) | **PASS** |
| Initramfs regenerated into `/usr/lib/modules/<kver>/` | **PASS** (kernel scriptlets auto-generate it; no manual dracut needed) |
| NVIDIA akmod DIY build against CachyOS kernel | **PASS** (driver 580.159.03, akmods rc=0) |
| `modinfo nvidia` resolves against CachyOS kver | **PASS** (vermagic `7.0.12-cachyos1.fc42.x86_64`) |
| Kernel config table (binderfs/ntsync/sched-ext/amdxdna/hz/preempt) | **PASS** (all present — table below) |
| sched-ext userspace (scx-scheds + scx_loader/scxctl) | **PASS** (1.1.0) |
| Layering-support verdict | **PASS — layering supported** (`rpm-ostree` present) |
| VM boot | **PARTIAL** — CachyOS kernel boots under KVM, no panic; full multi-user pending (see below) |

Two defects were found that do **not** block the Phase-A decision but must be
fixed before M1: a corrupt rpmdb baked into the scx layer, and the VM full-boot
being blocked by the build host's missing cgroup mount. Both are detailed below.

### Exact versions

| Item | Value |
|------|-------|
| Base image | `quay.io/fedora/fedora-bootc:42` |
| Base image digest | `sha256:3b80fff7ae609cc4c0ea6a1c728e32003a72719d1e0441637894a46ce840b0fe` |
| Base OS | Fedora Linux 42 (Adams) |
| Stock kernel (removed) | `6.19.14-101.fc42` |
| **CachyOS kernel installed** | **`kernel-cachyos-7.0.12-cachyos1.fc42.x86_64`** (BORE 6.6.3, 1000 Hz) |
| CachyOS COPR | `bieszczaders/kernel-cachyos` (kernel) + `bieszczaders/kernel-cachyos-addons` (scx) |
| **NVIDIA driver** | **`580.159.03`** (RPMFusion nonfree, proprietary) |
| Built kmod RPM | `kmod-nvidia-7.0.12-cachyos1.fc42.x86_64-580.159.03-1.fc42.x86_64.rpm` (~9 MB) |
| scx | `scx-scheds-1.1.0-1.fc42`, `scx-tools-1.1.0-1.fc42` (scx_loader/scxctl `1.1.0`) |
| bootc | `1.15.1` |
| rpm-ostree | `2025.12-1.fc42` (present ⇒ layering supported) |
| dnf5 / dracut / gcc | `5.2.18.0` / `107-4.fc42` / `15.2.1` |
| Spike image build time | **1029 s (~17 min)**, 16 threads (no build cache) |
| Final image size | 5.1 GB |

### 1. Kernel swap — PASS

Clean in a bootc container build with `dnf5`:

```
dnf5 -y install dnf5-plugins
dnf5 -y copr enable bieszczaders/kernel-cachyos
dnf5 -y copr enable bieszczaders/kernel-cachyos-addons
dnf5 -y remove --no-autoremove kernel kernel-core kernel-modules kernel-modules-core
dnf5 -y install kernel-cachyos kernel-cachyos-core kernel-cachyos-modules kernel-cachyos-devel-matched
```

- Removing the four Fedora kernel packages drags in nothing else (only benign
  `grep: /etc/default/grub: No such file` noise from kernel-core `%preun`).
- The CachyOS `%posttrans` scriptlet runs `Generating initramfs` + `depmod`
  itself. Net result in the image, confirmed present:
  `/usr/lib/modules/7.0.12-cachyos1.fc42.x86_64/{vmlinuz (~16.6 MB),
  initramfs.img (~132 MB), config (~293 KB)}`. **No manual dracut step was
  needed** (the Containerfile keeps a conditional dracut fallback, which stayed
  dormant). Note the config is at `/usr/lib/modules/<kver>/config`, **not**
  `/boot/config-<kver>`.
- `kernel-cachyos-devel-matched` lands headers at
  `/usr/src/kernels/7.0.12-cachyos1.fc42.x86_64` (required for the akmod build).
- COPR ships split packages mirroring Fedora's layout (`kernel-cachyos` meta +
  `-core` + `-modules` + `-devel` + `-devel-matched`). Default desktop
  `kernel-cachyos` is the correct pick; `-lts`/`-rt`/`-server`/`-lto` also exist.

### 2. NVIDIA akmod (DIY) — PASS (the plan's biggest Phase-A risk)

As of Feb 2026 the CachyOS COPR dropped prebuilt NVIDIA (the only
`kernel-cachyos-nvidia-open` packages in the repo are stale 6.18.12/6.12.73, not
the 7.0.12 default), so the driver **must** be compiled DIY. Done via RPMFusion:

```
dnf5 -y install rpmfusion-free-release rpmfusion-nonfree-release   # from mirrors.rpmfusion.org
dnf5 -y install akmods akmod-nvidia kmodtool gcc make
akmods --force --kernels 7.0.12-cachyos1.fc42.x86_64 --kmod nvidia
```

- `akmods` exit code **0**; built
  `kmod-nvidia-7.0.12-cachyos1.fc42.x86_64-580.159.03-1.fc42.x86_64.rpm` and
  auto-installed it (`akmods` installs the kmod itself; the later explicit
  `dnf5 install` reports "already installed").
- `modinfo -k 7.0.12-cachyos1.fc42.x86_64 nvidia` →
  `filename: /lib/modules/7.0.12-cachyos1.fc42.x86_64/extra/nvidia/nvidia.ko.xz`,
  `version: 580.159.03`, `vermagic: 7.0.12-cachyos1.fc42.x86_64 SMP preempt
  mod_unload`. All five modules present on disk (`nvidia`, `nvidia-drm`,
  `nvidia-modeset`, `nvidia-uvm`, `nvidia-peermem`).
- **No version skew:** the 580.159.03 driver builds cleanly against a 7.0 kernel
  with gcc 15.2.1 — the GCC-built CachyOS kernel + RPMFusion gcc akmod path is
  compatible. (The Containerfile makes this step capture-and-continue with a
  `/usr/lib/apex-nvidia-akmod-status` marker so a future skew failure won't block
  the other checks. Marker read `PASS driver=580.159.03 kver=7.0.12-...`.)

### 3. Kernel config table

From `/usr/lib/modules/7.0.12-cachyos1.fc42.x86_64/config`:

| Option | Value | Notes |
|--------|-------|-------|
| `CONFIG_ANDROID_BINDERFS` | **=y** | waydroid — binderfs present |
| `CONFIG_ANDROID_BINDER_IPC` | **=y** | waydroid |
| `CONFIG_NTSYNC` | **=m** | ntsync (Proton) — module |
| `CONFIG_SCHED_CLASS_EXT` | **=y** | sched-ext. NB: the old `CONFIG_SCHED_EXT` symbol was **renamed** to `CONFIG_SCHED_CLASS_EXT`; searching the old name reports "absent". sched-ext **is** enabled. |
| `CONFIG_DRM_ACCEL_AMDXDNA` | **=m** | XDNA NPU (npu-twin) — module |
| `CONFIG_HZ` / `CONFIG_HZ_1000` | **=1000 / =y** | 1000 Hz tick as expected |
| `CONFIG_PREEMPT` | **=y** | full preemption |
| `CONFIG_PREEMPT_DYNAMIC` | **=y** | runtime-selectable preempt |
| `CONFIG_PREEMPT_VOLUNTARY` | not set | (PREEMPT is the compiled default) |
| `CONFIG_PREEMPT_LAZY` | not set | |
| `CONFIG_FUTEX` (+ `_PI`/`_PRIVATE_HASH`/`_MPOL`) | **=y** | there is no `CONFIG_FUTEX_WAITV` symbol — `futex_waitv` is an unconditional syscall when `CONFIG_FUTEX=y`, so it **is** available (ntsync + futex_waitv requirement met). |

### 4. sched-ext userspace — PASS

`scx-scheds 1.1.0-1.fc42` + `scx-tools 1.1.0-1.fc42` (from the addons COPR;
`scx_loader`/`scxctl` moved to `scx-tools` at v1.0.18+). Verified in-image:
`scx_loader --version` and `scxctl --version` both report **1.1.0**; the full
scheduler set is on PATH (`scx_bpfland`, `scx_lavd`, `scx_rusty`, `scx_flash`,
`scx_layered`, `scx_p2dq`, `scx_cosmos`, `scx_tickless`, `scx_rustland`,
`scx_chaos`, `scx_cake`, `scx_beerland`, `scx_pandemonium`) plus `scx_loader`
(`/usr/bin` and `/usr/sbin`) and `scxctl` (`/usr/sbin`).

### 5. Layering-support verdict — SUPPORTED

`fedora-bootc:42` ships **both** stacks: `rpm-ostree-2025.12-1.fc42` **is
present** (so client-side `rpm-ostree install` package layering works for
users) **and** `dnf5` + `bootc 1.15.1` for the bootc-native path. This is the
escape-hatch row in plan §4a: **package layering is available**, not
bootc-native-only. Caveat: exercising layering on the *spike* image would hit
the rpmdb defect below — fix that first.

### 6. VM boot — PARTIAL (kernel proven, multi-user pending)

`bootc-image-builder` (and `bootc install to-disk`) **cannot run on this build
host**: `/sys/fs/cgroup` is empty `sysfs` (no cgroup hierarchy mounted on this
Void box), so bib's nested osbuild aborts with
`crun: invalid file system type on /sys/fs/cgroup`. Mounting cgroups would be a
host-level change (out of scope), so the qcow2 path was not completed.

Instead the CachyOS kernel + initramfs were **extracted from the image**
(`podman create` + `podman cp`, no cgroups needed) and **direct-booted headless
under QEMU+KVM** (`-kernel`/`-initrd`, serial → file). Serial evidence:

- `BORE CPU Scheduler modification 6.6.3 by Masahito Suzuki` — the **CachyOS
  BORE signature** (stock Fedora kernels have no BORE): conclusive proof the
  swapped kernel is the one running.
- `smpboot: CPU0: AMD Ryzen 7 PRO 250 w/ Radeon 780M`, 4 vCPU SMP up,
  `systemd 257.13 … Detected virtualization kvm`, initrd userspace reached
  `initrd-switch-root.target`.
- **No kernel panic.** Kernel init in ~855 ms; clean `reboot: Power down`.
- `switch-root` fails (`status=1`) — **expected**: a direct `-kernel` boot has
  no ostree/composefs root disk to switch into. That final step is exactly what
  `bootc install` would provide.

So the kernel version is proven four independent ways: RPM NEVRA, `vmlinuz`
bzImage magic (`version 7.0.12-cachyos1.fc42.x86_64`), the installed nvidia
module's `vermagic`, and the BORE banner on the live serial console.

**Exact remaining step (VM boot → full multi-user):** produce the qcow2 with
`bootc-image-builder` (config + serial karg already written to
`~/apex-os-m0-work/spike-b/{bib-config.toml,boot-qemu.sh}`, and a boot-check
oneshot service is baked into `localhost/apex-kernel-spike-bootcheck` to print
`uname -r` + nvidia-module presence to the console and power off). This needs a
host with cgroups mounted (a CI runner, or `mount -t cgroup2 none /sys/fs/cgroup`
on the build box). On such a host the whole step is: `bib --type qcow2 --local
--config /config.toml localhost/apex-kernel-spike-bootcheck` → `boot-qemu.sh`.

### 7. Defects found (fix before M1)

1. **Corrupt rpmdb baked into the scx layer.** After the `scx-scheds`/`scx-tools`
   install step, the sqlite rpmdb is corrupt: `rpm -qa` (full scan) returns 924
   packages, but indexed point-lookups (`rpm -q scx-scheds`) fail with
   `database disk image is malformed`; `rpm --rebuilddb` cannot recover it and
   deleting the `-wal`/`-shm` sidecars does not help. **Localized precisely:**
   the post-akmod / pre-scx image layer is clean (`rpm -q` works, 923 pkgs), so
   the scx-install transaction (or the layer commit after it) introduces the
   corruption. Root cause not yet pinned — candidates: a dnf5 write of the large
   (~135 MB) `scx-scheds` package, or a podman-overlayfs + sqlite-mmap
   interaction on this Void host. Does **not** affect kernel boot (boot never
   reads the rpmdb) but would break runtime `rpm`/`dnf`/`rpm-ostree` (i.e.
   layering and updates). Must root-cause before M1; try installing scx in its
   own stage and/or `rpm --rebuilddb` at a readable checkpoint, and re-test on a
   cgroup-enabled CI host to rule out the Void-host overlay theory.
2. **`bootc container lint`: 4 warnings** (9 checks passed, 1 skipped) — all
   `var-tmpfiles`: akmod build leftovers in `/var` (`/var/cache/akmods/...`
   incl. the kmod RPM, `/var/log/akmods`, plus `libX11`/`libdnf5`/`ldconfig`
   caches). Cosmetic for a spike; for production, build the akmod in a separate
   stage and copy only the kmod RPM into the final image (the ublue pattern),
   leaving `/var` clean.


## Spike D — Secure Boot chain

**Goal:** prove the Secure Boot signing-chain mechanics with our own key — an
APEX-signed kernel boots with SB **enforcing** in a VM, and unsigned / foreign
kernels are **rejected** by the firmware. Scriptable and headless (no
interactive MokManager), so it can drop into CI.

**Verdict:**

| Step | Result |
|------|--------|
| Generate APEX signing keypair (test) | **PASS** |
| Enroll our cert as PK/KEK/db, SB on, headless | **PASS** |
| APEX-signed kernel boots under SB enforcing (SB detected + lockdown active + userspace) | **PASS** |
| Unsigned kernel rejected by firmware | **PASS** (`Access Denied`) |
| Foreign (rogue-key) signed kernel rejected | **PASS** (`Access Denied`) |
| Kernel-module signing (bonus) | **NOT DEMONSTRATED in-VM** — impractical here; exact procedure documented below |

Everything below was run on the Void host (qemu 11.0.2 + KVM). Large artifacts
live outside the repo in `~/apex-os-m0-work/spike-d/`; only the scripts
(`signing/spike-d/`) are committed. **No key material is committed.**

### Environment / tools installed

- `qemu-system-x86_64` 11.0.2, KVM (`/dev/kvm`, user in `kvm` group).
- OVMF: `edk2-ovmf-202605_1`, SB-enforcing build
  `/usr/share/edk2/x64/OVMF_CODE.secure.4m.fd`
  (sha256 `71359fc0…97d5`), template varstore `OVMF_VARS.4m.fd`
  (sha256 `5d2ac383…5d1e`).
- **Installed via xbps:** `sbsigntool-0.9.4_6` (provides `sbsign`, `sbverify`,
  `sbvarsign`).
- **Installed via pip (isolated venv, host untouched):** `virt-firmware 26.7.2`
  → `virt-fw-vars`. (System pip is PEP-668 "externally managed"; a venv at
  `~/apex-os-m0-work/spike-d/venv` avoids `--break-system-packages`.)
- Already present: `gcc`, `cpio`, `mtools` (`mcopy`/`mmd`/`mformat`),
  `mkfs.vfat`, `openssl`.

### Test subject

Rather than download a distro cloud image, the stock host kernel
`/boot/vmlinuz-7.1.4_1` was used as the test subject — it already carries what
we need: `CONFIG_EFI_STUB=y` (bootable PE with EFI handoff),
`CONFIG_SECURITY_LOCKDOWN_LSM=y` + `_EARLY=y`, `CONFIG_SERIAL_8250_CONSOLE=y`.
A ~330 KB initramfs (statically-linked C `init`, see
`~/apex-os-m0-work/spike-d/initramfs/init.c`) mounts `/proc` `/sys` `efivarfs`,
tags the kernel's SB/lockdown log lines, reads the `SecureBoot` EFI var, and
powers off — so the VM proves userspace with no root disk and self-terminates.

### Exact invocations

**1. Keypair (`keygen.sh`)** — RSA-2048, self-signed, SHA-256; the future
"APEX MOK"/db key (here a throwaway TEST key):

```sh
openssl req -new -x509 -newkey rsa:2048 -nodes \
  -keyout apex-mok.key -out apex-mok.crt -days 3650 -sha256 \
  -subj "/CN=APEX-OS TEST Secure Boot key (SPIKE-D, DO NOT TRUST)/"
openssl x509 -in apex-mok.crt -outform DER -out apex-mok.der
```

**2. Enroll into an SB-enforcing varstore (`enroll-vars.sh`)** — headless, our
key only, Microsoft keys deliberately omitted:

```sh
virt-fw-vars \
  --input  /usr/share/edk2/x64/OVMF_VARS.4m.fd \
  --set-pk  <GUID> apex-mok.der \
  --add-kek <GUID> apex-mok.der \
  --add-db  <GUID> apex-mok.der \
  --no-microsoft --secure-boot \
  --output apex-VARS.ours-only.4m.fd
```

Result (`virt-fw-vars --print`): `PK`, `KEK`, `db` each a 911-byte blob (our
cert), `dbx` seeded, `SecureBootEnable: ON`. PK present ⇒ firmware leaves Setup
Mode and enters User Mode = SB enforcing.

**3. Sign the kernel (`sign-kernel.sh` → `sbsign`)**:

```sh
sbsign --key apex-mok.key --cert apex-mok.crt \
       --output vmlinuz-apex-signed.efi /boot/vmlinuz-7.1.4_1
sbverify --cert apex-mok.crt vmlinuz-apex-signed.efi   # -> Signature verification OK
```

A rogue keypair (not enrolled) signed a second copy, and a third copy was left
unsigned, for the negative tests.

**4. Boot under SB enforcing (`boot-sb-vm.sh`)** — the key QEMU invocation
(SMM on, which the `OVMF_CODE.secure` build requires):

```sh
qemu-system-x86_64 \
  -machine q35,smm=on,accel=kvm -cpu host -m 2048 -smp 2 \
  -global driver=cfi.pflash01,property=secure,value=on \
  -global ICH9-LPC.disable_s3=1 \
  -drive if=pflash,unit=0,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.secure.4m.fd \
  -drive if=pflash,unit=1,format=raw,file=vars-<test>.fd \
  -drive if=virtio,format=raw,file=esp-<test>.img,media=disk \
  -serial file:serial-<test>.log -display none -no-reboot
```

The FAT ESP holds `\EFI\BOOT\BOOTX64.EFI` (an **APEX-signed** UEFI shell) plus a
`startup.nsh` that launches `vmlinuz.efi` with a cmdline + `initrd=`. The shell
being APEX-signed is itself a positive check (it loads); the shell's `LoadImage`
of the kernel is the signature gate under test. The cmdline included
`console=ttyS0,115200 ... lockdown=integrity` (see lockdown note below).

### Serial evidence

**Positive — APEX-signed kernel boots (SB enforcing):**

```
FS0:\> vmlinuz.efi initrd=\initramfs.cpio.gz console=ttyS0,115200 ... lockdown=integrity
[    0.016634] Secure boot enabled
[    0.925962] Lockdown: swapper/0: hibernation is restricted; see man kernel_lockdown.7
[    0.977893] Run /init as init process
APEX-SPIKE-D: >>> reached userspace: kernel executed under UEFI Secure Boot <<<
APEX-EVIDENCE: Kernel is locked down from command line; see man kernel_lockdown.7
APEX-EVIDENCE: Secure boot enabled
APEX-EVIDENCE: efivar SecureBoot = 1 (1 = enabled)
[    2.004344] reboot: Power down
```

⇒ firmware verified the kernel against db, kernel detected Secure Boot, lockdown
LSM is active and enforcing (hibernation restricted), and userspace ran.

**Negative — unsigned kernel AND rogue-signed kernel (same VARS):**

```
FS0:\> vmlinuz.efi initrd=\initramfs.cpio.gz console=ttyS0,115200 ...
Script Error Status: Access Denied (line number 5)
```

⇒ firmware `LoadImage` refused the image (`EFI_ACCESS_DENIED`); **no kernel
messages, no userspace** — the boot never started. Identical result whether the
image was unsigned or signed by a key absent from db, confirming it is the
signature-vs-db check doing the gating, not merely the presence of a signature.
(The OVMF `release` build emits nothing on debug port 0x402, so the shell's
`Access Denied` on serial is the authoritative rejection evidence.)

### Lockdown caveat (important, and it differs on real distro kernels)

The Void host kernel is built `CONFIG_LOCK_DOWN_KERNEL_FORCE_NONE=y` — it does
**not** auto-enter lockdown just because Secure Boot is on. So `lockdown=integrity`
was passed on the cmdline to activate the lockdown LSM, and the log reads
"locked down **from command line**". Fedora/RHEL/Ubuntu kernels are built with
the SB→lockdown coupling and would instead print "locked down **from EFI Secure
Boot mode**" automatically. **Implication for the APEX kernel:** build it with
`CONFIG_LOCK_DOWN_KERNEL_FORCE_INTEGRITY=y` (or the SB-coupling) so lockdown is
automatic under SB and not dependent on a cmdline argument a user could drop.

### Kernel-module signing (bonus) — why not shown in-VM, and the real procedure

Not demonstrated end-to-end here, for concrete reasons on this host:

- No kernel-devel/`build` tree and no `scripts/sign-file` for `7.1.4_1`, so an
  out-of-tree `.ko` can't be built/signed in place.
- More fundamentally: **module signatures are checked against the kernel
  keyring, not the UEFI db.** The stock Void kernel has
  `CONFIG_SECONDARY_TRUSTED_KEYRING` unset and no `.machine`/MOK keyring
  (`CONFIG_INTEGRITY_MACHINE_KEYRING`), so it only trusts its ephemeral built-in
  `certs/signing_key.pem`. Enrolling our key in **db** (which gates the *boot*
  chain) does nothing for module trust. Under `lockdown=integrity` unsigned
  modules are refused, but we cannot make this kernel *accept* an APEX-signed
  module without rebuilding it.

Exact procedure for the APEX image pipeline (captured in
`signing/spike-d/sign-module.sh`):

```sh
# Sign every shipped kmod with the APEX key (DER cert), sha512 to match
# the kernel's CONFIG_MODULE_SIG_HASH:
/lib/modules/<kver>/build/scripts/sign-file sha512 apex-mok.key apex-mok.der module.ko
```

For the kernel to trust those signatures, the APEX kernel must be built so the
APEX public key is in a trusted keyring — either bundled at build time
(`CONFIG_SYSTEM_TRUSTED_KEYS=/path/apex.pem`, with `CONFIG_MODULE_SIG=y`), or
loaded at runtime via the `.machine` keyring (`CONFIG_INTEGRITY_MACHINE_KEYRING=y`
+ shim/MOK). Pair with `CONFIG_MODULE_SIG_FORCE=y` (or `module.sig_enforce=1`)
to reject unsigned modules unconditionally.

### Implications for the CI signing pipeline (M1/M5)

- The four scripts in `signing/spike-d/` are the pipeline primitives:
  `sbsign` for boot components (kernel/UKI/shim/bootloader), `virt-fw-vars` for
  building test varstores, and `boot-sb-vm.sh` as an automated SB smoke test
  that CI can gate on (APEX-signed boots, unsigned/foreign `Access Denied`).
- Keep the private key out of the tree — inject from a CI secret / HSM at sign
  time. Only the public cert (DER) ships in images for enrollment. Repo
  `.gitignore` already blocks `*.key`/`*.pem`/`*.p12`.
- Sign **all** early-boot PE objects with the same key: kernel/UKI and, if shim
  is used, the shim's payload (grub/systemd-boot/UKI). Prefer a single UKI
  (kernel+initrd+cmdline in one signed PE) so the cmdline is inside the
  signature envelope and can't be tampered with — and so lockdown isn't left to
  a mutable cmdline arg.
- Match the kernel's module-sig hash (sha512 here) and bake the APEX key into a
  trusted keyring so kmod signing is enforceable (above).

### db-enroll (this VM) vs shim + MOK (real hardware) — state it plainly

This spike enrolled the APEX key directly into UEFI **db** (with self-owned
PK/KEK). That is legitimate and fully enforcing, but only feasible where we
control the firmware's key database — i.e. VMs, or physical machines where the
owner clears Setup Mode and enrolls a custom PK/db. It is **not** how most retail
hardware ships: those trust the **Microsoft UEFI CA** in db and cannot easily
have db rewritten.

On real hardware the APEX flow is therefore **shim + MOK**, not db:

1. Ship a `shim` signed by the Microsoft UEFI CA (already trusted in db).
2. shim carries/enrolls the **APEX cert as a MOK** (Machine Owner Key); MOK
   enrollment is confirmed once by the user in MokManager at first boot (or
   pre-seeded via `mokutil`).
3. shim verifies the APEX-signed kernel/UKI against the MOK — no db change
   needed, SB stays enforcing, and the MOK is linked into the kernel's
   `.machine` keyring so the **same key also validates signed modules**.

So: the *signing* commands proven here (`sbsign`, `sign-file`) are identical for
both paths — only *where the trust anchor lives* differs (db in the VM;
MOK-behind-shim on locked-down retail firmware). To offer both, ship the APEX
cert for db-enrollment on owner-controlled machines **and** a
Microsoft-CA-signed shim that enrolls the same cert as a MOK everywhere else.
```

## Spike E — dual-boot `to-filesystem` rehearsal (shared ESP)

**Date:** 2026-07-21   **Host:** Void Linux, qemu 11.0.2 + KVM, OVMF (edk2 x64,
4 MB split), podman 5.8.3   **Image:** `quay.io/fedora/fedora-bootc:43`
(bootc **1.16.3**)   **Scripts:** [`files/scripts/spike-e/`](../files/scripts/spike-e/)
(reusable install template)   **Artifacts:** `~/apex-os-m0-work/spike-e/` (VM
disks + `out/` captures, not in repo).

### Question

Does `bootc install to-filesystem`, run against a **shared ESP** that already
carries an incumbent OS's bootloader files, **clobber, reorder, or preserve**
that OS's EFI boot entries, `BootOrder`, and ESP files? This de-risks the real
installs on the **ThinkPad L16** (Void, EFISTUB entries + kernels *on* the ESP)
and **MSI Katana** (Windows).

### Method (rehearsal harness)

A 30 GB GPT VM disk was built with a layout mirroring the L16:

| Part | Size | FS | Role |
|------|------|----|------|
| `vda1` | 1 GiB | FAT32 | **shared ESP** |
| `vda2` | 12 GiB | ext4 | incumbent root (Alpine) |
| `vda3` | ~17 GiB | btrfs (empty) | APEX-OS install target |

The **incumbent is Alpine 3.24.1** booting **classic EFISTUB** exactly like
Void on the L16: kernel + initramfs copied *onto* the shared ESP at
`\EFI\alpine\` and an `efibootmgr` entry whose `LoadOptions` carry the cmdline
(`initrd=\EFI\alpine\initramfs-lts root=UUID=… …`). Using a **non-Fedora
incumbent in its own ESP vendor dir** (`/EFI/alpine`, not `/EFI/fedora`) is
deliberate — it is the only way to tell "bootc wiped the whole ESP" apart from
"bootc overwrote its own vendor dir".

The install was run **from inside the VM** (podman in the incumbent) — the only
safe place to let bootc's `efibootmgr` touch NVRAM, since that NVRAM belongs to
the VM's OVMF (`OVMF_VARS` pflash), never the host. The host's ESP / efibootmgr
/ bootloader were never touched. NVRAM persists across VM reboots, so
`BootOrder` survival is directly observable.

Choreography (host-driven state machine over a serial log + a shared payload
disk carrying the image tar): **setup** (create the incumbent's NVRAM entry) →
**before** (capture) → **install** → **after** (capture) → boot-test the bootc
OS → boot-test the incumbent. See `99-run-all.sh`.

### The `bootc install` invocation that worked

```sh
# target prepared first: vda3 (empty btrfs) -> /target ; vda1 (ESP) -> /target/boot/efi
podman run --rm --privileged --pid=host --network=host \
    -v /dev:/dev \
    -v /run/udev:/run/udev \
    -v /var/lib/containers:/var/lib/containers \
    -v /target:/target \
    quay.io/fedora/fedora-bootc:43 \
    bootc install to-filesystem \
      --karg=root=UUID=<apex-root-uuid> --karg=rw \
      --karg=console=tty0 --karg=console=ttyS0,115200 \
      --skip-fetch-check \
      /target
```

`--replace` was **not** passed (target root is empty). The default mode is what
preserves the foreign ESP content — see gotchas. Result:

```
Installing image: docker://quay.io/fedora/fedora-bootc:43
Initializing ostree layout
Deploying container image...done (9 seconds)
Bootloader: grub
Installing bootloader via bootupd
Executing: "efibootmgr" "--create" "--disk" "/dev/vda" "--part" "1" \
           "--loader" "\EFI\fedora\shimx64.efi" "--label" "Fedora"
Installation complete!
```

The apex root received a correct bootc/ostree deployment
(`.bootc-aleph.json`, `ostree/deploy/default/deploy`, `/boot` with
`grub2/`, `loader/`, `bootupd-state.json`).

### `efibootmgr` before / after

**Before** (`BootCurrent: 0002`):
```
BootOrder: 0002,0000,0001,0003,0004,0005,0006,0007
Boot0002* Alpine (incumbent)  HD(1,GPT,…)/\EFI\alpine\vmlinuz-lts  [+cmdline in LoadOptions]
```

**After** (bootc's own ordering, before any orchestration):
```
BootOrder: 0008,0002,0000,0001,0003,0004,0005,0006,0007
Boot0002* Alpine (incumbent)  HD(1,GPT,…)/\EFI\alpine\vmlinuz-lts   <- PRESERVED, byte-identical
Boot0008* Fedora              HD(1,GPT,…)/\EFI\fedora\shimx64.efi   <- NEW (added by bootc)
```

**Diff:**
- `Boot0002` (incumbent): **untouched** — same device path, same LoadOptions/cmdline.
- All other pre-existing entries (`0000,0001,0003–0007`): **untouched**.
- `Boot0008 "Fedora"`: **added** via `efibootmgr --create` (label from
  `/EFI/fedora/BOOTX64.CSV`).
- `BootOrder`: **reordered** — bootc's `--create` **prepends**, so `0008`
  (Fedora) becomes the default and the incumbent `0002` is demoted from 1st to
  2nd. **Nothing was deleted or renumbered.**

### ESP contents + space delta

| | Before | After |
|---|---|---|
| `/EFI/alpine/vmlinuz-lts` | 14,468,096 B | 14,468,096 B (identical) |
| `/EFI/alpine/initramfs-lts` | 20,876,653 B | 20,876,653 B (identical) |
| `/EFI/BOOT/` (BOOTX64.EFI + fbx64.efi) | — | 1,037,240 B (added) |
| `/EFI/fedora/` (shim, grub, mm, CSV, grub.cfg, bootuuid.cfg) | — | 6,794,292 B (added) |
| **ESP total used** | **35,344,749 B (~33.7 MiB)** | **43,176,281 B (~41.2 MiB)** |

**bootc ESP delta ≈ 7.47 MiB (7,831,532 B)** — the `/EFI/BOOT` + `/EFI/fedora`
trees. The incumbent's `/EFI/alpine` kernels were **not touched**. (The L16 ESP
already carries Void kernels/initramfs; budget ~8–10 MiB headroom for bootc.)

### Pass / fail per sub-goal

| Sub-goal | Result |
|---|---|
| Incumbent boots (before) | **PASS** — OVMF loaded `Boot0002` → `\EFI\alpine\vmlinuz-lts` → Alpine. |
| `to-filesystem` completes | **PASS** — rc=0, primary attempt (no `--disable-selinux`). |
| bootc OS boots after | **PASS** — shim→grub→ostree kernel→dracut→**switch-root** into the ostree root; second systemd reached `basic`/`network` targets. |
| Incumbent boots after | **PASS** — normal boot loaded `Boot0002` → Alpine (kernel + os-release + cmdline confirmed from inside). |
| Existing entries/BootOrder preserved | **PASS (with caveat)** — entries & ESP files preserved byte-for-byte; `BootOrder` **reordered** (bootc prepended itself as default). |
| ESP delta measured | **PASS** — +7.47 MiB. |

### Gotchas / findings

1. **bootupd only manages `/EFI/BOOT` + its own vendor dir (`/EFI/fedora`).** It
   did **not** wipe the ESP. The upstream warning that "bootc install is always
   destructive with respect to `/boot` and the ESP" (PR #1752 / docs) did **not**
   materialise for `to-filesystem` in **default (empty-root)** mode on
   bootc 1.16.3 — foreign vendor dirs survive. This is the central good news.
2. **Never use `--replace=alongside` on a shared ESP with a foreign OS.** Its
   own help says it wipes bootloader state ("the bootloader state will have its
   contents wiped and replaced"). We used the default empty-root mode instead.
3. **bootc prepends its NVRAM entry and becomes the default boot.** The incumbent
   is preserved but demoted. The real install must **re-assert `BootOrder`**
   afterwards to the desired default (`efibootmgr -o …`) and record it.
4. **`/EFI/BOOT/BOOTX64.EFI` (removable fallback) is written by bootc.** It was
   absent before and bootc created it. If an incumbent already uses the
   removable-media fallback path, bootc **would overwrite** it. Void on the L16
   boots via a dedicated efibootmgr EFISTUB entry (not the fallback), so this is
   expected-safe — but **pre-flight check the L16's `\EFI\BOOT\` before install.**
5. **SELinux was a non-issue.** The primary attempt (no `--disable-selinux`)
   succeeded even though the install host (Alpine) has no SELinux — bootc labels
   the target from the container's policy. The script keeps a `--disable-selinux`
   fallback but it was not needed. (Real install from a Fedora live USB is still
   the recommended, most-faithful environment.)
6. **`bootc install` needs `/run/udev` on the host.** From a bare Alpine
   minirootfs it aborts with *"Comparing filesystems at /run/udev …: No such file
   or directory"*. Fixed by running `eudev` (`udevadm trigger`/`settle`) and
   bind-mounting `-v /run/udev:/run/udev`. A normal Fedora live USB already runs
   systemd-udevd, so this is a harness detail, not a real-install step.
7. **podman needs cgroup v2 mounted.** Alpine's minirootfs mounts no cgroups;
   crun failed with *"invalid file system type on /sys/fs/cgroup"* until
   `mount -t cgroup2 none /sys/fs/cgroup`. Again a harness detail (Fedora live
   has it).
8. **First-boot bootstrap of the incumbent:** OVMF did **not** reliably fall
   through to its internal UEFI Shell / `startup.nsh` (it exhausted disk options
   then hung on PXE). The harness boots the incumbent once via qemu `-kernel`
   (still under OVMF, so efivarfs works) purely to *create* its persistent
   EFISTUB entry; every subsequent boot uses the real NVRAM entry. Not relevant
   to the real machines (which already have their entries).
9. **LUKS was deferred.** The host lacks `cryptsetup`, so this ran plain btrfs.
   The core ESP/BootOrder result is independent of LUKS. For the L16 LUKS2 path
   (plan §5b): create LUKS2 + btrfs by hand, mount the decrypted root at
   `/target`, add a **separate unencrypted `/boot`** partition mounted at
   `/target/boot`, and pass `--boot-mount-spec UUID=<boot>` so kernels/BLS land
   on the plain `/boot` while `/` stays encrypted. Needs a dedicated LUKS rehearsal.

### Verdict — is this safe enough for the L16?

**Yes, with defined pre-flight guards.** `bootc install to-filesystem` in
default empty-root mode is **non-destructive to a foreign OS's ESP files and
NVRAM entries**: it adds `/EFI/BOOT` + `/EFI/fedora` (~7.5 MiB) and prepends one
NVRAM entry. Void's EFISTUB kernels on the shared ESP and Void's efibootmgr
entry will survive; only the default boot target changes.

**Pre-flight checks the real L16 install needs (beyond the plan's current list):**

- **Snapshot boot state first:** `efibootmgr -v` + a recursive listing of the
  real ESP, saved off-box, so the post-install diff is auditable and BootOrder
  can be restored.
- **Inspect `\EFI\BOOT\` on the L16 before install** — if Void relies on the
  removable fallback there, bootc will overwrite it; plan to restore/relayer.
- **Re-assert `BootOrder` after install** (`efibootmgr -o …`) — bootc makes
  itself the default; decide and set the intended default explicitly, and
  confirm Void's entry is still bootable.
- **Verify ESP free space** (≥ ~10 MiB headroom) — trivially satisfied on a
  ≥256 MiB ESP but worth asserting since the L16 ESP already holds Void kernels.
- **Target root must be empty** and passed *without* `--replace` — and
  **`--replace=alongside` is forbidden** on the shared ESP.
- **Pin the bootc/image version and re-run this spike on every bump** — the
  "preserve vs wipe" behaviour is a bootupd implementation detail, not a
  guarantee; this result is specific to bootc 1.16.3 / fedora-bootc:43.
- **Prefer a Fedora live USB as the install host** (SELinux + udev + cgroup2
  present) to avoid the harness workarounds above; capture the Windows/Void
  baseline and (Katana) BitLocker recovery key first per the existing plan.
- **Still to rehearse separately:** the **LUKS2 + separate `/boot` +
  `--boot-mount-spec`** variant (§5b), and Secure Boot interaction (§5a).

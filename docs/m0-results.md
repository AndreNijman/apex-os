# M0 results

Findings and evidence from the M0 spikes. Each spike proves one mechanic of the
APEX-OS build/boot chain before it is committed to in later milestones.

---
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


# APEX-OS — M1 notes (production images + CI)

M1 turns the proven M0 spike Containerfiles into three real production images
(shared **base** → **daily** / **gaming**) plus a GitHub Actions pipeline that
builds, cosign-signs (keyless), and pushes them to GHCR.

All four local builds were done on the Void host with **rootful** podman 5.8.3
(`sudo podman build --isolation=chroot`) — see "Host caveats" for why rootful
was required. Big artifacts stay out of the repo.

## Local build results

| Image | Build | Size | Time (Void host, cold cache) | Notes |
|-------|-------|------|------------------------------|-------|
| `Containerfile.base` | **PASS** | **4.28 GB** | ~38.7 min | kernel swap + desktop + scx + Bazaar; time dominated by slow overlay layer-commits on this host + the Nerd Font download, NOT dnf |
| `Containerfile.daily` | **PASS** | **4.33 GB** | ~4.5 min | VARIANT stamp + power-profiles-daemon |
| `Containerfile.gaming` GPU=mesa | **PASS** | **4.28 GB** | ~4.2 min | VARIANT stamp only (mesa already in base) |
| `Containerfile.gaming` GPU=nvidia | **PASS** | **5.49 GB** | ~7.5 min | RPMFusion + DIY NVIDIA akmod (+1.2 GB for driver/kmod) |
| `Containerfile.gaming` GPU=bogus | **FAILS (as designed)** | — | seconds | proves the production HARD-FAIL: unknown GPU aborts the build (exit 1) |

`bootc container lint` on every image: **10 passed, 1 skipped, 3 warnings**.
The 3 warnings are the standard cosmetic bootc ones after a dnf-based build
(`nonempty-run-tmp`, `var-log`, `var-tmpfiles`) — build leftovers in `/var` and
`/run`. Non-blocking; a future cleanup pass (or building the akmod in a discard
stage, the ublue pattern) would clear them.

### NVIDIA akmod — PASS (the key M1 proof)

The DIY akmod built and resolved against the base's CachyOS kernel:

```
Built kmod RPM: kmod-nvidia-7.1.3-cachyos1.fc43.x86_64-580.173.02-1.fc43.x86_64.rpm
modinfo nvidia (kver 7.1.3-cachyos1.fc43.x86_64):
  filename: /lib/modules/7.1.3-cachyos1.fc43.x86_64/extra/nvidia/nvidia.ko.xz
  version:  580.173.02
=> /usr/lib/apex-nvidia-akmod-status: "PASS driver=580.173.02 kver=7.1.3-cachyos1.fc43.x86_64"
```

Unlike the M0 spike (capture-and-continue), production **hard-fails** if the
akmod does not build/resolve: `set -e` plus explicit `exit 1` guards on the
kernel-devel presence, the produced kmod RPM, and the final `modinfo`. Verified
end-to-end by the `GPU=bogus` build aborting the pipeline.

## The build defect that had to be fixed for F43 (kernel %posttrans)

M0 spike B built on `fedora-bootc:42` and the CachyOS kernel `%posttrans` was
"non-critical". On `fedora-bootc:43` the same scriptlet is **fatal**:
`kernel-cachyos-core`'s `%posttrans` runs `rpm-ostree kernel-install → dracut`
**before `modules.dep` is written**, so dracut aborts with *"modules.dep is
missing"* and the scriptlet returns non-zero, failing the whole dnf transaction.

Findings (probed directly):
- The kernel **RPMs install fine regardless** — `%posttrans` is the last phase,
  so the packages are committed to the rpmdb even when it fails (`rpm -q` shows
  all four present afterwards).
- After the transaction, `modules.dep` **does** exist and a **manual dracut
  succeeds**, producing a valid ~134 MB `initramfs.img` + `vmlinuz` in
  `/usr/lib/modules/<kver>/` (exactly where bootc boots from).
- This reproduces in **both rootless and rootful** builds (rootless also hits a
  `/dev/kmsg: Permission denied` logger warning, but that is not the fatal
  cause — the missing `modules.dep` is).

**Fix applied in `Containerfile.base` (stage 1):**
1. Tolerate the kernel install's non-zero exit (`… || echo WARN …`), because the
   only failure is the cosmetic `%posttrans` dracut.
2. **Hard-gate on `rpm -q kernel-cachyos kernel-cachyos-core kernel-cachyos-modules
   kernel-cachyos-devel-matched`** — a genuine install failure (COPR down,
   package missing) fails here and aborts the build.
3. In the next step, run `depmod -a` and regenerate the initramfs with a generic
   `dracut`, then `test -s` both `initramfs.img` and `vmlinuz` (hard verify).

This is robust across F42/F43 and rootless/rootful.

## rpmdb corruption (M0 spike B defect #1) did NOT reproduce

Spike B saw the sqlite rpmdb corrupt after the `scx` install layer (indexed
`rpm -q` failing with "database disk image is malformed"). With scx isolated in
its **own** `RUN … && dnf5 clean all` stage, `rpm -q scx-scheds scx-tools`
**succeeded** during the base build (it is baked into the stage). So either the
separate-stage mitigation resolved it, or the corruption was specific to the
Void-host overlay + combined-transaction conditions of the spike. Either way the
prescribed mitigation (separate heavy transactions) holds. Worth re-confirming
on the cgroup-enabled CI runner.

## Package / COPR drift vs M0

| Component | M0 spike | M1 production (fc43) |
|-----------|----------|----------------------|
| Base image | `fedora-bootc:42` (spike B) / `:43` (spike A) | **`:43`** (both) |
| kernel-cachyos | `7.0.12-cachyos1.fc42` | **`7.1.3-cachyos1.fc43`** |
| NVIDIA driver | `580.159.03` | **`580.173.02`** |
| scx-scheds / scx-tools | `1.1.0` | **`1.1.2`** |
| Bazaar (ublue-os/packages COPR) | (not built) | **`0.7.12-3.fc43`** |
| akmods / kmodtool | — | `0.6.2` / `1.1` |
| Newly added in base | — | **sway 1.11, labwc 0.9.6** (greeter hosts), **flatpak, distrobox** |
| Newly added in daily | — | **power-profiles-daemon 0.30** |

COPRs (`bieszczaders/kernel-cachyos{,-addons}`, `solopasha/hyprland`,
`errornointernet/quickshell`, `ublue-os/packages`) and RPMFusion free+nonfree
all resolve cleanly on fc43. All COPRs are `copr disable`d after their install
so the shipped image carries no active COPR repo (RPMFusion is intentionally
left enabled in the gaming image for later userspace layering).

## Greeter host: cage → sway (M0 spike A follow-up)

M0 spike A found `cage` 0.2.0 does not expose `wlr-layer-shell` to quickshell
0.3.0, so the greeter never paints. M1 installs **sway 1.11 + labwc 0.9.6** (both
stock fc43, both layer-shell v4) in the base and switches the greeter host:
`files/desktop/apex-greet/greetd-config.toml` now launches
`sway --config /usr/share/apex-greet/sway-greet.conf` (new file) which runs
quickshell as sway's only client; cage is kept only as a commented fallback.

**Still open:** live pixel rendering is NOT verified. Headless QEMU (local) and
GitHub CI runners both lack GL/GPU, so the greeter render + login must be
confirmed on real hardware or a GPU/virgl-capable runner. This is unchanged from
M0 — M1 wired the correct host but could not visually verify it here.

## Flatpak / Flathub — first-boot unit, not build-time

Build-time `flatpak remote-add --system flathub` returns rc=0, but it writes to
`/var/lib/flatpak`, and bootc treats `/var` as machine-local state that is only
seeded once and never updated on upgrade — a build-time remote is fragile. So
the base ships:
- `/etc/flatpak/flathub.flatpakrepo` (canonical Flathub repo file + GPG key), and
- `apex-flathub-setup.service`, an idempotent first-boot oneshot
  (`flatpak remote-add --if-not-exists`, stamped, `After=network-online.target`).

Flatseal cannot be preinstalled (flatpak install needs a running system); it is
a documented first-run install from Flathub.

## CI design (`.github/workflows/build-image.yml`)

- **Triggers:** push to `main` (paths `Containerfile*`, `files/**`, `kernel/**`,
  `.github/**`), `workflow_dispatch` (with a `build_qcow2` input), weekly cron.
- **Permissions:** `contents: read`, `packages: write`, `id-token: write`
  (the last for cosign keyless OIDC).
- **Job `base`** (ubuntu-latest): rootful `sudo podman build` of
  `Containerfile.base` (rootful because the kernel `%posttrans`/dracut and the
  akmod need device access the runner's rootless podman denies), tags
  `apex-os-base:latest` + `:<sha>`, pushes, captures the pushed **digest** via
  `--digestfile`, cosign-signs the digest, and exposes the digest as a job output.
> **Superseded.** This section records M1, when APEX published three images. It
> is kept as a dated build record, not as current behaviour: there is one image
> now, the `flavors` matrix is a single `image` job, and the NVIDIA akmod is
> built and MOK-signed in `core` rather than in a flavor. See
> `docs/ci-release-tiers.md` and `Containerfile.apex`.

- **Job `flavors`** (needs base): matrix `[daily, gaming-mesa, gaming-nvidia]`.
  Each builds the right Containerfile with
  `--build-arg BASE=apex-os-base@<digest>` (so every flavor is pinned to the
  exact base image the base job signed) + the GPU arg for gaming, tags
  `apex-os:<flavor>` + `:<flavor>-<sha>`, pushes, cosign-signs its digest.
- **Job `qcow2`** (needs flavors, `workflow_dispatch` + `build_qcow2` only,
  `continue-on-error`): runs bootc-image-builder to produce a **daily** qcow2 and
  uploads it as an artifact. GitHub runners have **no `/dev/kvm`**, so the disk
  is produced but NOT booted — producing it is the check. Non-blocking so bib
  flakiness never fails the image pipeline.
- **OCI labels** on every image incl. `org.opencontainers.image.revision=<sha>`
  (injected via `--build-arg APEX_REVISION`), satisfying the "image carries git
  SHA" rule. Gaming also carries `org.apex-os.gpu`.
- **No stored secrets:** GHCR uses the automatic `GITHUB_TOKEN`; cosign uses
  keyless Sigstore/Fulcio via the runner's ambient OIDC — no private key anywhere.
- A `free-disk-space` step reclaims ~10+ GB from unused toolchains on each job
  (the base image is multi-GB).

## Host caveats (local build only — not real-machine changes)

1. **`/dev/shm` was left `755 root:root`** by a prior rootful podman run, which
   blocked rootless podman ("failed to create locks … permission denied"). Fixed
   with `sudo chmod 1777 /dev/shm` — restoring the standard sticky permission
   `/dev/shm` should always have. One-off, harmless.
2. **Rootful build required locally.** Rootless podman on this host cannot run the
   CachyOS kernel `%posttrans` (dracut needs `/dev/kmsg`) nor cleanly build the
   akmod, so all local builds used `sudo podman build --isolation=chroot`
   (chroot isolation also sidesteps the M0 cgroup-mount issue). CI uses the same
   rootful approach for the same reason.

## Top risks for the first CI run

1. **Rootless-vs-rootful on the runner.** The workflow uses `sudo podman` for the
   device access the kernel `%posttrans`/akmod need. If a runner's podman/sudo
   combination misbehaves, fall back to `buildah` in a privileged container or a
   rootful buildah setup. This is the single most likely first-run failure.
2. **Disk space.** base (~4.3 GB) + flavors + layers on a ~14 GB ubuntu-latest
   runner is tight even with the cleanup step; gaming-nvidia (5.5 GB) + its build
   deps are the pinch point. May need a larger runner or more aggressive pruning.
3. **cosign keyless OIDC.** Requires `id-token: write` (set) and that the repo/org
   permits Fulcio issuance; first run may surface an OIDC/permissions hiccup.
4. **Base digest hand-off.** `--digestfile` must capture the *pushed manifest*
   digest and the flavor `FROM …@<digest>` must pull it back; a mismatch (e.g.
   manifest-list vs image digest) would break the flavor pull.
5. **COPR/RPMFusion availability + upstream drift.** The weekly cron will pick up
   new kernel-cachyos/NVIDIA versions; a COPR outage or a kernel↔driver skew would
   fail the akmod job (correctly, now that it hard-fails). Pin/rerun as needed.
6. **bootc-image-builder in CI** (qcow2 job) is unproven here (no local bib run —
   host cgroup limits); left `continue-on-error` deliberately.
7. **Greeter render still unverified** on a GL target (carried from M0).

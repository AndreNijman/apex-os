# APEX-OS rollback & recovery drill

APEX-OS makes two promises: nothing you install or tune can leave you unable to
boot, and OS state maps 1:1 to a git commit. This drill tests both. Two layers
roll back independently: the **image/deployment** (bootc) and the **source**
(git). CI ties them together by stamping the source SHA on every image it builds
(`org.opencontainers.image.revision`).

## 1. Deployment rollback (on a running APEX-OS machine)

bootc keeps the booted deployment and the previous one. To undo a bad update (a
tuning change that tanked FPS, a kernel that won't finish booting, a broken
driver):

```bash
# Interactive: pick the previous deployment at the boot menu (systemd-boot/grub).
# Or, from a working session:
sudo bootc rollback      # swaps default ← → previous, then reboot
sudo systemctl reboot
```

`bootc status` shows both deployments, their image refs, and which one is booted,
rolled back or staged. A rollback swaps a pointer and reboots. It keeps `/etc`
and `/var` (including `/var/home`) as they are.

### Pinning a known-good deployment

By default bootc keeps only the booted and the previous deployment, so two bad
updates in a row can evict the last good image. Pin it before anything risky (a
kernel-channel switch, a COPR bump, a big tuning branch landing):

```bash
sudo ostree admin pin 0          # pin the current (index-0) deployment
# later, when sure the new one is good:
sudo ostree admin pin --unpin 2
```

`apex pin` (M3, the CLI over apexd and bootc) wraps this. `sudo apex channel set`
pins automatically when the move is toward `stable`, which usually deploys an
older image: bootc keeps the booted deployment and one more, so a switch
backwards followed by one update can evict the deployment you would return to.
Before §26 nothing pinned automatically, although this line said it did.

## 2. Source ↔ image mapping (the git side)

Every image CI publishes carries the exact commit it was built from:

```bash
skopeo inspect docker://ghcr.io/andrenijman/apex-os:daily \
  | jq -r '.Labels["org.opencontainers.image.revision"]'
```

For a published tag, that SHA is a commit on `main`. Rolling the OS back to how
it was on 2026-07-21 and checking out that commit are the same operation, seen
from two ends.

An earlier version of this section said promotions to a stable channel get an
annotated git tag (`good-YYYYMMDD`) and a matching image tag. Nobody has ever
created such a tag, and before §26 there was no stable channel to promote to.
`.github/workflows/promote-channel.yml` now moves a `stable`, `candidate` or
`beta` tag onto a digest. You can still recover the git commit from that digest
through the `org.opencontainers.image.revision` label above, which has been the
working link all along.

## 3. Rebuild-from-git drill (the full loop)

This is the CI mechanism, reproduced locally. Run it to prove a revert produces
a bootable image, or to reproduce a build as a contributor:

```bash
git checkout <good-sha-or-tag>
podman build -f Containerfile.daily \
  --build-arg BASE=ghcr.io/andrenijman/apex-os-base:latest \
  -t localhost/apex-os:daily-local .
# switch a machine onto the local build without a registry round-trip:
sudo bootc switch --transport containers-storage localhost/apex-os:daily-local
sudo systemctl reboot
```

The block above predates the single image. `Containerfile.daily` and the
`apex-os-base` repository no longer exist: the image is `Containerfile.apex`,
built on the `:base` tag of `ghcr.io/andrenijman/apex-os`. Today
`./build-local.sh --allow-unsigned base apex` builds `base` and the image on a
local core (pull one first, as `docs/local-builds.md` shows) and tags the result
`localhost/apex-os:apex`; switch to that name instead.

## 4. Factory reset, and what it preserves

`apex recover reset` (§19; `docs/recovery.md` is the reference) has two scopes.
`--scope desktop` removes APEX Shell's settings, keybinds and caches for the
invoking account; `--scope user` adds the blueprint, per-game profiles,
trusted-device registry, local-model settings and recorded agent sessions.
Neither touches a document, a checkout, a credential, a capsule, an installed
package or the booted deployment.

It stays a **dry run** unless you pass both `--commit` and a `--confirm` token
derived from the plan it printed. It refuses to run as root, and it first copies
everything it removes, caches excepted, to `~/apex-reset-backup-<timestamp>`.

```bash
apex recover reset --scope desktop          # prints the loss list, changes nothing
apex recover reset --scope user             # a wider one, still a dry run
```

A **full** factory reset (user accounts removed, `/etc` restored to image state,
disks repartitioned) is the installer's job. No verb on a running system does
it, and `docs/recovery.md` says why: `/etc` holds `passwd`, `fstab` and
`crypttab`, ostree three-way-merges it against the deployment, and no runtime
operation restores it without deploying.

An earlier version of this document claimed `apex reset --keep-home` shipped in
M3. It never did; the verb above is what exists.

## Status of this drill

- **Proven in M0/M1:** images build reproducibly and carry the git-SHA label;
  `bootc switch --transport containers-storage` onto a local build is the
  documented iterate path; a `to-filesystem` install preserves a shared ESP
  (spike E).
- **Verify on hardware (M4):** exercise the live `bootc rollback` + reboot cycle
  and `ostree admin pin` behaviour on the first real install (the dev box can't
  boot a bootc deployment). Do it on purpose during the L16 parallel-run, before
  daily-driving, as part of the §14 gates.

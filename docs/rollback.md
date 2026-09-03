# APEX-OS rollback & recovery drill

APEX-OS's promise is that nothing you install or tune can leave you unable to
boot, and that OS state maps 1:1 to a git commit. This doc is the drill that
makes that real. Two layers roll back independently: the **image/deployment**
(bootc) and the **source** (git). They are kept in sync by the SHA label CI
stamps on every image (`org.opencontainers.image.revision`).

## 1. Deployment rollback (on a running APEX-OS machine)

bootc keeps the currently-booted deployment and the previous one. To undo a
bad update — a tuning change that tanked FPS, a kernel that won't finish
boot, a broken driver:

```bash
# Interactive: pick the previous deployment at the boot menu (systemd-boot/grub).
# Or, from a working session:
sudo bootc rollback      # swaps default ← → previous, then reboot
sudo systemctl reboot
```

`bootc status` shows both deployments, their image refs, and which is booted /
rolled back / staged. Rollback is a pointer swap + reboot; `/etc` and `/var`
(including `/var/home`) are preserved.

### Pinning a known-good deployment

bootc only retains booted + previous by default, so two bad updates in a row
can evict the last good image. Before anything risky (kernel-channel switch,
COPR bump, a big tuning branch landing) pin it:

```bash
sudo ostree admin pin 0          # pin the current (index-0) deployment
# later, when sure the new one is good:
sudo ostree admin pin --unpin 2
```

`apex pin` (M3, the CLI over apexd+bootc) wraps this and auto-pins before
kernel-channel switches.

## 2. Source ↔ image mapping (the git side)

Every image CI publishes carries the exact commit it was built from:

```bash
skopeo inspect docker://ghcr.io/andrenijman/apex-os:daily \
  | jq -r '.Labels["org.opencontainers.image.revision"]'
```

That SHA is a commit on `main`. So "roll the OS back to how it was on
2026-07-21" and "check out that commit" are the same operation from two ends.
Promotions to a stable channel get an annotated git tag (`good-YYYYMMDD`),
and the matching image is tagged the same — pinning a deployment and checking
out its tag land you on identical state.

## 3. Rebuild-from-git drill (the full loop)

The mechanism CI uses, reproduced locally — this is what you run to prove a
revert actually produces a bootable image, and what a contributor runs to
reproduce a build:

```bash
git checkout <good-sha-or-tag>
podman build -f Containerfile.daily \
  --build-arg BASE=ghcr.io/andrenijman/apex-os-base:latest \
  -t localhost/apex-os:daily-local .
# switch a machine onto the local build without a registry round-trip:
sudo bootc switch --transport containers-storage localhost/apex-os:daily-local
sudo systemctl reboot
```

## 4. Factory reset, and what it preserves

`apex recover reset` (§19 — `docs/recovery.md` is the reference). Two scopes:
`--scope desktop` removes APEX Shell's settings, keybinds and caches for the
invoking account; `--scope user` adds the blueprint, per-game profiles,
trusted-device registry, local-model settings and recorded agent sessions.
Neither touches a document, a checkout, a credential, a capsule, an installed
package or the booted deployment.

It is a **dry run** unless given both `--commit` and a `--confirm` token
derived from the plan it printed, it refuses to run as root, and it copies
everything it removes to `~/apex-reset-backup-<timestamp>` first.

```bash
apex recover reset --scope desktop          # prints the loss list, changes nothing
apex recover reset --scope user             # a wider one, still a dry run
```

A **full** factory reset — user accounts removed, `/etc` restored to image
state, disks repartitioned — is the installer's job, not a verb on a running
system. `docs/recovery.md` says why: `/etc` holds `passwd`, `fstab` and
`crypttab`, ostree three-way-merges it against the deployment, and there is no
runtime operation that restores it without deploying.

An earlier version of this document claimed `apex reset --keep-home` shipped in
M3. It never did; the verb above is what exists.

## Status of this drill

- **Proven in M0/M1:** images build reproducibly and carry the git-SHA label;
  `bootc switch --transport containers-storage` onto a local build is the
  documented iterate path; `to-filesystem` install preserves a shared ESP
  (spike E).
- **Verify on hardware (M4):** the live `bootc rollback` + reboot cycle and
  `ostree admin pin` behaviour must be exercised on the first real install
  (the dev box can't boot a bootc deployment). Do this deliberately during
  the L16 parallel-run, before daily-driving, as part of the §14 gates.

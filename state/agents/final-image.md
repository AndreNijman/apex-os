# final-image — P0-001: build the image from the v2.2 tip, confirm two fixes on katana

repo: apex-os
branch: `task/final-image` (cut from `roadmap/v2.2` @ `661a9d80`; evidence commits only)
worktree: `/var/tmp/apex-work/wt-final-image`

## THE RUN ID — recorded first so a cut round loses nothing

**Run 35469530380** — `workflow_dispatch`, `build-image.yml`, ref
`task/final-image` @ `661a9d80`, queued 2026-09-19T21:08:57Z.
https://github.com/AndreNijman/apex-os/actions/runs/35469530380

Job ids for `gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs`
(`gh run view --log` REFUSES while a run is in progress):

```
105967845288 rust           success
105967845376 changes        success
105967845750 installer-iso  skipped   (dispatch input default false)
105967873637 core           in progress
```

**A skipped job reads as success here — read each job's STEPS, not its
conclusion.** Log bytes contain ANSI escapes, so plain `grep` calls the file
binary and prints NOTHING. Use `grep -a`. That silence reads exactly like a
real absence.

Two decisions already read out of the `changes` job:

* `Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/final-image): 03d77f96f521df7501d710d5c15ae8d0074a39ce` — the ordered
  fallback worked. **NOT a main-shell build**, and it is the same shell sha the
  last qualified image (`apex-97c9e8f2`) carried.
* `core rebuild: true (core sources changed)`. The diff `97c9e8f2..661a9d80`
  touches **no** `Containerfile*` and **no** `kernel/**`, so this is not a real
  core source change: `dorny/paths-filter` on a first dispatch of a brand-new
  branch has no base to diff against and reports everything changed. The last
  image run said the same. Consequence: ~45 min longer, and katana's pull is
  the whole ~6.3 GB rather than tens of MiB.

## THE ARTIFACT — run 35469530380, `success` at 2026-09-19T22:29Z, 1 h 20 m 01 s

```
ghcr.io/andrenijman/apex-os:apex-661a9d80d2239d848676d97e1e633d3f7325e853
  digest sha256:61f7935c259fa90362c09158fd355f32230e0ab21925340c3c2acf4f879bffcc
```

`:daily-661a9d80…` resolves to the same digest. Read out of the `image` job's
"Promote to every published tag" step, not constructed.

Every job's STEPS were read, not its conclusion: `core` 12/12 real steps
success, `base` 13/13, `image` 17/17. `installer-iso` and `qcow2` are
**skipped** because their dispatch inputs default false — the documented
"skipped counts as success" trap, checked rather than assumed. Only the
**daily** flavor was built (the dispatch default), which is the flavor katana
boots and the same shape as `apex-97c9e8f2`.

Four lines from the `image` job worth keeping:

```
modules: 14 out-of-tree, all signed by "APEX-OS Secure Boot key (andre)"
payload split OK: drivers in, gaming userspace out, glvnd intact
SBOM packages: 9830   file rows: 7049
signature and SBOM attestation both verify against
  .../build-image.yml@refs/heads/task/final-image
not publishing from refs/heads/task/final-image: per-SHA tags written
```

That last line is the one that matters for the boundary: **no floating tag
moved**, so the L16 cannot pick this image up through `apex update` by
accident. Only katana can see it, and only because it was pointed at it.

## katana baseline, taken 2026-09-20 ~05:10 AWST, BEFORE the switch

Booted `apex-97c9e8f25ee55593a97502505f51c6115ebbee7c`,
digest `sha256:55fc9e4ee9b2c1b27045cba5a594a40e97fb0d73c70170cfbe0003f5cd4d73b0`.

* Three deployments; the September 18 one still `Pinned: yes`. Rollback slot
  holds `apex-7f647470`.
* `efibootmgr -v` saved to the session scratchpad as `efibootmgr-before.txt`
  (16 lines, `BootCurrent: 0000`, `BootOrder: 0000,0001,0002,0003,0004,0005`).
  `Boot0000 APEX-OS Primary` is on the **Windows disk's** ESP — expected.
* Nothing running that a reboot would interrupt: no steam, no gamescope, one
  ssh session (mine) plus the greeter.
* `stat -c %C /etc/.pwd.lock` → `system_u:object_r:passwd_file_t:s0`.
* All **11** paths in `/var/lib/apex/pkg/etc.list` match `matchpathcon`. This is
  the hand-`restorecon`ed state from the last round. **It is a clean baseline,
  not proof of anything** — that is exactly why the reading after an install is
  the one that counts.
* `systemd-run --property=DynamicUser=yes … /usr/bin/id` → `result: success`.
  `systemctl --failed` empty. SELinux `enforcing`, policy `targeted`.
* `apex game status` has **no `scx_` field at all** → the running image predates
  the scheduler fix, which is §6.8's own way of telling the image is too old.
* `/sys/kernel/sched_ext/state` → `disabled`. `scx-scheds-1.1.3-3.fc43`,
  `scx_lavd` and `scx_rusty` both present, so §6.8 Rows A/B/C are all runnable.
* `PKG_COMPAT_LEVEL` is **3 in both engines** and `state.json` says the built
  extension is level 3 on fedora 43 — so the new image will **not** force a
  boot-time extension rebuild. That matters: it means no install has run when I
  first read the labels after the reboot.

## How to get it onto katana (from the last image unit, paid for)

`bootc switch` run straight over ssh is SIGHUPed when the channel closes, and
the Bash tool's 600 s cap closes it. Run it detached:

```sh
sudo systemd-run --unit=finimg-switch --collect \
  --property=TimeoutStartSec=infinity --service-type=oneshot \
  /usr/bin/bootc switch --transport registry <IMG>
```

`apex update` is not the route: katana tracks a per-SHA tag that never moves.

## The verification plan, and the ordering it depends on

**`/etc` labelling.** The landed fix self-heals on install, so a pass after an
install is ambiguous unless the machine was *broken first*. Order:

1. First thing after the reboot, before any `apex` command: `/etc/.pwd.lock`
   label, the `DynamicUser` probe, and whether a boot-time extension rebuild
   ran (it should not — see the compat level above).
2. Primary reading: ctime snapshot of `/etc` (files only — a directory's ctime
   moves when a child is written), `sudo apex install cronie vim-enhanced`
   (neither is in the image; cronie ships `/etc/cron.d`, which policy wants as
   `system_cron_spool_t`, and vim-enhanced ships `/etc/profile.d/*` → `bin_t`,
   so a `cp -a` mislabel would be visible), re-snapshot, and compare every
   written path's `stat -c %C` against `matchpathcon`.
3. Second reading, only after the first: `chcon -t rpm_var_lib_t
   /etc/.pwd.lock`, show the `DynamicUser` probe fails `217/USER`, install
   again, show it passes. **Stage `restorecon -F /etc/.pwd.lock` as the undo
   before the `chcon`** — `rpm-ostreed` is itself a `DynamicUser=yes` unit and
   this machine needs it.
4. `rpm-ostreed.service` and `capsule@.service` start clean afterwards.

**The scheduler.** `docs/gaming-and-sessions.md` §6.8 Rows A/B/C, all three
headless (`sudo apex game start` / `stop`; Row C needs
`sudo scxctl start -s scx_rusty` first). Record `root/ops` **verbatim** —
`lavd` is expected and has never been checked on hardware. `not loaded` with a
true reason is a PASS; a claim of `loaded` against a `disabled`
`/sys/kernel/sched_ext/state` is the only failure. §6.6 needs a real session
and a greetd edit and is **not** attempted here.

## NEXT

Build was in progress when this was written. If you are picking this up cold:
read run 35469530380's job steps, then follow the two sections above in order.

## Status

IN PROGRESS — build dispatched 2026-09-19T21:08:57Z, baseline taken.

# unit: katana-runner — katana is a self-hosted runner, on its own 500 GB partition

**Status: DONE.** The partition surgery is finished and verified, the runner is
installed, containment is tested rather than assumed, and a kernel build was
started on it. Nothing landed. No PR.

Repo `apex-os`, branch **`task/katana-runner`**, worktree
`/var/tmp/apex-work/wt-katana-runner`. Branched from `origin/roadmap/v2.2` at
`ec7b3ccf`. All commits pushed.

```
79ac97f5 docs(evidence): what was done to katana's disk and what the runner may run
5e0e0da2 ci(kernel): build the kernel on katana, because 100 GB does not fit in 14 GB
3c9f5e86 ci(katana): prove what a self-hosted job cannot reach, rather than assuming it
```

**The machine changes are NOT in this branch and cannot be.** They are recorded
in full, with a removal recipe, at
`ROADMAP/evidence/katana-runner-20260920.md`. Read that before touching katana.

## The partition

SPCC 2 TB, **serial `240023925111005`** — always address it by serial, the
`nvme0n1`/`nvme1n1` names have swapped across ordinary reboots.

| | before | after |
|---|---|---|
| p1 ESP (APEX Boot0000 + Windows Boot Manager) | 2048–411647 | **untouched** |
| p2 Microsoft reserved | 411648–444415 | **untouched** |
| p3 NTFS — Windows | 444416–1049020415 | **untouched** |
| p5 ext4 `games` | 1049020416–3905296383 (1362 GiB) | 1049020416–**2856720383** (862 GiB) |
| **p6 ext4 `apexdev`** | — | **2856720384–3905296383 (500 GiB)** |
| p4 NTFS recovery | 3905296384–3907026943 | **untouched** |

`p4 is physically last`, not p5 — p5 is last only by partition number. The new
partition fills the gap between them.

- `games` kept PARTUUID `F707EB10-…` and fs UUID `a41fee21-…`; its fstab line
  never changed. 59 GB used of 862 GiB.
- `apexdev` fs UUID **`8c8746e1-c0ae-4c3c-a16b-2a6164423ffa`**, mounted
  **`/var/lab`** by UUID with `nofail`. 484 GB free.
- Table backup: `/var/lib/apex/katana-runner-20260920/gpt-spcc.before.bin`, on
  the **other** disk. Undo is `sgdisk --load-backup=… <disk>` then `resize2fs`.

Proof, not assertion: sha256 of all 1041 regular files before and after →
**diff empty**; file listing → diff empty; `efibootmgr -v` before/after →
`cmp` identical; `sfdisk -d` lines for p1–p4 → byte-identical.

## The runner

`katana`, actions/runner 2.337.0, labels
`self-hosted,Linux,X64,katana,apex-builder`, unit `apex-github-runner.service`
(enabled), user **`ghrunner` uid 960 — no sudo, no groups**.

- **Performance is not capped.** No `CPUQuota`, no `MemoryMax`; a job gets all
  20 cores and 62 GiB. `IOWeight=50` + `OOMScoreAdjust=300` protect Andre's
  session instead of throttling his builds.
- **Ephemeral.** `.runner` says `"ephemeral": true`. Re-registered before every
  job by a root-only `ExecStartPre=+`, so the job never sees the credential.
- **apex-os is PUBLIC.** The control that keeps fork code off the machine is the
  repo setting `fork-pr-contributor-approval` = **`all_external_contributors`**
  (changed from `first_time_contributors` on 2026-09-20). The `if:` guard in
  each workflow is only a belt — a fork controls the workflow file on a
  `pull_request` event. **Never add `pull_request_target` to a self-hosted job.**
- `AndreNijman` is a **User, not an org**, so runner groups with a repository
  allowlist are not available. Repo-level registration is the only option.
- A uid-scoped nft rule (`table inet apex_runner`) denies uid 960 all of RFC1918
  except `192.168.1.1:53`. Andre's own uid is unaffected (verified: he gets
  HTTP 200 from the router at the same time the runner is rejected).

Containment run **35517909892** is green with all 16 forbidden actions denied
and all 4 positive controls passing, and the `deny` harness was separately shown
to go red when handed a leak.

## NEXT — for a stranger

1. **Check whether the kernel build finished.** Run **35518017589** on branch
   `task/katana-runner` was `in_progress` at the `compile` step when this unit
   ended — the first real kernel build on katana, `-j20` via the new
   `KERNEL_BUILD_JOBS` ARG. `gh run view 35518017589`. If it went red, the
   likely causes in order: rootless podman + `--isolation=chroot` disagreeing
   with something `rpmbuild` wants; disk (the guard step demands 150 GB free
   and there were 478 GB); or `-j20` on 62 GiB, in which case drop
   `KERNEL_BUILD_JOBS` back toward 12 in `.github/workflows/kernel-build.yml`
   — **do not** change the Containerfile default, which is deliberately 12.
   Re-run with `gh workflow run kernel-build.yml --ref task/katana-runner`.
2. **Land it.** `git merge-tree origin/roadmap/v2.2 HEAD` → **exit 0, no
   conflicts**, and the branch is **0 behind / 4 ahead** of
   `origin/roadmap/v2.2` (checked at `f42ed421`). Land via
   `/var/tmp/apex-work/int-os`, not a main checkout. Landings are merges, not
   rebases. The only file here that belongs to another unit's history is
   `Containerfile.kernel`, and the change to it is additive: a
   `KERNEL_BUILD_JOBS` ARG whose default (12) reproduces the previous
   behaviour exactly.
3. **Replace the stored credential.** `/etc/apex-runner/gh-token` holds Andre's
   broad `gh` OAuth token (every repo he owns) because an ephemeral runner must
   re-register and that needs an admin credential. A fine-grained PAT scoped to
   `apex-os` with `Administration: write` replaces it; that needs Andre's
   browser. Rotate by writing the new token to that file and
   `systemctl restart apex-github-runner`.
4. **A reboot test was never run.** Andre had a live session and other units
   were using the box. `findmnt --verify --fstab` is clean, `var-lab.mount` is
   active, and the runner unit has `RequiresMountsFor=/var/lab/runner` — but
   nobody has actually rebooted katana and watched `/var/lab` and the runner
   come back.
5. Optional: the runner is registered to `apex-os` only. `apex-shell` has its
   own CI and would need its own registration if it ever wants katana.

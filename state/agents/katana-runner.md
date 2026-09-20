# unit: katana-runner — katana is a self-hosted runner, on its own 500 GB partition

**Status: DONE, and the headline is that APEX'S KERNEL NOW BUILDS IN CI.** The
partition surgery is finished and verified, the runner is installed, the
containment is tested rather than assumed, and **a real kernel build ran on
katana and passed end to end** — run **35518017589**, `rpmbuild took 34m23s at
-j20`, `btf_scx=usable`, both BTF readers agreeing, and
`tests/check-kernel-contract.sh` at **`fail=0`** against the RPMs that build
produced. That closes the question `docs/update-cost.md` said this tier could
not answer for itself. Nothing landed. No PR.

Repo `apex-os`, branch **`task/katana-runner`**, worktree
`/var/tmp/apex-work/wt-katana-runner`. Branched from `origin/roadmap/v2.2` at
`ec7b3ccf`. All commits pushed.

```
ae633df8 docs(evidence): APEX's kernel built on katana in 34m23s, and every gate passed
c5710aa5 docs(evidence): what was checked about rootless podman, and one warning that is a choice
047b938a docs(evidence): the backup directory is the undo, not a leftover
b4f8b8b1 docs(evidence): record the measured OOM and cap numbers, not the intent
81f6a24f docs(evidence): say which of the three fork-PR layers is load-bearing
c06a48a0 ci(katana): three hardening fixes found by reading the files again
ea8b42f8 docs(update-cost): answer the CI question the kernel tier could not answer
f42ed421 docs(evidence): cite .runner for ephemeral, not just the behaviour
79ac97f5 docs(evidence): what was done to katana's disk and what the runner may run
5e0e0da2 ci(kernel): build the kernel on katana, because 100 GB does not fit in 14 GB
3c9f5e86 ci(katana): prove what a self-hosted job cannot reach, rather than assuming it
```

Diff against `roadmap/v2.2` is five files and nothing else:
`.github/workflows/katana-probe.yml` (new), `.github/workflows/kernel-build.yml`
(new), `ROADMAP/evidence/katana-runner-20260920.md` (new),
`docs/update-cost.md`, and `Containerfile.kernel` — the last being the only
file belonging to another unit's history, changed additively (a
`KERNEL_BUILD_JOBS` ARG defaulting to 12, which reproduces the old behaviour
exactly). Nothing touches `installer/**`, `files/desktop/**`, the shell, or any
boot stanza.

**`c06a48a0` was committed after the kernel run started and is not in the code
that run executed** — pushing it would have cancelled the build via
`cancel-in-progress`. It changes the contract step's `RPMS=` derivation and the
workflow's push-branch filter, so the next kernel run is the first to exercise
them. Its probe half *was* exercised: pushing it re-ran `katana-probe.yml` as
run **35520119201**, green, with all **17** containment assertions denied —
`pkexec` among them now.

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
- **apex-os is PUBLIC.** Three layers, and only one is load-bearing:
  **neither** self-hosted workflow has a `pull_request` trigger, so an ordinary
  fork PR does not start them and a contributor's experience is unchanged (no
  red check from a skipped job — they still get `pr-validation.yml` on
  `ubuntu-24.04`); the `if: … head.repo.full_name == github.repository` guard
  catches a future edit that adds one; and **the control** is the repo setting
  `fork-pr-contributor-approval` = **`all_external_contributors`** (changed from
  `first_time_contributors` on 2026-09-20), because on a `pull_request` event a
  fork controls the workflow file and can add the trigger, point `runs-on` at
  `katana` and delete the guard in one commit.
  **Never add `pull_request_target` to a self-hosted job.**
- `AndreNijman` is a **User, not an org**, so runner groups with a repository
  allowlist are not available. Repo-level registration is the only option.
- A uid-scoped nft rule (`table inet apex_runner`) denies uid 960 all of RFC1918
  except `192.168.1.1:53`. Andre's own uid is unaffected (verified: he gets
  HTTP 200 from the router at the same time the runner is rejected).

Containment grew as holes were found: 16 assertions, then 17 with `pkexec`, now
**21**. Latest run **35520468567**, green, all 21 refused, 4 positive controls
passing. The `deny` harness was separately shown to go red when handed a leak,
so it is capable of failing.

**One of those holes was real and this unit had claimed the opposite.** The top
level of the runner tree was `0775 root:ghrunner`, and directory write lets you
rename or unlink entries whatever their ownership — so `touch bin/pwned` was
refused (what the probe checked) while `mv bin bin.x && cp -r bin.x bin` was
not. Measured: *"LEAK: ghrunner CAN replace bin/"*. That is persistence across
the ephemeral boundary. Fixed with `chmod 1775` plus a root-owned `.root-decoy`
the probe now tries to rename every run. **`svc.sh` must stay ghrunner-writable**
— `config.sh` rewrites it on every registration, and root-owning it killed the
ephemeral loop until that was undone. Second hole, same shape: this unit's own
backup directory (holding his Steam Proton prefix) was 0755; now 0700 and in
`InaccessiblePaths=`.

Final state, after three probe jobs and a 37-minute kernel build: runner
`online busy=false`, three processes (run.sh, run-helper.sh, Runner.Listener)
and **no stray job process**, `/var/lab` back to 3.1 GB of 492 GB, `games`
unchanged at 59 GB of 848 GB, and the nft counter at 13 rejected packets.

## NEXT — for a stranger

1. **Re-run the kernel build once, because the run that passed did not contain
   commit `c06a48a0`.** That commit was deliberately held back while the build
   was in flight (pushing it would have cancelled the run via
   `cancel-in-progress`), and it changes two things the green run never
   exercised: the contract step's `RPMS=` derivation, and the workflow's
   push-branch filter. The old derivation used `find … | head -1` under
   `pipefail`, which is the 141-on-success trap — it happened to survive with
   only 5 rpms, but it is now written without the pipeline. One
   `gh workflow run kernel-build.yml --ref task/katana-runner` confirms the
   rewritten step. Not urgent; the kernel itself is proven.
2. **Land it.** `origin/roadmap/v2.2` moved 27 commits while this unit ran, so
   it was **merged in** rather than left as a stale "0 behind" claim: the merge
   was clean, it touched none of this unit's five files, and the branch is now
   **0 behind / 12 ahead**. Land via
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
5. **`kernel-build.yml` is a proof, not the producer, and a green run must not
   be read as "the kernel tier ships from CI".** It builds
   `localhost/apex-kernel:ci` and publishes nothing, while `Containerfile.core`
   consumes `ghcr.io/andrenijman/apex-os:kernel@sha256:…`. Closing that needs
   `packages: write`, a `podman login` step and a registry push credential on
   the build host — a separate decision, deliberately not taken here.
   `docs/update-cost.md` now says this next to the question it answers.
6. **The new partition relieves `apex-root` only if something moves onto it,
   and so far only the runner has.** Measured while this unit ran: katana's
   `apex-root` is **95 % full (900 G of 954 G)** and `/var/home/andre` is
   **841 GB** of that — `.local` 506 GB (Steam, plus 41 GB of his podman
   storage), `Projects` 117 GB, `Pictures` 84 GB, `win-backup` 43 GB,
   `.ollama` 26 GB, `.npm` 22 GB, `bootlab-work` 19 GB, and 18 GB of build
   scratch in `/var/tmp` (`apex-build` alone is 12 GB). **None of it was
   moved** — it is Andre's data and relocating it was not what this unit was
   asked to do. The safe, obviously-development candidates, if he wants them
   moved onto `/var/lab`: `/var/tmp/apex-build` and the `p0-004*`/`apex-p0021`
   scratch trees, `~/bootlab-work`, and his rootless podman graphroot (a
   `~/.config/containers/storage.conf` `graphroot` change plus a
   `podman system reset`). `~/.npm` is cache and is simply purgeable.
7. Optional: the runner is registered to `apex-os` only. `apex-shell` has its
   own CI and would need its own registration if it ever wants katana.

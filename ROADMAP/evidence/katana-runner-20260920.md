# katana as a self-hosted runner — what was done to the machine, 2026-09-20

Andre: *"ok use the katana as a full self hosted runner from now on. all resources
on katana permitted, use the second drive, the 2 tb one, create a 500gb partition
just for this and everything apex and other development related."*

Clarified mid-unit: *"i mean all system resources as in performance, make sure
its properly restricted so my system cant just be hijacked or whatever. but
people should still be able to PR and stuff."*

**None of this is in the image.** It is machine state on katana. Everything
needed to undo it is below, and every artefact named here lives in
`/var/lib/apex/katana-runner-20260920/` — on the **other** disk (the Micron
`apex-root`), deliberately, so the undo does not live on the disk being changed.

---

## 1. The disk

Device names on katana are **not stable** — `nvme0n1`/`nvme1n1` have swapped
across ordinary reboots. Everything below addresses the disk by serial:

```
/dev/disk/by-id/nvme-SPCC_M.2_PCIe_SSD_240023925111005    # the "2 TB" drive
/dev/disk/by-id/nvme-Micron_2450_MTFDKBA1T0TFK_220534D1CB81   # apex-root
```

### Before

```
Disk 3907029168 sectors × 512 B, GPT 3B36957D-4A72-46D7-B13B-FCDEEBD4102F
#   Start           End        Size     Code  Name
1        2048        411647    200 MiB  EF00  ESP    (APEX Boot0000 + Windows Boot Manager)
2      411648        444415     16 MiB  0C01  Microsoft reserved
3      444416    1049020415    500 GiB  0700  NTFS — Windows itself
5  1049020416    3905296383   1362 GiB  8300  ext4 "games"   (62.5 GB used, 5 %)
4  3905296384    3907026943    845 MiB  2700  NTFS — Windows recovery
last usable sector 3907029134
```

**A correction to the dispatch note, because it changes nothing but would
mislead the next person:** p5 is last by partition *number*, but **p4 is last by
sector** — the recovery partition sits at the very end of the disk, after
`games`. So the freed space is a *gap between the new p5 and p4*, not space at
the end of the disk. That is equally safe (nothing moves, no existing entry
changes), and it is why the new partition's end is pinned to `3905296383`
rather than to the last usable sector.

### After

```
#   Start           End        Size     Code  Name
1        2048        411647    200 MiB  EF00  ESP              UNTOUCHED
2      411648        444415     16 MiB  0C01  Microsoft res.   UNTOUCHED
3      444416    1049020415    500 GiB  0700  NTFS Windows     UNTOUCHED
5  1049020416    2856720383    862 GiB  8300  ext4 "games"     shrunk
6  2856720384    3905296383    500 GiB  8300  ext4 "apexdev"   NEW
4  3905296384    3907026943    845 MiB  2700  NTFS recovery    UNTOUCHED
```

* `games` kept its partition GUID `F707EB10-1714-4991-87F6-B614A3A051DC` and its
  filesystem UUID `a41fee21-c288-44a5-804a-4f0b6817428e`, so its existing
  `/etc/fstab` line did not have to change. 59 GB used of 862 GiB.
* New partition 6: PARTUUID `BAB2891F-F411-468B-93D2-FA4D90A8778C`,
  filesystem UUID **`8c8746e1-c0ae-4c3c-a16b-2a6164423ffa`**, label `apexdev`.
* p6 starts at 2856720384 = 2048 × 1394883, so it is aligned.

### The undo

```
sgdisk --load-backup=/var/lib/apex/katana-runner-20260920/gpt-spcc.before.bin \
       /dev/disk/by-id/nvme-SPCC_M.2_PCIe_SSD_240023925111005
resize2fs /dev/disk/by-id/nvme-SPCC_M.2_PCIe_SSD_240023925111005-part5   # back to 1362 GiB
```

`gpt-spcc.after.bin`, `sfdisk.before.txt`, `sfdisk.after-table.txt`,
`sgdisk-info.before.txt`, `gpt-first-34-sectors.before.bin` and `fstab.before`
are in the same directory.

### What made it safe

The whole operation ran as a **transient systemd unit** (`kr-shrink.service`),
not over ssh — a resize interrupted by a dropped connection is a corrupt
filesystem, and the agent's own tooling has a 600 s ceiling. The script is kept
at `/usr/local/sbin/kr-shrink.sh`; its log is `shrink.log`.

Every step was gated on a **number**, and each gate aborts before the next
destructive step:

| gate | assertion | result |
|------|-----------|--------|
| A | disk is 3907029168 sectors, p5 is 2856275968, p6 does not exist | ok |
| B | `efibootmgr -v` mentions ESP `2ba9a2ea-…` **before** any change | ok |
| — | `e2fsck -f -y` before the shrink | rc=0, clean |
| — | `resize2fs` to 855 G, then block count **≤ 225962496** | 224133120 ✓ |
| — | `e2fsck -f -y` again before touching the table | rc=0 |
| C | `blockdev --getsz` p5 = 1807699968, p6 = 1048576000 | ok |
| C | `sfdisk -d` lines for **p1–p4 byte-identical** before/after | ok |
| D | `efibootmgr -v` still mentions ESP `2ba9a2ea-…` | ok |
| — | grow to fill; final block count **= 225962496** | ok |
| — | `e2fsck -f -y` final | rc=0 |
| E | games filesystem UUID and LABEL unchanged | ok |

**Data.** The partition held exactly one thing: Steam's *Bodycam* (appid
2406770), 56 GB, re-downloadable, plus a 587 MB Proton prefix that is not.
The prefix and the app manifests were archived to
`steam-compatdata-and-manifests.tar.zst` (239 MB, sha256 in
`steam-backup.sha256`) on the other disk before anything was unmounted.

The game payload was **not** copied anywhere — there is nowhere to put 56 GB
(apex-root had 51 GB free, which is the whole reason this unit exists). What
replaced a copy is a full integrity proof: **sha256 of all 1041 regular files
plus a size-and-type listing of all 2423 entries, taken before and after.**

```
games-sha256.before.txt  ==  games-sha256.after.txt     (diff empty)
games-filelist.before.txt == games-filelist.after.txt   (diff empty)
efibootmgr.before.txt    ==  efibootmgr.after.txt       (cmp identical)
```

`e2fsck` reported the filesystem clean at every one of the three passes, and
the shrink relocated nothing because all 62.5 GB already sat below the new
boundary.

### The mount

```
/etc/fstab (appended, following the existing `games` line exactly):
UUID=8c8746e1-c0ae-4c3c-a16b-2a6164423ffa  /var/lab  ext4  defaults,noatime,nofail  0  2
```

`findmnt --verify --fstab` → *Success, no errors or warnings detected*.
`restorecon -RvF /var/lab` relabelled it from `unlabeled_t` to `var_t`.

```
/var/lab            0755 root:root        the mount point
/var/lab/runner     0750 ghrunner:ghrunner  runner home + container graphroot
/var/lab/runner/_work 0700 ghrunner       job workspace, wiped before every job
/var/lab/scratch    0750 andre:andre      Andre's dev space — the runner cannot read it
```

**No reboot test was performed** — Andre has a live session and other units are
using the box. What was checked instead: `findmnt --verify --fstab` (catches the
typo that only bites at boot), the generated `var-lab.mount` is `active`, and
the runner unit carries `RequiresMountsFor=/var/lab/runner`.

---

## 2. The runner

`apex-os` is a **PUBLIC** repository (0 forks at the time of writing). GitHub's
own guidance is that a self-hosted runner on a public repo is dangerous by
default, so the containment below is the reason this is acceptable at all.

**The owner `AndreNijman` is a User account, not an organisation**, so runner
groups with a repository allowlist are not available. Repo-level registration is
the only option; this is a constraint, not a choice that was made.

### What is installed

| thing | where |
|---|---|
| runner | `actions/runner` **v2.337.0**, `/var/lab/runner/actions-runner` |
| name / labels | `katana` — `self-hosted, Linux, X64, katana, apex-builder` |
| unit | `/etc/systemd/system/apex-github-runner.service` (enabled) |
| re-registration | `/usr/local/sbin/apex-runner-register` (root, 0700) |
| credential | `/etc/apex-runner/gh-token` (root, 0600) |
| egress rule | `/etc/apex-runner/nftables.conf` + `apex-runner-firewall.service` |
| user | `ghrunner`, uid 960, **no sudo, no wheel, no groups** |
| subuid/subgid | `ghrunner:1000000:65536` (clear of andre's 524288) |

### Performance: the box is not throttled

Andre asked for the machine's full performance, so the unit has **no `CPUQuota`
and no `MemoryMax`**. A job sees all 20 cores and all 62 GiB. The only limits
present protect his interactive session rather than capping his builds:

* `IOWeight=50` — the build yields disk bandwidth, not CPU.
* `OOMScoreAdjust=300` — under memory pressure the **build** is killed, not his
  desktop.

Measured during the first real kernel build rather than assumed:

```
CPUQuotaPerSecUSec=infinity   MemoryMax=infinity   MemoryHigh=infinity
AllowedCPUs=(empty, i.e. all)  IOWeight=50  OOMScoreAdjust=300

pid 130012  adj=300  Runner.Listener
pid 239413  adj=500  cc1          <- the compile inherits it and podman adds more
pid 240009  adj=500  cc1
pid 240024  adj=500  cc1
18 cc1 processes on 20 cores; load average 20.6
```

Andre's own processes sit at the default `oom_score_adj=0`, so every one of
these dies before anything of his does. That is the whole of the "protect his
session" mechanism — there is no cap anywhere on how fast the build may go.

`Containerfile.kernel`'s compile parallelism became a build ARG in the same
branch: default stays 12 (the value it was measured at on a 29 GiB box), and CI
passes `$(nproc)` = 20 here.

### Security: the job is assumed hostile

1. **Fork pull requests never reach the machine.** Three layers, and it matters
   which one is load-bearing:
   * **Neither self-hosted workflow has a `pull_request` trigger at all.**
     `katana-probe.yml` fires on `workflow_dispatch` and on a push touching
     itself; `kernel-build.yml` on `workflow_dispatch` and on pushes to `main`
     or `roadmap/**`. So an ordinary fork PR does not start either of them, and
     a contributor's PR experience is completely unchanged — they get
     `pr-validation.yml` on `ubuntu-24.04` exactly as before, with no red check
     from a skipped self-hosted job.
   * The `if: … head.repo.full_name == github.repository` guard on each
     self-hosted job, which catches the case where somebody later adds a
     `pull_request` trigger and forgets.
   * **The control**: the repository setting
     `actions/permissions/fork-pr-contributor-approval`, changed from
     `first_time_contributors` to **`all_external_contributors`** on
     2026-09-20. This is the one that actually holds, because on a
     `pull_request` event GitHub runs the workflow file **from the fork's
     head** — a fork can add a `pull_request` trigger, point `runs-on` at
     `katana` and delete the `if:` line, all in its own copy of the file. What
     stops that is that nothing from an external contributor runs at all until
     Andre approves it.

   **Never give a self-hosted job a `pull_request_target` trigger**: it runs
   the fork's code with the base repository's secrets and permissions, which is
   the exact combination that turns a pull request into remote code execution.
   `grep -rn pull_request_target .github/workflows/` returns only the comments
   forbidding it.
2. **Ephemeral.** `config.sh --ephemeral --replace --disableupdate`, re-run
   before every job by `ExecStartPre=+/usr/local/sbin/apex-runner-register`,
   which runs **as root** so the credential that mints registration tokens is
   never readable by the job. `_work` is deleted and recreated each time.
   `--disableupdate` means the runner cannot rewrite its own `bin/`; version
   bumps are deliberate.
3. **Unprivileged and blind to Andre's data.** `ghrunner` is in no sudoers rule
   and no group. `/var/home/andre` is 0700 and additionally listed in the unit's
   `InaccessiblePaths=`, along with `/root`, `/var/roothome` and
   `/var/lab/scratch`. The runner tree is root-owned with `bin/` and
   `externals/` read-only; only the top level and `_work` are writable.
4. **No route onto his LAN.** `table inet apex_runner` — a *separate* nft table
   so `apex-firewall.service` can rebuild its own without taking this with it —
   rejects uid 960 to `10/8`, `172.16/12`, `192.168/16` and `fc00::/7`, with a
   single exception for `192.168.1.1:53` so rootless containers can still
   resolve. Rootless podman's pasta emits packets as the uid, so containers are
   covered by the same rule. That closes the concrete hijack path: the Synology,
   the Orange Pi that hosts the memory vault, the router admin page, the L16.

`NoNewPrivileges=` is deliberately **not** set: rootless podman needs the file
capabilities on `newuidmap`/`newgidmap`. What stands in for it is that
`ghrunner` has no sudoers entry and no group membership at all.

### It was tested, not assumed

`.github/workflows/katana-probe.yml` attempts every forbidden thing from inside
a real job and fails the run if any succeeds. Run **35517909892**, all green:

```
denied: sudo -n true                    denied: list his games library
denied: su root                         denied: list his dev scratch
denied: list his home                   denied: read the token that registers the runner
denied: read his ssh dir                denied: write into the runner's bin/
denied: read his known_hosts            denied: edit the runner's systemd unit
denied: read his gh credentials         denied: reach the router
denied: list his APEX worktrees         denied: reach this host over the LAN
                                        denied: reach 10/8
                                        denied: ssh anywhere on the LAN
ok: resolve DNS · reach the internet · write the workspace · run a container
cpus=20  mem=62GiB   /dev/nvme1n1p6  492G  2.8G  484G  1% /var/lab
```

Three further checks, because a probe that cannot fail proves nothing:

* **Negative control.** The same `deny` harness was run with an assertion that
  always succeeds (`deny "…" true`). It reported `LEAK` and exited 1. The
  harness does detect a leak.
* **Negative control on the firewall.** Andre's own uid reaches
  `http://192.168.1.1/` and gets HTTP 200 from the same machine at the same
  time. The rule is uid-scoped, not a global block, and it has not changed
  anything for him.
* **Ephemeral cleanup.** The job deliberately double-forked a `sleep 9999` out
  of its own process tree. After the job: no `ghrunner` process survived,
  `_work` was empty, and the journal shows a fresh registration. `KillMode=`
  control-group plus no logind session for `ghrunner` means there is nowhere
  for a process to hide.

The GitHub API reports `ephemeral=null` for this runner. That is a reporting
gap, not a configuration one, and it is worth knowing about before somebody
reads it as "the `--ephemeral` flag did not take". Two things say otherwise:
`/var/lab/runner/actions-runner/.runner` contains `"ephemeral": true` and
`"disableUpdate": true`, and the listener **exits after one job** and is
re-registered, which a non-ephemeral runner does not do.

### Which jobs target it

**Only the kernel build.** `Containerfile.kernel` needs a ~100 GB tree and a
hosted `ubuntu-24.04` runner has 14 GB — that impossibility is the reason this
unit exists.

Everything else **stays on `ubuntu-24.04` on purpose**: `pr-validation.yml`,
`build-image.yml`, `boot-v2.yml` and `release-shell.yml` are unchanged. The
hosted runner is a second environment and it keeps finding defects a developer
machine cannot (`ROADMAP/evidence/` has a whole family of them). Moving them
here would trade that away for minutes.

`kernel-build.yml` triggers on `workflow_dispatch` and on pushes touching
`kernel/**`, `Containerfile.kernel` or the workflow itself, with a `concurrency`
group so two pushes cannot queue two 100 GB builds.

---

## 3. Known costs and follow-ups

* **The stored credential.** `/etc/apex-runner/gh-token` holds Andre's `gh` CLI
  OAuth token (`repo`, `workflow`, `gist`, `read:org` — every repo he owns),
  root-readable only. An ephemeral runner *must* re-register before each job and
  that needs an admin credential; there is no way around storing one. **Replace
  it with a fine-grained PAT scoped to `apex-os` alone with `Administration:
  write`** — that needs Andre's browser, so it could not be done here. Rotating
  it is one line: write the new token to that file, `systemctl restart
  apex-github-runner`.
* **Ephemeral does not clear the container graphroot.** `_work` is wiped, but
  `/var/lab/runner/.local/share/containers/storage` persists so the ~2 GB
  builder base is not re-pulled every run. That is a deliberate trade and it is
  only acceptable *because* fork code never reaches this runner.
* **`/run/apex-runner` carries over between jobs too.** `RuntimeDirectoryPreserve=yes`
  keeps podman's runtime directory across restarts rather than making it
  re-initialise every job. Same carry-over class as the graphroot, same
  justification, and it is worth knowing rather than discovering.
* **`ghrunner` took uid/gid 960 from `useradd --system`**, which is inside the
  range a future image's `sysusers.d` could allocate to something else. It is
  not pinned. If the image ever ships a system user that collides, the
  ownership on `/var/lab/runner` is what will look wrong.
* **`apex-firewall.service` was checked, not assumed.** Its `ExecStart` is
  `nft -f /usr/share/apex/nftables/apex.nft` plus `apex-firewall reload`, and
  its `ExecStop` is `nft delete table inet apex`. Every operation in that file
  and in `/usr/libexec/apex-firewall` is **scoped to `table inet apex`** — a
  `delete table inet apex`, and `nft flush set $TABLE …` for its port sets.
  **There is no `flush ruleset` anywhere in it**, so a restart of APEX's
  firewall cannot take `table inet apex_runner` with it. If that ever changes,
  the symptom is silent — the check is
  `nft list table inet apex_runner`, and its `counter` should be non-zero after
  the probe workflow runs.

## 4. Everything created on katana, for a clean removal

```
systemctl disable --now apex-github-runner.service apex-runner-firewall.service
rm /etc/systemd/system/apex-github-runner.service \
   /etc/systemd/system/apex-runner-firewall.service
rm -rf /etc/apex-runner
rm /usr/local/sbin/apex-runner-register /usr/local/sbin/kr-shrink.sh \
   /usr/local/sbin/kr-manifest.sh /usr/local/sbin/kr-setup-runner.sh
semanage fcontext -d '/var/lab/runner/actions-runner(/.*)?'
userdel -r ghrunner            # also drop its /etc/subuid, /etc/subgid lines
umount /var/lab                # then remove the /etc/fstab line
```
Deregister the runner in the repository's Settings → Actions → Runners, and put
`fork-pr-contributor-approval` back to `first_time_contributors` if that is
wanted. The partition itself is undone with the `sgdisk --load-backup` above.

**Do not delete `/var/lib/apex/katana-runner-20260920/`** — it is the undo, not
a leftover. It carries its own `README.md`, which the runner unit's
`Documentation=` points at, so `systemctl cat apex-github-runner` leads a
stranger to it. It is ~240 MB, almost all of it the Steam prefix archive; the
archive is the only part that is safe to drop once Andre confirms his save data
is fine.

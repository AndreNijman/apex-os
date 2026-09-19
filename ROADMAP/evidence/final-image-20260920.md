# final-image — the image built from the v2.2 tip, and the two fixes on katana

2026-09-20, unit `final-image`, item **P0-001**.

`roadmap/v2.2` was at `661a9d80` with zero unlanded work, and the last qualified
image (`apex-97c9e8f2`, the one katana was running) predated the final two
commits. This unit built an image from the tip and took the readings that only a
real machine can give.

**Neither the L16 nor `main` was touched.** No merge, no PR, no floating tag.

---

## 1. The artifact

Run **35469530380** — `workflow_dispatch`, `build-image.yml`, ref
`task/final-image` @ `661a9d80`. Queued 2026-09-19T21:08:57Z, **`success` at
22:29Z, 1 h 20 m 01 s**.
<https://github.com/AndreNijman/apex-os/actions/runs/35469530380>

```
ghcr.io/andrenijman/apex-os:apex-661a9d80d2239d848676d97e1e633d3f7325e853
  digest sha256:61f7935c259fa90362c09158fd355f32230e0ab21925340c3c2acf4f879bffcc
```

`:daily-661a9d80…` resolves to the same digest. Both strings were read out of
the "Promote to every published tag" step, not constructed from the sha.

Every job's **steps** were read, not its conclusion — the documented
"skipped counts as success" trap: `core` 12/12 real steps success, `base` 13/13,
`image` 17/17. `installer-iso` and `qcow2` are `skipped` because their dispatch
inputs default false. Only the **daily** flavor was built, which is the flavor
katana boots and the same shape as `apex-97c9e8f2`.

Four lines worth keeping:

```
Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/final-image): 03d77f96f521df7501d710d5c15ae8d0074a39ce
modules: 14 out-of-tree, all signed by "APEX-OS Secure Boot key (andre)"
payload split OK: drivers in, gaming userspace out, glvnd intact
SBOM packages: 9830   file rows: 7049
not publishing from refs/heads/task/final-image: per-SHA tags written
```

The shell pin is the same sha `apex-97c9e8f2` carried; the ordered fallback
worked and **this is not a main-shell build**. The last line is the boundary
holding: **no floating tag moved**, so the L16 cannot take this image through
`apex update` by accident.

`core rebuild: true (core sources changed)` is **not** a real core source
change — the diff `97c9e8f2..661a9d80` touches no `Containerfile*` and no
`kernel/**`. `dorny/paths-filter` on a first dispatch of a brand-new branch has
no base to diff against and reports everything changed. The cost is ~45 min of
build and a whole-image (~6 GB) pull for katana. The previous image run behaved
identically.

Onto katana with
`bootc switch --transport registry <tag>`, run as a **detached transient unit**
(`systemd-run --unit=finimg-switch --collect --property=TimeoutStartSec=infinity
--service-type=oneshot`) because a `bootc switch` run straight over ssh is
SIGHUPed when the tool's 600 s cap closes the channel. Staged in one pass,
`Consumed 2min 9.694s CPU time, 2.9G memory peak`. Clean `systemctl reboot`;
booted 2026-09-20 06:45:11 AWST.

---

## 2. `apex install` no longer mislabels the files it writes into /etc — **CONFIRMED**

### 2.1 The baseline, and why a clean baseline proves nothing on its own

Taken on `apex-97c9e8f2` before the switch, and again on `apex-661a9d80`
**before any `apex` command had run**:

| reading | before the switch | first thing after the reboot |
|---|---|---|
| `stat -c %C /etc/.pwd.lock` | `system_u:object_r:passwd_file_t:s0` | same |
| all 11 paths in `etc.list` vs `matchpathcon` | 11/11 match | 11/11 match |
| `DynamicUser=yes` probe | `result: success` | `result: success` |
| `systemctl --failed` | empty | empty |
| `apex game status` `scx_` fields | **absent** | present |

Katana's `/etc` had been `restorecon`ed by hand during the previous round, so
**that clean reading is not evidence of anything**. It is the starting line.

Two things establish that no install had silently run first: `state.json` still
read `built: 2026-09-19T20:09:00Z`, `image_sha256: 75290ae7…` — unchanged from
before the reboot — and `PKG_COMPAT_LEVEL` is **3 in both engines**, so
`apex-sysext-rebuild.service` found a match at boot and no-oped. The
self-healing property of the landed fix therefore had no chance to fire before
the measurement.

### 2.2 The primary reading: 56 written paths, 55 exact

`sudo apex install nginx` — the path a person actually takes,
`unconfined_u:unconfined_r:unconfined_t`. The population is a **before/after
ctime snapshot of `/etc`**, files and symlinks only (a directory's ctime moves
when a child is created inside it, so directories are counted only when they are
new). Each path's `stat -c %C` is then compared against `matchpathcon` — the
**loaded policy on this machine**, not Fedora's policy rpm, which is one of the
three questions the container suite structurally could not answer.

```
WRITTEN PATHS: 56
MISMATCHED: 1
WANTING A NON-etc_t TYPE: 40
```

nginx was chosen for what policy says about the `/etc` it ships, not for what it
is: `httpd_config_t` for the whole `/etc/nginx` tree, `systemd_unit_file_t` for
`/etc/systemd/system/nginx.service.d`, and it creates a user, which is what makes
rpm write `/etc/.pwd.lock` into the extraction tree — the katana file, arriving
for the same reason it arrived the first time. The window also caught the
still-mislabelled leftovers of §2.4 and relabelled them: `system_cron_spool_t`
for `/etc/cron.d/0hourly` and `/etc/crontab`, `bin_t` for
`/etc/cron.hourly/0anacron`, `/etc/sysconfig/crond` and `/etc/profile.d/vim.sh`,
`passwd_file_t`, `shadow_t`.

**The one mismatch is not one.** `/etc/ld.so.cache` came out
`unconfined_u:object_r:ld_so_cache_t:s0` where policy wants
`system_u:object_r:ld_so_cache_t:s0` — the **type is correct**, only the SELinux
user field differs, and the file is not written by `install_etc` at all:
`apex-pkg` runs `ldconfig` at line 1182, after the relabel pass, and `grep -c
ld.so.cache /var/lib/apex/pkg/etc.list` is **0**. It is outside the population
the fix is responsible for and it was caught only because ctime is a blunt
instrument.

### 2.3 The discriminators, and they are the ones the defect was found through

After that install:

```
systemd-run --property=DynamicUser=yes … /usr/bin/id   → Finished with result: success
systemctl start rpm-ostreed.service     → active,  Result=success  ExecMainStatus=0
systemctl start capsule@probe.service   → active,  Result=success  ExecMainStatus=0
systemctl --failed                      → empty
stat -c %C /etc/.pwd.lock               → system_u:object_r:passwd_file_t:s0
```

`rpm-ostreed` and `capsule@` are two of the six `DynamicUser=yes` units on this
machine (`chrony-wait`, `fwupd-refresh`, `rpm-ostree-countme`, `wsdd` are the
rest). Before the fix they fail at step USER with `217/USER` and **log no AVC**.

### 2.4 It was made to fail first, on this machine, on this policy

A pass after an install is ambiguous while the machine was already clean, so the
symptom was reproduced live before it was cured. It was reproduced **by
accident**, which makes it better evidence, not worse: the first install of the
session was run through a `systemd-run … bash -c "apex install …"` wrapper, and
that wrapper changed the SELinux domain (§2.5). The relabel pass was denied, and
the result was the original defect, exactly:

```
06:46:53  avc: denied { open } for comm="restorecon" path="/tmp/tmp.XwllhLanzJ"
          scontext=system_u:system_r:setfiles_t:s0
          tcontext=system_u:object_r:initrc_tmp_t:s0  tclass=file
apex-pkg: warning: relabelling /etc reported problems — see above, then:
          restorecon -nv -f /tmp/tmp.XwllhLanzJ
```

Six of the eight freshly written `/etc` paths came out wrong —
`/etc/cron.deny`, `/etc/pam.d/crond`, `/etc/sysconfig/crond` and
`/etc/profile.d/vim.sh` as `rpm_var_lib_t`; `/etc/cron.d`, `/etc/cron.daily`
and `/etc/cron.hourly` as inherited `etc_t` where policy wants
`system_cron_spool_t` and `bin_t` — and `/etc/.pwd.lock` as `rpm_var_lib_t`.

```
06:47:45  rpm-ostreed.service: Failed to update dynamic user credentials:
          Permission denied
          Failed at step USER spawning rpm-ostree: status=217/USER
```

One `restorecon -F -i -f` from an interactive shell put `/etc/.pwd.lock` back
and `rpm-ostreed` started at 06:48:15. **Every event in that window is
accounted for in the journal and every one of them was this unit's own
command** — the 217 has no second cause, and the recovery was not spontaneous.
`journalctl --since 06:46:40 --until 06:48:40` has the `sudo` lines to match.

That sequence is worth being precise about, because it is two different claims:

* *A machine that boots this image starts clean* — measured at 06:45–06:46,
  before anything ran (§2.1).
* *An install repairs a machine that is already dirty* — measured at §2.2,
  where the install relabelled the six paths the broken run had left wrong
  without being asked to.

Both hold. The second is the weaker guarantee and it is the one that would
matter on the L16, which is clean today; the point of this image is that it does
not need the weaker one.

### 2.5 What the accident actually found, measured in three contexts

`restorecon` hands the path list to the kernel as a file, and whether it can
**open that file** depends on the domain apex-pkg is running in. Isolated with
one probe script run three ways:

| how apex-pkg is reached | domain | `mktemp` file type | `restorecon -F -i -f` |
|---|---|---|---|
| `apex-sysext-rebuild.service` — ExecStart is the `bin_t` script itself | `unconfined_service_t` | `tmp_t` | **works** |
| a unit whose ExecStart is `bash -c "…"` (this unit's wrapper) | `initrc_t` | `initrc_tmp_t` | **denied** |
| interactive `sudo apex install` | `unconfined_t` | `user_tmp_t` | **works** |

`restorecon` execs `/usr/bin/setfiles` and transitions to `setfiles_t`, which
policy does not let read an `initrc_tmp_t` file.

**No path APEX itself takes hits this, and that is enumerated rather than
asserted.** Every caller of the engine in the tree was read:

| caller | how it execs | domain |
|---|---|---|
| `apex-sysext-rebuild.service` | `ExecStart=/usr/libexec/apex-pkg rebuild --if-needed` | `unconfined_service_t` — **measured** (§2.6) |
| `apex install/remove/upgrade` etc. | `Command::new(PKG_ENGINE).args(args)`, `apexd/apex/src/ops.rs:563` — absolute path, no shell | the caller's own domain; a terminal user is `unconfined_t` — **measured** (§2.2) |
| `apex recover`'s `rebuild-package-extension` step | `argv: &["/usr/libexec/apex-pkg", "rebuild", "--if-needed"]`, `apexd/apexd-core/src/recover.rs:211` — an argv array, no shell | `apexd` itself runs `system_u:system_r:unconfined_service_t:s0`, read off `ps -eo label` on katana |
| `apex blueprint`/`apply` | `run(crate::ops::PKG_ENGINE, &args)`, `apexd/apex/src/blueprint.rs:1113` | as above |

`apex-boot-health`, `apex-env` and `apex-flatpak-preinstall.service` name
`apex-pkg` only in **comments** and never exec it; `grep -rn 'sh", "-c'` over
both Rust call sites finds nothing. The only shell-wrapped invocations anywhere
are in CI, inside a container, where `/etc` is not a real machine's.

It is a live fragility all the same: any future caller that reaches `apex-pkg`
through a shell inside a unit loses the relabel, and the install still exits 0.

**A one-line hardening exists and was measured rather than guessed:** the same
denied context succeeds when the list arrives on **stdin** —
`printf '%s\n' … | restorecon -F -i -f -` returns `rc=0` and sets the label from
`initrc_t`, because no `open()` is needed. Recorded here as a follow-up; it was
not made, because changing the engine would invalidate the image this evidence
is about.

### 2.6 The unattended path, run for real

The claim "the boot-time rebuild is fine" is not left to the table above.
`state.json`'s `pkg_compat_level` was set to `2` by hand and
**`apex-sysext-rebuild.service` was restarted**, which is the exact level-2 →
level-3 rebuild that re-broke `/etc` on 2026-09-20 under the old engine.

```
apex-pkg: extension compatibility changed (OS 43, level 2 -> OS 43, level 3) — rebuilding
apex-pkg: done — 237 package(s), 530MB extension
WRITTEN PATHS: 44    MISMATCHED: 0    WANTING A NON-etc_t TYPE: 28
stat -c %C /etc/.pwd.lock  → system_u:object_r:passwd_file_t:s0
DynamicUser probe          → Finished with result: success
no restorecon/setfiles AVC in the run
```

**44 paths written by the unattended path, zero mismatched.** `state.json` came
back at level 3 on its own.

> `RemainAfterExit=yes` makes `systemctl start` on that unit a **no-op when it is
> already active**, and it is active from boot. The first attempt reported
> `Result=success ExecMainStatus=0` having run nothing at all. Use `restart`.

### 2.7 The removal pass, and the extraction tree

`sudo apex remove nginx cronie vim-enhanced` put katana back to its original
eight-package request. 15 paths in the window, **0 mismatched** (`ld.so.cache`
again, and again only in the user field). `etc.list` is back to the **same 11
entries** it held at baseline, and every one of them matches policy.

A ctime diff cannot see a deletion, so the removals were checked by hand rather
than assumed. Every file gone: `/etc/crontab`, `/etc/cron.deny`,
`/etc/anacrontab`, `/etc/vimrc`, `/etc/profile.d/vim.sh`, `/etc/sysconfig/crond`,
`/etc/pam.d/crond`, `/etc/logrotate.d/nginx`. **Nothing image-owned was taken
with them** — the 26-file class was spot-checked and intact
(`/etc/fonts/fonts.conf`, `/etc/ld.so.conf`, `/etc/krb5.conf`,
`/etc/pki/tls/openssl.cnf`, `/etc/rpc`, `/etc/pulse/client.conf`,
`/etc/asound.conf`, `/etc/profile.d/steam.sh`).

**The removal pass takes files and leaves directories.** `/etc/nginx` (plus
`conf.d` and `default.d`), `/etc/cron.d`, `/etc/cron.daily`, `/etc/cron.hourly`
and `/etc/systemd/system/nginx.service.d` were left behind empty, owned by no
package. `/etc/nginx/*` and the nginx drop-in dir were correctly labelled; the
three cron directories were **`etc_t` where policy wants `system_cron_spool_t`
and `bin_t`** — and that is not a hole in the fix, it is the fix declining
correctly. Those three were created by the **broken** first install of §2.4,
whose relabel never ran; every later install then found them already present and
left them alone, because `install_etc` records a directory only when *it*
creates one. Which is the right rule — but it means a directory created by a run
whose relabel failed stays wrong for ever, and no later install self-heals it.
Worth knowing, given the self-heal is otherwise the machine's safety net. All
seven were `rmdir`'d as this unit's own litter, after `rpm -qf` confirmed no
package owned any of them.

The third question the container could not answer — *what type does the source
tree actually carry here* — reads **`var_lib_t`**, not the `rpm_var_lib_t` the
suite's control reproduces:

```
/var/lib/apex/pkg/work.LqNnuR        unconfined_u:object_r:var_lib_t:s0
/var/lib/apex/pkg/work.*/etc/**      unconfined_u:object_r:var_lib_t:s0
matchpathcon /var/lib/apex/pkg/work.X → system_u:object_r:var_lib_t:s0
```

Both types have been seen on this machine across rebuilds, which is the
`pkg-etc-label` card's own point: the source type is incidental and the fix
holds because it asks the **policy**, not the source. `$ETC_SAVE`
(`/var/lib/apex/pkg/etc`) comes out `var_lib_t`, which is what policy wants
there, so its `restorecon -F -R` is doing its job too.

Still outstanding and deliberately untouched, as the `pkg-etc-label` card left
it: `/etc/{passwd,group,shadow,gshadow}{,-}.apexnew` are written on every
transaction that creates a user. They are correctly labelled and inert. They are
transaction artefacts rather than package content and dropping paths from
`etc.list` is the change that ate 26 image-owned files once already.

---

## 3. Gaming Mode's sched-ext scheduler — **CONFIRMED, and the honest answer is `not loaded`**

`docs/gaming-and-sessions.md` §6.8. All three rows are headless; §6.6 needs a
real session and a greetd edit and was **not** attempted.

The image is new enough to run the rows at all: `apex game status` on
`apex-97c9e8f2` had **no `scx_` field**, and on `apex-661a9d80` it has three.
That is §6.8's own test for image age.

### Row A — a scheduler is asked for, and what comes back is the truth

```
/sys/kernel/sched_ext/state   before : disabled
scx_requested: scx_lavd       scx_state : not loaded
scx_detail: sched_ext/state is disabled — nothing is attached

sudo apex game start                    → apex: game mode ON

/sys/kernel/sched_ext/state   after  : disabled
/sys/kernel/sched_ext/root/ops       : <does not exist>
scx_state : not loaded
scx_detail: asked for scx_lavd; scxctl refused: scxctl start -s scx_lavd
            reported success, but sched_ext/state is still disabled after 2s
            — no scheduler attached
```

**`grep -c 'no scx scheduler running'` over this boot's apexd journal: `0`.**
That is the fix: the verb was read off `/sys/kernel/sched_ext/state`, saw
`disabled`, and chose `start`. The old hardcoded `switch` produced that refusal
on every boot of three images.

`root/ops` could **not** be recorded, because nothing attached. The expectation
`lavd` therefore remains unchecked on hardware — say so rather than infer it.

**This is a pass, and the reason is real.** `scx_loader`'s own journal names it:

```
libbpf: extern (func ksym) 'scx_bpf_create_dsq': func_proto [1864]
        incompatible with vmlinux [60823]
libbpf: failed to load BPF skeleton 'bpf_bpf': -EINVAL
Error: the running kernel's BTF has malformed scx kfunc prototype(s): …
  This happens when the kernel was built with pahole < 1.26.
  Fix: boot a kernel whose BTF was generated with pahole >= 1.26.
[ERROR]: Failed to start scheduler (attempt 5/5)
```

**No sched-ext scheduler can load on this image at all**, for a kernel-build
reason that has nothing to do with apexd: APEX's kernel BTF was generated with
`pahole < 1.26`, so every `scx_*` scheduler fails with
`func_proto incompatible with vmlinux`. The kernel is
**`7.2.6-cachyos1.fc43.x86_64`** — `rpm -q kernel` says *not installed*, so it
is the CachyOS kernel this repo bakes from `kernel/**` rather than a Fedora one,
and that is where the BTF is generated and where a follow-up item has to land.
`scx-scheds-1.1.3-3.fc43` is the userspace side and it is not at fault. `nr_rejected` stays `0` — the kernel
never sees an attach to reject; the BPF program will not load. A separate item,
and it is what `SCX_SETTLE` would have been blamed for: the 2 s budget is **not**
the cause. `scx_state` never read `unknown` and `state` never read `enabling`,
so the constant needs no change on the evidence available.

### Row B — it goes away again, and the exit path says so

```
sudo apex game stop
/sys/kernel/sched_ext/state : disabled
scx_state : not loaded          active : false
apexd: game: sched-ext after exit: sched_ext/state is disabled — nothing is attached
```

That line is the property. Before the fix the exit path discarded every outcome
but a hard `Err`, so a refused stop was silent.

### Row C — the `switch` branch, reached, and the reading that makes the fix worth having

Row C as written needs a scheduler genuinely running, which this kernel cannot
do. The branch was reached anyway, through a condition worth more than the
scripted one: after Row A's failed attempt, **`scx_loader`'s own bookkeeping
believed a scheduler was running while the kernel said none was.**

```
sudo scxctl get              → running Lavd in Auto mode
cat /sys/kernel/sched_ext/state → disabled
sudo scxctl start -s scx_lavd   → error: scx scheduler already running,
                                  use 'switch' instead of 'start'
```

That is the exact lie the fix exists to stop repeating, and APEX did not repeat
it: with `state` reading `disabled` it chose `start`, was refused, and took the
retry:

```
apexd: scxctl start -s scx_lavd failed (exit status: 1): error: scx scheduler
       already running, use 'switch' instead of 'start'
apexd: scxctl asked for 'switch' instead of 'start' — retrying once
apexd: game: sched-ext: not loaded — asked for scx_lavd; scxctl refused:
       scxctl switch -s scx_lavd reported success, but sched_ext/state is
       still disabled after 2s — no scheduler attached
```

**Both verbs were exercised on hardware, the retry fired, and the status still
refused to claim a scheduler that the kernel says is not there** — while
`scxctl get` was saying "running Lavd in Auto mode" three inches away. Neither
verb is hardcoded; the retry is driven by scx_loader's own error text.

Row C's *other* half — the named "stopped it rather than restoring it"
limitation on exit — could not be reached, because nothing was ever running to
restore. Still open.

Cleaned up: `apex game stop`, `scxctl stop` (`stopped`), `scxctl get` →
`no scx scheduler running`, `sched_ext/state` → `disabled`, `active : false`.

### Not a defect, checked before it was written down

The `game mode ON` journal line reads `1/1 GPU(s) locked` while
`apex game status` prints `gpus_locked: 0`. Those agree: `gpus_locked` is a
`Vec<u32>` of **GPU indices**, rendered as a list, so `0` means "card index 0",
not "none" — the same rendering as `cpus : 0-11`. Recorded because it reads like
the exact class of defect this unit was sent to look for.

---

## 4. katana, as it is left

* Booted `apex-661a9d80d2239d848676d97e1e633d3f7325e853`,
  `sha256:61f7935c…`. **Three deployments**: booted, `apex-97c9e8f2` as
  rollback, and the **September 18 `apex-266dcc57` still `Pinned: yes`**
  underneath. `apex-7f647470` was pruned by the rotation — it was unpinned, and
  that is the expected behaviour.
* `efibootmgr -v` **byte-identical** before and after (`cmp` clean, 16 lines).
  `Boot0000 APEX-OS Primary` still points at the Windows disk's ESP. Nothing was
  written to the Windows disk.
* **The NVMe controllers swapped again** — the third time. The APEX Micron
  `220534D1CB81` is `nvme0n1` now (it was `nvme1n1` before this reboot) and the
  Windows SPCC `240023925111005` is `nvme1n1`. `/var` is on `apex-root`,
  verified by LABEL and serial. Never trust a device name on this machine.
* greetd `cmp`-identical to `config.toml.orig-qual2`, `0` `initial_session`,
  `greetd` active, no timer armed. **greetd was never edited by this unit** and
  no dead-man timer was needed, because §6.8 is headless.
* `~/apex-pre-rebase-20260919/` intact, 4 files, 2.1 M.
* Package set back to its original eight requests (`chromium gamemode gamescope
  libSM.i686 libgcc.i686 mangohud steam steam-devices`); 223 packages, 512 MB
  extension, rebuilt against the new image. `cronie`, `vim-enhanced` and `nginx`
  were added for the measurement and removed.
* `systemctl --failed` empty. `/var` 51 G free. All transient probe units
  collected, scratch files removed.
* `/etc` is clean and, unlike last round, **nothing was `restorecon`ed by hand
  to make it so** — the last thing to touch it was `apex remove`. Final reading:
  `etc.list` 11 tracked paths, **0 mismatched**; `/etc/.pwd.lock`
  `passwd_file_t`; `DynamicUser` probe `result: success`; `systemctl --failed`
  empty.

## 5. What this does not say

* **Fresh install and rollback-reboot remain unverified**, as they were. The
  rollback slot now holds a genuinely different image (`apex-97c9e8f2`), so a
  rollback reboot would finally prove something — it was not attempted, because
  it costs katana's boot and nothing on this unit needed it.
* §6.6 (a Gaming Mode session destroyed without cooperation) was not re-run.
* `root/ops` is still unread on any machine.
* **Secure Boot is still disabled on katana.** Unchanged, and still against the
  stated end-state.

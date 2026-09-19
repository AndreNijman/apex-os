# pkg-etc-label
items: P0-001 (the `/etc` SELinux mislabel found during the 2026-09-20 katana qualification)
repo: apex-os
worktree: /var/tmp/apex-work/wt-pkg-etc-label
branch: task/pkg-etc-label
base: roadmap/v2.2 @ 777ba028
commits: 3ce33791 — pushed to origin/task/pkg-etc-label, **NOT landed**

## NEXT

**The code work is finished. Nothing is in progress. Do not re-dispatch an agent
for it.** Two things remain, and only one of them is anybody's job right now.

### 1. Land it. Nothing here needs a machine.

`task/pkg-etc-label` is one commit on top of `roadmap/v2.2` @ `777ba028`,
touching three files, all of which this unit owned:

```
files/system/libexec/apex-pkg            +80/-1     the fix (install_etc + relabel_etc)
tests/test-apex-pkg-etc-label.sh         new suite, 21 assertions
.github/workflows/pr-validation.yml      new step in the `engine` job, floor 9
```

Landings are merges, not rebases. Check for drift on `apex-pkg` before merging —
`pkg-share` and `pkg-update` also live in that file and this unit did not touch
anything either of them touched (`relabel_etc` is new, and the only edits inside
`install_etc` are added lines).

### 2. What a real machine still has to confirm, and it is short

Everything below was measured, in a container, against Fedora 43's policy rpm.
**No package was installed on any machine; neither katana nor the L16 was
touched.** Three things a container structurally cannot say:

* **Does a `DynamicUser=yes` unit start after an install?** There is no PID 1
  here. That is the symptom the whole defect was found through, and it is the
  one-line check:

  ```
  sudo apex install <something that ships /etc>
  stat -c %C /etc/.pwd.lock                       # want passwd_file_t
  systemd-run --wait --collect --property=DynamicUser=yes --unit=du-probe /usr/bin/id
  ```

  The probe must finish with `result: success`. Before the fix it exits
  `217/USER` and logs no AVC.

* **Does `restorecon` resolve against the APEX image's loaded policy?** The
  suite asks Fedora 43's `selinux-policy-targeted`. A type APEX's policy names
  differently would still be right on the machine and unmeasurable here.

* **Does the extraction tree come out `rpm_var_lib_t` on its own?** That is
  katana's measurement, not this suite's: the container's tree would be
  `container_file_t`, so the suite `chcon`s it to `rpm_var_lib_t` to reproduce
  the finding exactly. If a future engine change moves `WORK_ROOT`, the source
  type moves with it and the fix still holds — the pass asks the policy, not the
  source — but the katana reproduction in the control is then a historical
  re-enactment rather than a live one.

**The machines that already have the mislabel fix themselves.** `install_etc`
relabels every path it writes *and* the tracked paths its removal pass declines
to delete, so the next `apex install` or `apex update` on katana puts its
remaining `etc.list` entries right without anyone running `restorecon` by hand.
katana's were already fixed live on 2026-09-20; the L16 was measured clean.

---

## What was wrong

`install_etc` copies its payload with `cp -a`, which preserves the **source**
label, and the source is an rpm extraction tree under `/var/lib`. So every file
`apex install` put in `/etc` arrived as `rpm_var_lib_t`.
`ROADMAP/evidence/katana-pwd-lock-selinux-20260920.md` has the measurement: 7 of
11 tracked paths wrong, and one of them `/etc/.pwd.lock`, which policy wants as
`passwd_file_t`. systemd takes that lock before it will allocate a dynamic UID,
so **all six `DynamicUser=yes` units on the machine broke at once** —
`capsule@.service` and `rpm-ostreed.service` among them — each failing at step
USER with `217/USER`. No AVC: the denial is in PID 1 before any domain
transition. `systemctl --failed` showed nothing, because those units are
on-demand. Four rounds of probing to find, and it recurred on the next rebuild
with a *different* wrong type.

## What replaced it

`install_etc` now collects every path it creates or overwrites **as it goes**,
and hands the list to `restorecon -F -i -f`, which asks the **loaded policy**
what each path should be.

Built as it goes rather than derived afterwards, because only the branch that
ran knows what it actually wrote:

* the "you edited it" branch writes `<path>.apexnew` and records **both** it and
  the original — the original is still a file this engine put there, so its
  label stays ours even though its content is not;
* the "already there and not ours" branch records **only** `.apexnew`. The
  original predates this engine and its label belongs to whoever put it there;
* a directory is recorded **only when we create it**. `mkdir` inherits the
  parent's type by transition, which is wrong wherever policy has an opinion
  (`/etc/cron.d` is `system_cron_spool_t`); one that was already there is not
  ours to relabel;
* the removal pass records the paths it **declines** to delete.
  `/etc/security/limits.d/10-gamemode.conf` on katana was exactly that —
  tracked, no longer shipped, still mislabelled, and the same defect rather than
  a separate accident.

`$ETC_SAVE` gets one `restorecon -F -R`: that tree is engine-owned top to
bottom, so `-R` is unambiguous there in a way it could never be under `/etc`.

**`-F` is load-bearing, and it is not obvious.** A bare `restorecon` silently
declines to touch any file whose current type is in the policy's
`customizable_types` list — `not reset as customized by admin`, one line per
file, then exit 0. The exemption is for labels an admin chose; a file apex-pkg
has just written is not one of those. The suite has a control for it that reads
the type out of the policy rather than naming one.

**stderr is not swallowed.** This defect reported nothing at all for months, and
`restorecon`'s own message is the only thing that would name a path it could not
set.

**`label_tree` is deliberately untouched.** The evidence's suggested shape names
it — "`relabel_tree` must cover `${root}/etc`" — but `split_out_of_image` has
already moved `/etc` out of the payload root before `label_tree` runs, so there
is no `/etc` there to label. A stranger reading the evidence will expect to see
that half of the fix; this is why it is not there.

## The instrument

`tests/test-apex-pkg-etc-label.sh`, 21 assertions. It does **not** check
`/etc/.pwd.lock`: that is where the defect happened to bite, not where it lives.
The property is over **everything the pass writes** — real packages decide which
paths exist, and the filesystem's own **ctime** decides which of them the pass
touched, so a file nobody has thought of is covered. Nothing in the suite names
a path the engine also names.

The population is read off a before/after ctime snapshot, not re-derived from
`install_etc`'s branch logic. Directories are the exception and not a small one:
a directory's ctime moves when a child is created inside it, so a pre-existing
`/etc/profile.d` would otherwise read as something the pass wrote and then be
demanded of a relabel pass that correctly left it alone. A directory counts only
when it is **new**. (An earlier version used a marker file and `-cnewer`, and
read the baseline relabel of 1200 files as the pass's own writes.)

The engine is run **twice**, because the second install is the one that
overwrites: the first writes `<path>.apexnew` and leaves the original alone, the
second finds a matching saved copy and overwrites — and that is the run that put
`rpm_var_lib_t` on katana's `/etc/.pwd.lock`.

### Two modes, and the suite says which one it used

* **enforced** — `/etc` inside the container is a bind mount of a **host-backed**
  volume, and `/sys/fs/selinux` is mounted **rw**. Both are necessary and each
  was measured: podman's own rootfs is mounted with a fixed `context=`, where
  `setxattr security.selinux` returns `EOPNOTSUPP` and nothing can be
  relabelled at all; and with `/sys/fs/selinux` mounted **ro**, `selinuxenabled`
  returns 1 and `restorecon` becomes a silent no-op. Labels are set by the
  engine and read back per file. **21 passed / 0 failed**, 2026-09-20.
* **coverage** — no SELinux on the host (a GitHub `ubuntu-24.04` runner), or
  `APEX_ETC_LABEL_FORCE_COVERAGE=1`. `restorecon` and `selinuxenabled` are
  shimmed to record and to answer yes, so the engine takes the same path it
  takes on a labelled machine, and what is asserted is that every path the pass
  wrote was **handed to the policy labeller** and that `matchpathcon` — which
  works from the policy files with SELinux switched off — wants a non-default
  type for a real share of them. **9 passed / 0 failed**, and 9 is the CI floor.

The mode is decided by a **capability probe** (mislabel a scratch file, relabel
it, check it moved), not by reading `selinuxenabled`. And the suite **refuses to
degrade silently**: if the host has `/sys/fs/selinux` and the enforced leg still
did not run, that is a FAIL, not a skip.

### Controls — four, all firing on today's packages

* **pre-fix replica.** `relabel_etc` overridden to a no-op; 58 mislabelled files
  come back, and `/etc/.pwd.lock` is `rpm_var_lib_t` where policy wants
  `passwd_file_t` — the katana finding, reproduced rather than asserted. If the
  package set stops producing that file the control goes red and says so.
* **one file broken by hand.** After a green run, `chcon` a single written file
  and require the checker to name **that file and only that file**. This is what
  separates "reads labels per path" from "checked that restorecon ran".
* **one path dropped from the engine's list.** The realistic regression is not
  "the pass was deleted" but "a new branch forgets to record what it wrote", so
  the coverage gate must go red for a single missing path. It does.
* **the `customizable_types` case**, which documents why `-F` is passed. The type
  is read out of the policy, not named.

Plus four can-only-pass guards: the pass must have written something in each
run, and policy must want a non-default type for some of what it wrote — without
that second one, "label it `etc_t`" would pass every assertion in the file.

### Mutation-tested from outside the suite as well

* delete `relabel_etc "$relist"` → **6 red enforced, 2 red coverage**
* delete the one `printf` that records the plain-write branch → **coverage red
  for the 44 paths that branch handles**

Both restored with plain `cp`, verified `cmp` byte-identical.

### Package set, and why those packages

`chrony cronie logrotate vim-enhanced which sudo openssh-server fontconfig` —
chosen for what the **policy** says about the `/etc` they ship, not for what the
files are called: `bin_t` (profile.d scripts), `passwd_file_t` and `shadow_t`,
`system_cron_spool_t`, `dhcp_etc_t` and plain `etc_t`. chrony and cronie create
users, which is what makes rpm write `/etc/.pwd.lock` into the extraction tree —
the katana file, arriving here for the same reason it arrived there. fontconfig
is in for the `/etc` **symlinks**, the case `install_etc`'s own "cp -a, NEVER
install" comment is about.

Final: etc-label 21/0 (enforced) and 9/0 (coverage forced), pkg 82/0,
multilib 3/0, multilib-extract 11/0, pkg-update 17/0,
check-suites-run-in-ci 80 suites / 0 undeclared, check-shellcheck-coverage
171 scripts / 0 failing. `shellcheck -S warning -x` clean on both changed
scripts at **0.9.0** (the runner's version, via the koalaman container).

## Traps paid for here

* **`restorecon` without `-F` fixes `rpm_var_lib_t` fine and refuses
  `container_file_t`.** The first version of the `-F` control asserted that
  dropping `-F` would leave the *install* mislabelled; it does not, because
  `rpm_var_lib_t` is not a customizable type. An assertion that cannot fail for
  the reason it names is worse than none — it was replaced with the direct
  measurement.
* **Rootless podman leaves subuid-owned files in a mounted volume.** Cleanup has
  to go through `podman unshare rm -rf`; a plain `rm` leaves a directory the
  user cannot delete, and the second run then fails on a stale volume.
* **A directory's ctime moves when a child is written.** Any "what did this pass
  touch" measurement built on ctime must special-case directories or it reports
  the whole tree.

## Left for later, deliberately

* `/etc/passwd`, `/etc/shadow`, `/etc/group` and their `-` backups reach the
  etcroot whenever the transaction creates a user, so a real install leaves
  `/etc/passwd.apexnew` and friends lying in `/etc`. They are correctly labelled
  now and they are inert, but they are noise the engine should probably not
  write at all. Same question as the evidence's "`.pwd.lock` arguably should not
  be in `etc.list`": these are **transaction artefacts**, not package content,
  and no rpm in the set owns them. Deciding which of `etcroot`'s paths are
  artefacts wants its own measurement, and dropping files from `etc.list` is
  exactly the change that ate 26 image-owned files last time. Not guessed at
  here.

# `apex install` mislabels every file it puts in `/etc`, and one of them breaks
# `DynamicUser=yes` for the whole machine

Found 2026-09-20 on katana by the `integrated-image` unit, while checking the
machine was fit to be rebased. **This is a shipped `apex-pkg` defect, not a
katana accident**, and it is the kind that reports nothing until something
unrelated dies eight hours later.

## The symptom the monitor saw

```
rpm-ostreed.service: Failed to update dynamic user credentials: Permission denied
rpm-ostreed.service: Failed at step USER spawning rpm-ostree: Permission denied
Main process exited, code=exited, status=217/USER
```

Persistent across `systemctl reset-failed` + `start`. Not space (`/run` 1 % of
13 G, `/var` 53 G free), not a missing `/run/systemd/dynamic-uid`, not
`/var/lib/private`. **And not rpm-ostreed**: a bare probe fails identically, so
every `DynamicUser=yes` unit on the machine was broken.

```
# systemd-run --wait --collect --property=DynamicUser=yes --unit=du-probe-1 /usr/bin/id
Main processes terminated with: code=exited, status=217/USER
```

## What it actually was

SELinux is Enforcing and **no AVC was logged for it** — the denial is on a path
PID 1 takes before any transition, and the useful line only exists at debug
level. `systemd-analyze set-log-level debug`, one probe, `set-log-level info`:

```
(id)[288965]: du-probe-3.service: Cannot open /etc/.pwd.lock: Permission denied
(id)[288965]: du-probe-3.service: Failed to update dynamic user credentials: Permission denied
(id)[288965]: du-probe-3.service: Failed at step USER spawning /usr/bin/id: Permission denied
```

`dynamic_user_realize()` takes the passwd lock (`take_etc_passwd_lock()`)
before it will allocate a UID. It could not open the lock file:

```
/etc/.pwd.lock   cur=system_u:object_r:rpm_var_lib_t:s0
                want=system_u:object_r:passwd_file_t:s0
/usr/etc/.pwd.lock (the image default)  system_u:object_r:passwd_file_t:s0
```

`rpm -qf /etc/.pwd.lock` → not owned by any package. `/var/lib/apex/pkg/etc.list`
line 10 → `.pwd.lock`. **apex-pkg installed it, and tracks it.** It is created
by `useradd`/`groupadd` scriptlets running inside the dnf `--installroot`
transaction, then swept into `/etc` by `cp -a "${root}/etc/." "${etcroot}/"`
(apex-pkg line 880) and `install_etc`'s `cp -a` (lines 1024-1035).

`cp -a` preserves the SOURCE label. `relabel_tree()` (lines 938-946) runs
`setfiles` on `${root}/usr` and `${root}/opt` **and on nothing else** — `/etc`
is never relabelled, neither in the extraction tree nor after the copy into the
live `/etc`. So every file `apex install` puts in `/etc` arrives as
`rpm_var_lib_t`.

## Every tracked file, checked — 7 of 11 were mislabelled

```
MISLABEL /etc/chromium/chromium.conf              rpm_var_lib_t  want etc_t
MISLABEL /etc/chromium/master_preferences         rpm_var_lib_t  want etc_t
MISLABEL /etc/ld.so.conf.d/fdk-aac-lib.conf       rpm_var_lib_t  want etc_t
MISLABEL /etc/profile.d/steam.csh                 rpm_var_lib_t  want bin_t
MISLABEL /etc/profile.d/steam.sh                  rpm_var_lib_t  want bin_t
MISLABEL /etc/.pwd.lock                           rpm_var_lib_t  want passwd_file_t
MISLABEL /etc/security/limits.d/10-gamemode.conf  rpm_var_lib_t  want etc_t
```

Six of those are logged-only (`permissive=1` on the sshd AVC for the limits.d
file, which fires on **every ssh login**). `.pwd.lock` is the one that is
load-bearing, and it takes down a facility nothing in APEX owns.

`/etc/security/limits.d/10-gamemode.conf` is dated Jul 23 2025 and owned by no
package — a leftover from the old extension era — but it is in `etc.list`, so
its mislabel is the SAME defect, not a separate accident.

## Fix applied to the machine (live only; no repo change)

```
# restorecon -v <the 7 paths>
Relabeled /etc/.pwd.lock from system_u:object_r:rpm_var_lib_t:s0 to system_u:object_r:passwd_file_t:s0
  …6 more…
# systemd-run --wait --collect --property=DynamicUser=yes --unit=du-probe-4 /usr/bin/id
          Finished with result: success
# rpm-ostree status
State: idle        (and systemctl is-active rpm-ostreed → active)
```

`restorecon` changes labels, never content, so apex-pkg's removal pass — which
compares sha(live) against sha(saved) — is unaffected.

## Blast radius, measured rather than guessed

`grep -l '^DynamicUser=yes' /usr/lib/systemd/system/*.service` on the booted
image — **6 units**, and rpm-ostreed is the least interesting of them:

```
capsule@.service          <- APEX Capsules. Our own feature.
chrony-wait.service
fwupd-refresh.service
rpm-ostree-countme.service
rpm-ostreed.service
wsdd.service
```

So the reading is not "rpm-ostreed broke". It is: **after `apex install`, APEX
Capsules cannot start, firmware refresh cannot start, and the ostree daemon
cannot start** — on a machine that reports nothing wrong until something tries.

## EXPECTED TO RECUR, and that is the point

The level 2 -> 3 rebuild this unit is about to trigger runs the SAME
`install_etc` with the SAME `cp -a`. The dnf transaction re-runs the
useradd/groupadd scriptlets, `.pwd.lock` reappears in the extraction tree, and
`install_etc` writes it into the live `/etc` again — live sha and saved sha are
both the empty-file sha, so the "user has not touched it" branch fires and
overwrites. `post-boot.sh` therefore audits every `etc.list` path against
`matchpathcon` and re-runs the `DynamicUser` probe **before** any `restorecon`,
so a second occurrence is measured instead of quietly repaired. If it recurs,
the finding is not "katana got a bad label once" but "every extension rebuild
re-breaks DynamicUser", which is a different severity.

`systemctl --failed` will NOT show it: `rpm-ostreed` is `Type=dbus` and
on-demand, and nothing starts it at boot.

## What still needs doing, and by whom

1. **A code fix in `files/system/libexec/apex-pkg`.** DELIBERATELY NOT MADE ON
   `task/integrated-image`: image build 35461554871 was already running against
   97c9e8f2, and shipping an unreviewed engine change into the image Andre is
   about to boot the L16 onto is the wrong trade at 03:00. The shape of it:
   `relabel_tree` must cover `${root}/etc` as well as `/usr` and `/opt`, and
   `install_etc` must `restorecon` each path it writes into the live `/etc`.
   A `.pwd.lock` that is a transaction artefact rather than package content
   arguably should not be in `etc.list` at all.
2. **Check the L16.** It has run `apex install`. If its `/etc/.pwd.lock` is
   `rpm_var_lib_t`, its `rpm-ostreed` is broken in exactly this way and nothing
   has told anyone. One read-only command:
   `stat -c %C /etc/.pwd.lock` — want `passwd_file_t`.
   The `integrated-image` agent did not run it: the L16 is off limits.

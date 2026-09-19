# The integrated image, built and qualified on katana

Unit `integrated-image`, 2026-09-20. The L16 half of `final` is **Andre's** and
nothing here touched it.

## 0. The build

**Run 35461554871**, `workflow_dispatch` on `task/integrated-image` @ `97c9e8f2`
(= `roadmap/v2.2` tip), queued 2026-09-19T18:33:05Z, **completed `success` at
19:45Z, 1 h 12 m**. Every job's steps were read rather than assumed, because a
skipped job counts as success here: `core` 13 steps all `success`, `image` 17
steps all `success`. `installer-iso` and `qcow2` are `skipped` because their
dispatch inputs default false.

```
ghcr.io/andrenijman/apex-os:apex-97c9e8f25ee55593a97502505f51c6115ebbee7c
  digest sha256:55fc9e4ee9b2c1b27045cba5a594a40e97fb0d73c70170cfbe0003f5cd4d73b0
```

Two properties that matter more than "it is green":

* `Pinned apex-shell roadmap/v2.2 (apex-shell has no branch named
  task/integrated-image): 03d77f96f521df7501d710d5c15ae8d0074a39ce` — the
  ordered fallback landed as `12e9d454` did its job. This is **not** a
  main-shell build, which is the failure that killed three earlier runs in
  `check-labwc-keybinds`.
* `not publishing from refs/heads/task/integrated-image: per-SHA tags written`
  — `:apex`, `:daily`, `:gaming-mesa`, `:gaming-nvidia`, `:core`, `:base` and
  every `platform-*` were untouched. No machine but katana saw this image.

`core rebuild: true (core sources changed)`, so katana's pull was the whole
image (67 of 113 layers, 6.3 GB) rather than the tens of MB a base-only push
costs.

## 1. Getting it onto katana

`bootc switch --transport registry <the tag above>`, then a clean
`systemctl reboot`. Booted 2026-09-20 04:07:50 AWST onto
`sha256:55fc9e4e…`; `ostree admin status` shows three deployments with the
**September 18 rollback still `Pinned: yes`** — it was pinned before the switch
precisely so the deployment rotation could not prune it.

`apex update` is NOT the route and could not have been: katana tracks a per-SHA
tag that never moves, and a non-main dispatch moves no floating tag. The unit's
own re-open condition is "an image built from a tip carrying `b512cf12`, taken
by a machine that already has an extension", and a `bootc switch` satisfies it
exactly.

**A trap worth the two lines it takes to record.** `bootc switch` run straight
over ssh is SIGHUPed when the channel closes, and a 600 s tool cap closes it.
The 6.3 GB pull survived in the image store, nothing was staged, and
`bootc status` reported `staged: none` — which reads exactly like a failed
pull. Re-run it as a detached transient unit and it finds `No changes` and
stages in 10 seconds:

```
sudo systemd-run --unit=intimg-switch --collect \
  --property=TimeoutStartSec=infinity --service-type=oneshot \
  /usr/bin/bootc switch --transport registry <IMG>
```

## 2. `pkg-update` — an image update now delivers a baked fix. CONFIRMED.

This is the one that mattered, because the old failure **reported success**.
The before/after is on one machine, one reboot apart.

| | before (`apex-7f647470`) | after (`apex-97c9e8f2`) |
|---|---|---|
| `apex-sysext-rebuild` | 18:29:40.573718 → **.606170** — 33 **ms** | 04:08:07.005801 → 04:09:00.811002 — **53.8 s** |
| the journal's own reason | *nothing; it just finished* | `extension compatibility changed (OS 43, level 2 -> OS 43, level 3) — rebuilding` |
| `state.json` `pkg_compat_level` | 2 | **3** |
| engine constant | `PKG_COMPAT_LEVEL=2` | `PKG_COMPAT_LEVEL=3` |
| `apex-user.raw` sha256 | `eb3b8ba0…22c28a6` | **`75290ae7…8424c0041`** |
| `Result` / `ExecMainStatus` | success / 0 | success / 0 |

and the pkg-share properties the rebuild had to preserve, all re-measured after
it rather than carried over:

```
ls /usr/share/vulkan/icd.d/ | grep -c i686        -> 13
comm -12 <extension file list> <rpm -qal>         -> 0 lines  (ext 3 564, image 266 093)
newly carried vs the pre-rebuild extension: 0     lost: 0
systemctl --failed                                -> nothing
```

`multilib: carrying 2079 of 4382 file(s) from the 32-bit set (2151 already
owned by the image, 152 already placed by this set's native packages)` — the
rpmdb-driven rule ran, from an image, on hardware.

**Read the numbers honestly.** The extension is byte-different but carries an
identical file set, because the resolved set (223 packages) did not move — the
*level* did, which is the entire point of the fix. And the duration is **54
seconds, not "minutes"**: katana's rpms were already cached. 54 s against 33 ms
is the discriminator; the wording in the earlier notes was optimistic.

**Katana's rebuild was the redundant one**, exactly as `pkg-update`'s card
predicted: its extension had already been built by the fixed engine but stamped
level 2. A `level 2 -> level 3` line here is the mechanism working.

**Still not answered, and it cannot be claimed from this run:** whether
`systemd-sysext refresh` re-merging `/usr` disturbs a *live desktop*. Nothing
was running on katana but the greeter. Zero failed units afterwards is the
weaker fact that was available.

## 3. The coredump drop-in. CONFIRMED.

Build side, `Containerfile.base` STEP 137/138:
`coredump storage bounded: MaxUse=256M KeepFree=2G (default was a 4 GiB cap)`.

Hardware side, on the booted image:

```
# systemd-analyze cat-config systemd/coredump.conf | grep -E '^(MaxUse|KeepFree)='
MaxUse=256M
KeepFree=2G
# ls /usr/lib/systemd/coredump.conf.d/
50-apex-coredump-limits.conf
```

Neither line appears on an image without the drop-in — Fedora ships every value
commented out — so the grep is a real discriminator and it read **nothing**
before the switch.

## 4. `p2-b` — the Apex.I18n catalogue stage. CONFIRMED, both halves.

No full base build had ever carried this stage. This one did.

Build (`base` job): `catalogue: compiled 1 catalogue(s) from apex-shell
03d77f96f521df7501d710d5c15ae8d0074a39ce`, `Apex.I18n built and proved to load:
29776 bytes`, `Apex.I18n stage complete`, `catalogue: apex-shell_de.qm loads
under LANG=de`.

Hardware, on the booted `/usr` — and the **journal is not a channel here**,
because greetd dup2s the VT onto the session's stdio, so the Containerfile's
own offscreen probe was reproduced instead:

```
/usr/lib64/apex-shell/qml/Apex/I18n/libapexi18n.so   29 776 bytes
/usr/lib64/apex-shell/qml/Apex/I18n/qmldir              946 bytes
/usr/share/apex-shell/translations/apex-shell_de.qm   1 421 bytes

LANG=C.UTF-8      APEXI18N: registerTypes uri=Apex.I18n
                  APEXI18N: initializeEngine uri=Apex.I18n
                  APEXI18N: no catalogue for C in /usr/share/apex-shell/translations
LANG=de_DE.UTF-8  APEXI18N: registerTypes uri=Apex.I18n
                  APEXI18N: initializeEngine uri=Apex.I18n
                  APEXI18N: installed /usr/share/apex-shell/translations/apex-shell_de.qm
```

Both legs matter: the `C` run proves the module reports honestly when there is
no catalogue for the locale, and the `de_DE` run proves the catalogue the image
compiled is actually **installed into a live QML engine on this machine**. The
shell half is present too — `I18nBootstrap.qml:25 import Apex.I18n`, and
`shell.qml` creates that component — and greetd is `active`, which a fatal root
import would prevent.

What is left is still a translator's job: 5 of 186 marked strings.

## 5. The defect found on the way in, and it CAME BACK

Full account: `ROADMAP/evidence/katana-pwd-lock-selinux-20260920.md`.
`apex install` writes files into `/etc` with `cp -a` and never relabels them,
and `/etc/.pwd.lock` mislabelled breaks `DynamicUser=yes` for **all six** units
that use it, `capsule@.service` among them.

**The rebuild in §2 re-created it**, which is the second measurement and the
one that changes the severity:

```
before the switch   /etc/.pwd.lock  rpm_var_lib_t   -> DynamicUser BROKEN, rpm-ostreed 217/USER
after the rebuild   /etc/.pwd.lock  var_lib_t       -> DynamicUser works, rpm-ostreed fine
policy wants                        passwd_file_t
```

All 7 tracked `/etc` paths were mislabelled again, with a *different* wrong type
than before. So this is not "katana got a bad label once": **every extension
rebuild re-labels these files with whatever the extraction tree happened to
carry**, and whether the machine breaks depends on which wrong type it lands on.
A latent defect that fires sometimes is worse than one that fires always.
`restorecon` was applied again and re-verified.

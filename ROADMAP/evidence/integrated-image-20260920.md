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

## 6. `gaming-release` — §6.6 and §6.7 of `docs/gaming-and-sessions.md`

Both sessions were armed through greetd's own launch path with
`greetd-set.sh apex-gaming`, a `qual-greetd-restore` dead-man timer armed
FIRST, and `greetd-restore.sh` last. greetd ends byte-identical to
`config.toml.orig-qual2` (`cmp` says so), `0` occurrences of
`initial_session`, `0` timers listed, `greetd` active, and the greeter is
live on tty1 (sway + swaybg + the apex-greet shell).

### The image is new enough, and it says so from the outside

`apex game status` printed **`owner_pid : 7799`** and `ps` confirms 7799 is
`bash /usr/libexec/apex-gaming-session`. `owner_pid : 0` would have meant the
watch was absent; on the previous image the key is not printed at all. (It is
only inserted while a session is ACTIVE — `apexd/apexd/src/game.rs` ~457 — so
its absence on an idle machine is by design, and the idle discriminator is
`strings -a /usr/bin/apexd | grep -c 'the session owner is gone'`.)

### §6.6 — a session destroyed with nothing able to cooperate. CONFIRMED.

**The run-book's own recipe did not actually test the new code, and that is
worth recording.** `sudo systemctl restart greetd` killed gamescope, but the
session script itself survived long enough to run its trap and released game
mode by the ordinary path — `[apex-gaming-session] apexd game mode released`,
three times, idempotent. A clean release is a good outcome; it is not this
row.

So the owner was **SIGKILLed** instead, which is what "nothing can cooperate"
means — no trap, no signal handler, no `apex game stop`:

```
04:12:37  kill -9 9688            (bash /usr/libexec/apex-gaming-session)
04:12:38.911833  apexd: game: the session owner is gone (/proc/9688 is gone) — releasing game mode.
                 Nothing else could: `apex game stop` from a deactivated session is refused by polkit.
04:12:39.012106  apexd: game mode OFF — tier restored to performance, auto-switch on
```

**1.9 seconds**, which is the 2 s watch. And the discriminators:

```
apex game status        -> active : false
/sys/fs/cgroup/apex-game -> removed
pgrep gamescope          -> (none)
```

**The governor is not a discriminator here** and neither reading moved:
`tier` and `prior_tier` are both `performance` and `scaling_governor` reads
`performance` before, during and after, because katana's AC default tier is
already `performance`. The run-book says so; this run confirms it rather than
quoting it as evidence.

**Neither is `sched_ext/state`, and THAT is a defect — see §7.** It reads
`disabled` during the session as well as after, so it cannot discriminate on
this machine at all.

### §6.7 — the overlay is gone and the dump store is bounded. CONFIRMED.

```
[apex-gaming-session] starting: gamescope -e -f --expose-wayland --prefer-vk-device 10de:249d --prefer-output HDMI-A-1 -- steam -gamepadui
[apex-gaming-session] NOT passing --mangoapp: it and --expose-wayland cannot both be on.
```

`--expose-wayland` present, `--mangoapp` **absent**, and the explanation
printed. Against the 2026-09-19 two-hour session on the previous image:

| | 2026-09-19 (old image) | this run |
|---|---|---|
| new `mangoapp` core dumps | 15 376 in one boot | **0** (15 642 before, 15 642 after — the count is the journal's index of the OLD dumps) |
| `Glfw Error` lines | 57 612 | **0** |
| session journal lines | 144 187 | **171** |
| stored dumps | 4.0 GB | 26 M -> 31 M, and the 5 M is gamescope's own exit segfault |
| `MaxUse` / `KeepFree` | *(no drop-in; Fedora comments every value out)* | **256M / 2G** |

`gamescope` still segfaults on exit (`139`, core dumped) — finding 3 from the
previous run, unchanged and still cosmetic, since the trap runs anyway.

### NOT confirmed, and named rather than glossed

* **Whether the MangoHud overlay RENDERS with `APEX_GAMING_EXPOSE_WAYLAND=0`.**
  Not attempted. It needs somebody looking at the panel, or a frame capture;
  "mangoapp stayed alive" is a different claim from "the overlay drew a pixel"
  and must not stand in for it.
* **Safe Graphics' automatic dGPU branch** — needs the panel genuinely dark.
* **Any real game launch.** Steam was started with `-gamepadui` and gamescope
  came up; no title was run.
* **Whether the `/usr` re-merge disturbs a live desktop** (see §2).

## 7. A defect found while qualifying §6.6: Gaming Mode has NEVER loaded a
## sched-ext scheduler, and `apex game status` says it did

```
apexd: scxctl switch -s scx_lavd failed (exit status: 1): error: no scx scheduler running, use 'start' instead of 'switch'
apexd: game: sched-ext: scx_lavd for the session, kernel scheduler restored on exit
```

The second line is the user-facing summary, and it is also the `notes` field
of `apex game status`. It asserts as fact something the line above it just
reported as failed.

`apexd/apexd-core/src/syswriter.rs:739`:

```rust
Action::ScxSwitch { sched } => self.run_scxctl(&["switch".into(), "-s".into(), sched.clone()]),
```

`scxctl switch` replaces a **running** scheduler. Nothing loads one at boot on
APEX, so the first entry into Gaming Mode always finds none and `scxctl` says
so in as many words. `scxctl start` is the verb for that state.

**This is NOT a regression from the new image.** The same two lines are in
`journalctl -b -1` and `-b -2`, i.e. on the previous image too. `scxctl` and
`scx_lavd` are both installed (`scx-scheds-1.1.3-3.fc43`), `scx_loader.service`
is active, `/sys/kernel/sched_ext/nr_rejected` is 0 and `enable_seq` is 0 — the
kernel never rejected a scheduler, because one was never offered.

Three consequences, in order of how much they cost:

1. The profile's headline sched-ext feature has never once applied.
2. `apex game status` reports it as applied, so nobody could have noticed.
3. `docs/gaming-and-sessions.md` §6.6 names `sched_ext/state` as a release
   discriminator, and on this hardware it is not one — exactly like the
   governor, which the run-book already flags. The doc should say so.

Not fixed here, deliberately: the image under qualification is already built
and on the machine, and a one-word engine change would invalidate the run it
is embedded in. It wants its own unit, with a test that requires the failure
back.

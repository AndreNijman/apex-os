# gaming-release — fixing the two defects the Gaming Mode hardware run found

Unit `gaming-release`, items **P1-043** and the Gaming Mode half of **P1-038**.
Branch `task/gaming-release` off `roadmap/v2.2` @ `de77ae5d`.

This file is the *fix* record. The *qualification* record it answers is
`ROADMAP/evidence/katana-image-qual-20260919.md` §3, which found the defects
and deliberately did not patch them. Read that first; nothing here repeats its
measurements except where a number is load-bearing.

**Scope, stated up front.** Everything below is either a repository change with
a test that fails in both directions, or a *read-only* measurement taken on
katana. Nothing was installed, patched or left running on katana; the machine
ends this unit exactly as it started it, and greetd was never touched.

---

## 1. Defect 1 — Gaming Mode could not release itself

### 1.1 The shape of the defect, restated so the fix can be judged against it

`files/system/libexec/apex-gaming-session` released game mode from an `EXIT`
trap that ran `apex game stop`. That is D-Bus `GameMode.SetActive(false)`,
behind polkit action `org.apexos.apexd.manage-power`:

```
allow_any      auth_admin
allow_inactive auth_admin
allow_active   yes
```

The instant logind stops calling the session *active*, the session's own
release is refused. §3.4 measured it both ways: the trap logged
`apexd game mode released` on a clean `SIGTERM` to gamescope, and on a
`systemctl restart greetd` it logged `gamescope exited with 139` and then
nothing at all — `apex game status` still read `active : true` 75 minutes
later, with a p-core cpuset, steered IRQs, the `performance` tier and
`scx_lavd` still installed.

Two properties of that failure decided the fix:

1. **The refusal is silent.** The trap was
   `apex game stop >/dev/null 2>&1 && log "apexd game mode released"`, so the
   failed branch printed *nothing*. A machine stuck in a gaming power profile
   had no clue in its own log.
2. **A trap is the wrong place for it regardless.** A session that is
   `SIGKILL`ed runs no trap, so there is no call to authorise however the
   policy is written.

### 1.2 Why the polkit rule was NOT loosened

Giving the *release* path its own `allow_inactive=yes` is one line, and it was
rejected on two grounds:

* `allow_inactive` is **every local session, active or not**. Any unprivileged
  user on the machine could switch Gaming Mode off while someone else is
  playing. The action guards entering *and* leaving the same tuning; only one
  half of it is uncontroversial to hand out.
* It does not close the case it exists for. Property 2 above: no trap runs on a
  `SIGKILL`, so there is nothing for a looser rule to permit.

The problem is not *who may ask*. It is that **the asking is done by something
that may already be dead.**

### 1.3 What was built

`apexd` is root, already holds the session's exit plan in memory, and asks
polkit nothing about itself. It is the only party that still exists after the
session is destroyed, so it owns the release:

| piece | where |
|---|---|
| `SessionOwner`, `owner_state()`, `parse_proc_stat()` | `apexd/apexd-core/src/game.rs` |
| `GameSession.owner`, `game_enter_owned()`, `game_release_if_owner_gone()` | `apexd/apexd/src/game.rs` |
| `GameMode.StartOwnedBy(owner_pid)`, `emit_game_mode_changed()` | `apexd/apexd/src/dbus.rs` |
| the 2 s watch loop | `apexd/apexd/src/main.rs` |
| `Ctx.proc_root` | `apexd/apexd/src/state.rs` |
| `apex game start --owner-pid N` | `apexd/apex/src/main.rs`, `proxy.rs` |
| `--owner-pid $$`, and a trap that cannot be silent | `files/system/libexec/apex-gaming-session` |

Four decisions inside that, each of which could reasonably have gone the other
way and so is written down:

* **`StartOwnedBy` keeps the same polkit action.** *Entering* Gaming Mode is
  exactly as restricted as it was. Nothing in this change makes any new
  operation available to any new caller.
* **The owner is a PID *and a start time*.** PIDs are reused; a bare-PID watch
  would hold game mode engaged for as long as some unrelated process held the
  number — the same failure, quieter. `starttime` is field 22 of
  `/proc/<pid>/stat`, and it is read **by splitting after the last `)`**,
  because field 2 is `comm` and a `comm` may contain spaces *and* parentheses.
  Counting whitespace fields from the start of the line is the classic way to
  read the wrong number out of that file, and on a 2 s watch it would read the
  wrong number forever.
* **An unreadable `/proc` is a third answer, never a release.** `Alive`,
  `Gone(why)` and `Unknown(why)`. Turning an I/O error into a hardware change
  is not a fail-safe. `Unknown` is logged once per run of failures and the
  session is left alone.
* **The owner is watched, not pinned.** `StartForPid` already exists and puts a
  PID in the game cpuset; conflating the two would have put every Steam process
  on the p-cores, which is a behaviour change no run has measured. Ownership
  and cpuset placement stay separate questions.
* **A zombie owner counts as gone.** It has exited and holds nothing; waiting
  for an unreaped parent would mean a greetd that died mid-teardown pins the
  machine indefinitely — which is the defect.
* **Adoption rule:** a session that already has an owner keeps its first one; a
  session with none can be adopted. Undefined would have been worse than either.

The `EXIT` trap survives as the *second* of two paths — it is instant on the
clean exit where the watch would take up to a tick longer, and both call the
same idempotent `game_exit()`, so the two racing is harmless. What it can no
longer do is fail in silence: it now prints polkit's own message and names what
releases the machine instead.

### 1.4 What is tested, and how each assertion was proved to fail

Everything below was mutation-tested: the source was broken in a specific way,
the suite was watched go red on the *named* row, and the file was restored with
plain `cp` and verified byte-identical with `cmp`.

**`apexd/apexd-core/tests/gamemode.rs`** — 7 new rows over `/proc` fixtures.

| mutation | row that went red |
|---|---|
| parse `stat` by whitespace from the start | `starttime_is_read_after_the_last_paren…` (+3 more) |
| `fields.get(19)?` → `unwrap_or(0)` | `a_truncated_stat_line_is_unparseable_rather_than_wrong` |
| `Unknown` collapsed into `Gone` | `an_unreadable_proc_is_a_third_answer_and_never_a_release` |
| drop the zombie rule | `a_zombie_owner_is_gone` |
| ignore the start time (bare-PID watch) | `a_reused_pid_is_gone_and_not_alive` |

**`apexd/apexd/src/game.rs`** — 6 new rows against a real `Ctx` with a
`MockWriter`. These are the ones that hold the *property*, not the call: the
session is destroyed by deleting the owner's `/proc` entry — no trap, no
`apex game stop`, nothing cooperating — and the assertions are that the tier
came back, auto-switch came back, `active` is false in the status a user reads,
and **`Action::ScxStop` is in the writer's record**, i.e. the scheduler was
actually stopped rather than planned to be.

| mutation | row that went red |
|---|---|
| the watch never releases | `an_owner_that_dies_releases_the_machine_with_nothing_cooperating`, `a_reused_owner_pid_is_a_release_and_not_a_reprieve` |
| the watch releases on every tick | `a_live_owner_is_never_released` |
| an unreadable `/proc` releases | `an_unreadable_proc_leaves_the_session_alone_and_says_so` |
| a later owner steals the session | `the_first_owner_of_a_session_keeps_it` |

**`tests/test-apex-gaming-session.sh`** — 10 new rows, 46 total, all passing.
The `apex` fake now **intercepts `apex game …`** so the real binary can never
reach the live system bus; without that, a suite run would enter game mode on
the machine running the tests. The fake records its own `PPID` beside the argv,
which is what lets the suite assert that `--owner-pid` carries *the session
script's own pid* rather than a literal that happens to parse.

| mutation | row that went red |
|---|---|
| drop `--owner-pid` | hands apexd an owner pid / is the script's own pid / fallback message |
| `--owner-pid 1` instead of `$$` | `…and it is the session script's own pid, not a literal` |
| trap fails in silence again | `a refused release is REPORTED, not swallowed` + the polkit-message row |
| remove the older-apexd fallback | `an apexd that refuses --owner-pid still gets a plain game start` |

### 1.5 Two things checked rather than assumed, and one limitation named

**apexd can actually read `/proc`.** A watch that reads `/proc/<pid>/stat` is
worth nothing if the daemon's own unit hides it. `apexd/apexd/apexd.service`
sets `ProtectHome=yes`, `RestrictRealtime=yes`, `MemoryDenyWriteExecute=yes`
and explicitly `ProtectControlGroups=no`; there is **no `ProtectProc=`, no
`PrivateUsers=` and no PID-namespace isolation**, so the daemon sees every
process on the machine. Nothing in this change needed a unit edit.

**apexd restarting mid-session is a pre-existing gap and is not made worse
here.** `GameSession` lives in memory and dies with the daemon — that is stated
in `apexd/src/game.rs`'s own header and predates this work. A *graceful* stop
already releases game mode (`main.rs` calls `game_exit()` on SIGTERM); a crash
with `Restart=on-failure` comes back with no session, so the cpuset, IRQ
affinities and sched-ext stay applied with nothing recorded to undo them. The
owner is simply one more field lost with the rest. Closing that would mean
persisting the exit plan across restarts, which is a different decision about a
different failure and is deliberately not taken here.

### 1.6 What a real run still has to confirm

The property cannot be measured off hardware and **needs an image build**: the
watch lives in `apexd`. The run-book is `docs/gaming-and-sessions.md` §6.6. Its
discriminators on katana are `apex game status` → `active : false`,
`/sys/fs/cgroup/apex-game` removed, and `/sys/kernel/sched_ext/state` →
`disabled`, plus apexd's own `the session owner is gone` line.

**The CPU governor is not a discriminator on katana and the run-book says so.**
Measured 2026-09-19 22:45 with game mode off: `scaling_governor` = `performance`,
`energy_performance_preference` = `performance`, `apex tier` = `* performance`.
That machine's default AC tier already *is* the game tier — §3.4 recorded
`tier: performance  prior_tier: performance` — so the governor reads the same
before, during and after. Quoting it as proof would be a reading that cannot
fail.

---

## 2. Defect 2 — the mangoapp crash loop, diagnosed and removed

### 2.1 The dump says what crashed

Three mangoapp cores were kept on katana for this. `coredumpctl info 261683`,
package `mangohud/0.8.2-2.fc43`:

```
Stack trace of thread 261683:
#0  0x00007f0dd037cc50 XInternAtom (libX11.so.6 + 0x17c50)
#1  0x0000556b4b0e279f main (mangoapp + 0x1f79f)
```

Two frames: `main` calls `XInternAtom` and dies in it — a NULL `Display *`.

### 2.2 The session log says why the Display was NULL

From the `qual-gaming-r2` journal, the lines immediately before each crash:

```
libdecor-gtk-WARNING: Could not get required globals
Failed to load plugin 'libdecor-gtk.so': failed to init
libdecor-cairo-WARNING: Could not get required globals
Failed to load plugin 'libdecor-cairo.so': failed to init
No plugins found, falling back on no decorations
Glfw Error 65550: X11: Platform not initialized     (x4)
[gamescopereaper] reaper: "mangoapp" process shut down. Restarting.
```

`libdecor` is loaded **only by GLFW's Wayland backend**. `65550` is
`GLFW_PLATFORM_UNAVAILABLE`, and the message is what `glfwGetX11Display()`
emits when X11 is not the selected platform — returning NULL. mangoapp hands
that NULL to Xlib. The GLFW error count is exactly 4 per crash, in the session
log and in the A/B below.

### 2.3 The A/B that turns that from a story into a cause

Run on katana 2026-09-19 22:30–22:40, headless, same binaries, `ulimit -c 0`
so nothing was stored, one variable changed:

| gamescope command | mangoapp environment | result |
|---|---|---|
| `gamescope --backend headless --mangoapp -- sleep 22` | `DISPLAY=:0`, `GAMESCOPE_WAYLAND_DISPLAY=gamescope-0` | pid 276839, **same pid 5 s later**, **0 restarts** |
| `gamescope --backend headless --expose-wayland --mangoapp -- sleep 22` | the same **plus `WAYLAND_DISPLAY=gamescope-0`** | pid churn, **171 restarts**, 376 GLFW + 376 libdecor lines |

The environments were read out of `/proc/<mangoapp pid>/environ` in each run,
not assumed. `--expose-wayland` is what puts `WAYLAND_DISPLAY` into the
environment of gamescope's children; GLFW auto-selects Wayland whenever it is
set.

**APEX passed both flags unconditionally.** The combination is ours.

### 2.4 The choice, and why it is cheap

`--expose-wayland` stays; `--mangoapp` goes.

* Native Wayland (xdg-shell) games are worth more than an overlay that has
  never rendered on this system — §3.4: *"the MangoHud overlay never
  appears"*. `--mangoapp` was buying a crash loop and nothing else.
* §6.1 and §6.3 were qualified on hardware **with** `--expose-wayland`, and
  Steam Big Picture reached a UI through it. Removing it would invalidate a
  passing hardware row to rescue a failing one.

It is **gated on the flag rather than deleted**: `APEX_GAMING_EXPOSE_WAYLAND=0`
drops `--expose-wayland` and brings `--mangoapp` back. That is not decoration —
it is what makes the gate testable in both directions, and
`tests/test-apex-gaming-session.sh` asserts all three states (default → no
overlay + an explanation; exposure off → overlay, no exposure; no mangoapp on
PATH → neither the flag nor an explanation of a choice nobody made).

Neither underlying bug is APEX's to fix and neither is worked around: mangoapp
should request the X11 platform or refuse to dereference a NULL `Display`, and
`gamescopereaper --respawn` has no backoff.

### 2.5 The guard that outlives this particular crasher

`files/system/coredump/50-apex-coredump-limits.conf` →
`/usr/lib/systemd/coredump.conf.d/`, `MaxUse=256M`, `KeepFree=2G`.

**The reframe that matters:** the 4.0 GB §3.4 measured was **not** a missing
bound. `coredump.conf(5)`: `MaxUse=` defaults to 10 % of the filesystem
*capped at 4 GiB*, `KeepFree=` to 15 % capped at 4 GiB. katana's `/var` is
954 GB, so the effective cap was 4 GiB and 4.0 GB is exactly it. systemd's
vacuum was working correctly the whole time; the default is simply the wrong
size for an ostree machine, where `/var` is the only writable filesystem and
also holds every container image, every flatpak, the extension `apex-pkg`
builds and the user's home. Filling it breaks `apex update`, `apex install` and
the next boot's sysext rebuild.

`systemd-analyze cat-config systemd/coredump.conf` on katana confirmed the
search path includes `/usr/lib/systemd/coredump.conf.d/*.conf` and that the
shipped `coredump.conf` has **every value commented out** — so the drop-in's
two lines are the only ones in the merged config.

The build assertion runs that same resolution rather than testing that a file
exists, which is the only interesting way to get this wrong (a drop-in in a
directory nothing reads). It was verified **in both directions against the real
`ghcr.io/andrenijman/apex-os:base-d12d3450…` image** before it was written:

```
with the drop-in bind-mounted:     MaxUse=256M / KeepFree=2G   (rc 0)
without it:                        0 matching lines            (rc 1)
```

Stated rather than implied: this bounds the **disk** and nothing else. systemd
has no per-executable rate limit, so a crasher can still flood the journal.
That is the crasher's defect; this is the guard that stops it taking the
machine with it.

---

## 3. The three smaller ones

### 3.1 gamescope's exit segfault is upstream, and now it is *attributed*

One read-only command, `coredumpctl info 247577`:

```
Stack trace of thread 247577:
#0  0x00007f4d3ae58ca0 n/a (n/a + 0x0)
#1  _ZN13CVulkanDeviceD2Ev.lto_priv.0 (gamescope + 0xd0390)
#2  __run_exit_handlers (libc.so.6 + 0x1c371)
#3  exit (libc.so.6 + 0x1c44e)
```

`CVulkanDevice::~CVulkanDevice()` running from `__run_exit_handlers` — a
static-destruction-order crash inside gamescope's own Vulkan teardown, after
`exit()` has already been called. Nothing in it is APEX configuration, and
§3.7 already dated the same crash to the old image (2026-09-19 09:21) and to
`SIGABRT` on 2026-08-23. **Recorded, not chased.** The session script reports
it faithfully (`gamescope exited with 139`) and both release paths still run.

### 3.2 §6.2's check could not discriminate — fixed in the doc

gamescope prints `No CAP_SYS_NICE, falling back to regular-priority` from its
own start-up probe **whether or not `--rt` was passed** (§3.2 saw it with the
flag absent; §0.7 saw the identical two lines on an image that passed it
unconditionally). A reader following the old §6.2 and grepping for
`CAP_SYS_NICE` finds both the session's honest `CAP_SYS_NICE: absent` and
gamescope's warning, and can conclude the opposite of the truth.

§6.2 now tells the reader to read the `starting: gamescope …` line, which is
the only witness to what was actually passed, and says in as many words not to
grep for `CAP_SYS_NICE`.

### 3.3 §6.5's precondition was narrower than reality — fixed in the doc

The niri bar transform runs from `apex-shell-firstrun.service`, a **user**
unit, which starts with `user@<uid>.service` — at boot on this machine, four
hours before any niri session (§3.6: the rewrite is stamped 18:29:35). §6.5
said "after one login to the niri session". It now says the precondition is the
user manager starting, that a niri login is sufficient but not necessary, and
that the check *confirms* the end state rather than causing it.

---

## 4. Verdict, and what is left

**Both defects are fixed in the repository and neither is confirmed on
hardware, because both need an image build.** That is the honest state and it
is not a hedge: the release fix lives in `apexd`, and the mangoapp fix lives in
`/usr/libexec/apex-gaming-session`, and katana runs neither until it is
rebased.

What a real run still has to confirm, in priority order:

1. **§6.6** — that a `systemctl restart greetd` under a live Gaming Mode
   session leaves `active : false`, no `apex-game` cgroup and
   `sched_ext/state = disabled` within a few seconds, with apexd's
   `the session owner is gone` line in its journal. This is the whole point of
   the unit.
2. **§6.7** — that a full session logs zero `Glfw Error` lines, adds zero
   mangoapp dumps, and that the journal for one session is a normal size rather
   than 144 187 lines.
3. The other direction of the mangoapp gate with
   `APEX_GAMING_EXPOSE_WAYLAND=0`: `--mangoapp` back, `--expose-wayland` gone,
   **and the overlay actually rendering** — which is the one thing no machine
   has ever seen it do here.
4. Still open from §3.8 and not this unit's: Safe Graphics' automatic dGPU
   branch (needs the panel genuinely dark), and any actual game launch.

### Katana's state at the end of this unit

Unchanged, and checked rather than assumed at 22:45:

```
greetd                         active
apex game status               active : false
/sys/kernel/sched_ext/state    disabled
/sys/fs/cgroup/apex-game       absent
/var/lib/systemd/coredump      26M  (unchanged; the A/B ran with ulimit -c 0
                               and every entry it made reads Storage: none)
df -h /var                     53G available, unchanged
```

No file was installed, no unit was changed, no package was touched, and the
three kept core dumps are still there.

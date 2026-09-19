# Gaming Mode, Safe Graphics and the niri session

What the 2026-09-19 katana hardware qualification found in these three
sessions, what was changed, and exactly what a machine has to run to close the
rows that a laptop with one GPU cannot answer.

The evidence this is written against is
`ROADMAP/evidence/katana-qualification-20260919.md`. Section numbers below are
that file's.

---

## 1. Gaming Mode opens the card the monitor is on (§6.1)

### What it did

`apex-gaming-session` passed gamescope no device preference. gamescope took its
default — the first DRM node — which on a hybrid laptop is the integrated GPU.
On the MSI Katana that is `card1`, an Intel Iris Xe whose only connector is the
laptop panel. The RTX 3070 driving the user's only external monitor is `card2`,
and gamescope never looked at it.

### The rule, and why it is this one

**The output decides the card.** Not "prefer the discrete GPU": a discrete GPU
with no connector attached is a session with no screen, which is a worse
failure than the one being fixed.

1. Every connector under `/sys/class/drm/card*-*` whose `status` is
   `connected`.
2. External beats internal. `eDP-*`, `LVDS-*` and `DSI-*` are the panel built
   into the chassis; everything else is a cable somebody plugged in. This is
   also what makes the docked-with-the-lid-shut case work, because `eDP-1` is
   still `connected` there.
3. Among equals, by connector name, so two runs on one machine cannot
   disagree. Which of two monitors is "the" gaming monitor is not knowable from
   sysfs.
4. The chosen connector's card supplies `--prefer-vk-device <vendor>:<device>`,
   read from `device/vendor` and `device/device`.

That rule needs no special case for a single-GPU machine or an all-AMD one: on
those, every connector belongs to the only card, so it emits that card's id —
which is what gamescope would have picked anyway — plus a `--prefer-output`
that still moves the session onto the monitor rather than the panel. Nothing
about `10de:249d` is hardcoded anywhere.

`--prefer-vk-device` and not `WLR_DRM_DEVICES`: gamescope's DRM backend is its
own, not wlroots', and it ignored that variable completely when it was tried
(§6.1, second attempt). `--prefer-vk-device` is what moves the DRM node as well
as the Vulkan device, measured.

### Where it lives

| | |
|---|---|
| the rule | `apexd/apexd-core/src/gpu.rs`, `choose_display` |
| the tests | `apexd/apexd-core/tests/gpu_parity.rs` |
| the caller | `apex gaming --gamescope-device-args` |
| the consumer | `files/system/libexec/apex-gaming-session` |

### Failing loudly

A silent fallback to the iGPU *is* the defect being fixed, so there is no
silent path. Every case that cannot produce a complete answer — no connectors,
nothing connected, a card with no PCI id, a connector whose `status` could not
be read — sets a problem string that the session prints and `apex gaming`
raises as a warning.

It is deliberately **not fatal**. Refusing to start would regress every
single-GPU machine over a probe that hiccupped, and gamescope's own default is
correct on those.

A **partial** answer is used rather than discarded. `apex gaming
--gamescope-device-args` exits non-zero when it could not produce a complete
answer — a screen *and* the card to drive it with — and the commonest way that
happens is a DRM node with no PCI device behind it, where `--prefer-output` is
perfectly good and only `--prefer-vk-device` is missing. Throwing the good half
away would be a second silent regression on top of the one being fixed. What
"fail loudly" requires is that the gap is said, not that the session gets less
than it could have.

`APEX_GAMING_NO_DEVICE_SELECT=1` restores the old behaviour, and says what it
is giving up. `APEX_GAMESCOPE_ARGS` is appended after the computed flags, so a
hand-set preference wins.

---

## 2. `--rt` is only claimed when it can be granted (§6.2)

gamescope gates its realtime path on **CAP_SYS_NICE**, not on
`RLIMIT_RTPRIO`. With `/etc/security/limits.d/30-apex-gaming-rtprio.conf`
installed and working — the soft limit really was 20 — the session passed
`--rt`, and gamescope answered

```
No CAP_SYS_NICE, falling back to regular-priority compute and threads.
```

on every attempt. The session now checks the capability (its own `CapEff`, or a
file capability on the gamescope binary) and passes `--rt` only when one holds.
`apex gaming` gained a `realtime capability` row beside the existing `realtime
limit` one; they are separate warnings because a machine can have the limit and
not the capability, which is every APEX machine today.

### Why nothing grants it

* **`setcap` cannot work.** APEX's `/usr` is a read-only composefs, and
  gamescope arrives through a `systemd-sysext` overlay whose lower layers are
  read-only too. There is no writable inode to hang the `security.capability`
  xattr on.
* **`pam_cap` would work, and would break Steam.** It is the direct analogue of
  the limits.d drop-in and it would put capabilities into the **permitted** set
  of every process in the session — including Steam's own `bwrap`, which
  refuses to start with any of them: *"Unexpected capabilities but not setuid,
  old file caps config?"*. That is the message §6.3 recorded and could not
  attribute. Granting CAP_SYS_NICE at login would trade a frame-pacing
  regression for a Steam that does not start.

If a future gamescope RPM ships `cap_sys_nice=ep` on the binary, the session's
`getcap` probe finds it and `--rt` comes back with no change to any of this —
and the Steam interaction above does not arise, because a file capability
applies to gamescope's own exec and not to the session it was started from.

---

## 3. The capability sets are logged, so §6.3 can be attributed next time

`apex-gaming-session` now logs `CapEff`, `CapPrm` and `CapAmb` unconditionally
at start, and says out loud when the permitted set is non-empty, naming what it
breaks. §6.3's bwrap failure was observed and not root-caused; note that the
qualification did **not** log in through greetd — it used
`systemd-run --property=PAMName=login` on VT 2 (§4) — so whether the non-empty
permitted set was the harness's or the image's is still open. The next run
answers it from the session log alone.

---

## 3a. Adaptive sync is asked about the right screen, and the silence is named (§7.2)

`vrr_capable` does not exist anywhere on katana. The NVIDIA connector publishes
seven sysfs attributes and that is not one of them; the Intel connector
publishes fifteen and also lacks it. The old probe globbed every connector on
the machine, matched nothing, passed nothing and **said nothing** — so on a
240 Hz monitor, "this machine has no VRR" and "this driver does not publish the
property" produced identical silence. They are different answers and only one
of them is about the hardware.

The probe now lives in `choose_display` beside the screen it is a question
about, and asks only about the connector this session will actually use. Two
things were wrong with the glob, and the second is the subtler one:

* It would have enabled adaptive sync on the strength of the laptop panel while
  the session ran on the monitor.
* **DRM connector names are unique per card, not per machine.** A hybrid laptop
  has `card1-HDMI-A-1` (the iGPU's own port, usually wired to nothing) *and*
  `card2-HDMI-A-1`. Anything that resolves a connector by name alone answers
  about whichever sorts first — the disconnected one on the wrong GPU. That is
  §6.1's mistake wearing different clothes, and it is why the fixtures now carry
  the namesake connector.

All four outcomes are logged and reported distinctly: the output says `1`, the
output says `0`, the output publishes nothing while other connectors here do,
and nothing on this machine publishes the property at all.

`--adaptive-sync` therefore arrives inside the same `--gamescope-device-args`
output as the device flags, from the same selector, and the session script
holds no DRM logic of its own.

```sh
for p in /sys/class/drm/*/vrr_capable; do [ -e "$p" ] && echo "$p = $(cat "$p")"; done
apex gaming | grep 'adaptive sync'
```

Empty on katana, and on the L16 too — amdgpu does not publish it for `eDP-1`
there either, so `apex gaming` says `not published` rather than `no`. If VRR matters for Gaming Mode on NVIDIA, the property has to
come from somewhere other than this sysfs attribute — that is a separate piece
of work and this row stays COULD NOT RUN.

---

## 4. Safe Graphics and a screen on the second GPU (§5.5)

The pixman software renderer cannot import DMA-BUFs, and wlroots' multi-GPU
path needs exactly that to feed a secondary card's connector. On katana the
recovery session painted the laptop panel and left the external monitor dark,
with 60 `Swapchain for output 'HDMI-A-1' failed test` errors against zero for
`eDP-1`.

`apex-safe-graphics` now:

* names the primary GPU, the outputs it can light and the outputs it cannot,
  in `check` and in its own log, before the compositor starts;
* when every connected screen is on one **non-primary** card, points wlroots at
  that card with `WLR_DRM_DEVICES`. One device means no import, so software
  rendering drives it. labwc *is* wlroots and honours the variable — unlike
  gamescope, §6.1;
* keeps the primary in the mixed case (a live panel and a monitor on the other
  card), because no single device choice can light both, and says which screen
  it is about to leave dark plus the override that recovers on the other one.

`APEX_SAFE_GRAPHICS_DRM_DEVICE=/dev/dri/cardN` forces a device.
`APEX_SAFE_GRAPHICS_RENDERER=auto` lets wlroots pick a hardware renderer; it is
a deliberate opt-out and warns what it undoes, because "the GPU does not work"
is the case this session exists for.

---

## 5. The niri session and niri's own default config (§5.4)

niri's upstream `default-config.kdl` carries `spawn-at-startup "waybar"`, and
that file is what lands in `~/.config/niri/config.kdl` — whether niri writes it
on first run or `apex-shell-firstrun` copies it. firstrun then appended the
APEX autostarts, which start quickshell. Two bars, every login.

`apex-shell-firstrun` now disables that one line, matched in full at column 0
and only in upstream's own spelling, guarded by a marker, a backup, `niri
validate` before and after, and a whole-file hash comparison with that line
removed.

### What else the upstream default turns on — recorded, not changed

These were **not** touched. They are user-visible defaults and changing them is
a decision for Andre, not a side effect of fixing a bar.

| line | what it does | why it is worth a decision |
|---|---|---|
| `hotkey-overlay { // skip-at-startup }` | the "Important Hotkeys" pop-up on every niri start | APEX Shell has its own keybind UI; Hyprland and labwc sessions show nothing equivalent |
| `Mod+T { spawn "alacritty"; }` | terminal | APEX's own terminal choice is foot-first (see `apex-safe-graphics`) |
| `Mod+D { spawn "fuzzel"; }` | launcher | APEX Shell has a launcher |
| `Super+Alt+L { spawn "swaylock"; }` | lock | APEX's lock is compositor-enforced (§5.3); swaylock may not be installed, so this is a bind that does nothing |
| `Super+Alt+S { spawn-sh "pkill orca \|\| exec orca"; }` | screen reader | APEX ships `apex-screen-reader` |

**The binds matter more than they look.** `ApexShellKeybinds.kdl` is created
*empty* by firstrun and stays empty until APEX Settings writes it, so the
`include` that is supposed to override these overrides nothing on a fresh
machine — the stock binds are the only binds there are.

---

## 5a. Leaving Gaming Mode is apexd's job, not the session's

### What it did

`apex-gaming-session` released game mode from an `EXIT` trap that ran
`apex game stop`. On katana 2026-09-19 that worked on a clean gamescope exit
and did nothing at all when greetd was restarted underneath it: `apex game
status` still read `active: true` seventy-five minutes later, with a p-core
cpuset, steered IRQs, the `performance` tier and `scx_lavd` still installed —
and **not one line in the session's own log** to say so.

The cause is not a bug in the trap. `apex game stop` goes through polkit action
`org.apexos.apexd.manage-power`, whose defaults are

```
allow_any      auth_admin
allow_inactive auth_admin
allow_active   yes
```

The instant logind stops calling the session *active* — a greetd restart, a VT
switch away, any logind-driven teardown — the session's own call is refused.
Measured directly, from a session with no seat:

```
$ apex game stop
apex: leaving game mode failed: org.freedesktop.DBus.Error.AccessDenied:
      not authorized for org.apexos.apexd.manage-power
$ sudo apex game stop
apex: game mode OFF
```

So the process that is *supposed* to clean up loses the privilege to do it at
exactly the moment it needs to.

### Why the polkit rule was not loosened

Giving the release path an `allow_inactive=yes` of its own is one line and it
is the wrong line. `allow_inactive` is every local session, active or not, so
any unprivileged user on the machine could switch Gaming Mode off while someone
else is playing. And it still would not close the case it exists for: a session
that is `SIGKILL`ed runs no trap at all, so there is no call to authorise.

The problem is not "who may ask". It is that **the asking is done by something
that may already be dead.**

### The rule, and where it lives

`apexd` is root, holds the session's exit plan in memory, and asks polkit
nothing about itself. It is the only party that still exists after the session
is destroyed, so it takes the job:

* `GameMode.StartOwnedBy(owner_pid)` enters game mode **and** records the
  process whose death ends the session. It is the same polkit action as before
  — *entering* Gaming Mode is exactly as restricted as it was.
* A 2 s watch in `apexd/src/main.rs` reads `/proc/<pid>/stat`; when the owner is
  gone, apexd calls the same idempotent `game_exit()` a D-Bus request would,
  and emits the same signals, so `apex game status` and apex-shell do not keep
  showing a session that is over.
* `apex-gaming-session` passes `--owner-pid $$`.

Three details that are decisions rather than mechanics:

* **A PID is not enough; the start time is recorded with it.** PIDs are reused.
  A bare-PID watch would keep game mode engaged for as long as some unrelated
  process held the number — the same failure, quieter. `starttime` (field 22 of
  `/proc/<pid>/stat`, and read by splitting after the **last** `)`, because
  `comm` may contain spaces and parentheses) is the kernel's own tiebreaker.
* **An unreadable `/proc` is a third answer, and never a release.** Turning an
  I/O error into a hardware change is not a fail-safe. It is logged once and the
  session is left alone.
* **The owner is watched, not pinned.** Putting the session script in the game
  cpuset would put every Steam process on the p-cores, which is a behaviour
  change nothing has measured. `StartForPid`/`AttachPid` remain the way to pin.

The `EXIT` trap is still there, and is now the *second* of two paths: it is
instant on the clean exit, where the watch would take up to a tick longer, and
both call the same idempotent release so the two racing is harmless. What it
can no longer do is fail in silence — it prints polkit's own message and names
what will release the machine instead.

## 5b. MangoHud and `--expose-wayland` cannot both be on

### What it did

`mangoapp` crash-looped at roughly 2 Hz for the whole of **every** Gaming Mode
session on katana: 15 376 core dumps in one boot, 14 403 respawns and 57 612
GLFW lines in a single two-hour session (144 187 journal lines for one login),
and 4.0 GB of stored core dumps on a `/var` that was already 94 % full. The
overlay never drew a pixel.

### The cause, from the dumps and then from an A/B

The backtrace off the kept core dump is two frames long:

```
#0  XInternAtom (libX11.so.6 + 0x17c50)
#1  main (mangoapp + 0x1f79f)
```

— i.e. `XInternAtom(NULL, …)`. What precedes it in the session log names the
reason: `libdecor` plugin failures, which only GLFW's **Wayland** backend loads,
and four `Glfw Error 65550: X11: Platform not initialized` lines, which is
`glfwGetX11Display()` refusing on a non-X11 platform and returning NULL.

A controlled A/B on the same machine, headless, same binaries, one variable:

| gamescope flags | `mangoapp` environment | result in 22 s |
|---|---|---|
| `--mangoapp` | `DISPLAY=:0`, `GAMESCOPE_WAYLAND_DISPLAY=gamescope-0` | same pid alive throughout, **0 restarts** |
| `--expose-wayland --mangoapp` | the same **plus `WAYLAND_DISPLAY=gamescope-0`** | **171 restarts** |

`--expose-wayland` is what puts `WAYLAND_DISPLAY` into the environment of
gamescope's children. GLFW auto-selects its Wayland backend whenever that
variable is set, and mangoapp then hands the NULL X11 display it gets back
straight to Xlib.

### The choice

`--expose-wayland` stays and `--mangoapp` goes. Native Wayland (xdg-shell)
games are worth more than an overlay that has never rendered on this system,
§6.1 and §6.3 were qualified on hardware **with** `--expose-wayland`, and the
overlay was costing a crash loop for nothing.

It is gated on the flag rather than deleted, so `APEX_GAMING_EXPOSE_WAYLAND=0`
brings the overlay back by itself — which is also what makes the gate testable
in both directions in `tests/test-apex-gaming-session.sh`.

Neither half is APEX's to fix: mangoapp should ask GLFW for the X11 platform
(or refuse to dereference a NULL `Display`), and `gamescopereaper --respawn`
has no backoff. Both are recorded rather than worked around. What APEX *does*
own is that a 2 Hz crasher could take `/var` with it, and that is bounded
separately in `files/system/coredump/50-apex-coredump-limits.conf`.

## 6. What still needs katana, and the exact commands

None of the rows below can be answered by a machine with one GPU and no
external monitor. Nothing here was simulated; each row names the command that
closes it.

Run everything from a **greetd login**, not from `systemd-run`, so §6.3's
capability question is answered on the real path (§4 explains why the
qualification could not).

### 6.1 Gaming Mode on the right screen

```sh
# 1. What the selector decides, before rebooting into anything:
apex gaming
apex gaming --gamescope-device-args ; echo "rc=$?"
#    expect: --prefer-vk-device 10de:249d / --prefer-output HDMI-A-1, rc=0
#    and the report's "the screen Gaming Mode will use" block naming card2.

# 2. Pick "APEX Gaming Mode" at the greeter, then afterwards:
journalctl --user -b -o cat | grep -E 'apex-gaming-session|gamescope' | head -40
#    expect in the session log:
#      [apex-gaming-session] GPU/output: --prefer-vk-device 10de:249d --prefer-output HDMI-A-1
#      [gamescope] vulkan: selecting physical device 'NVIDIA GeForce RTX 3070 Laptop GPU'
#      [gamescope] drm: opening DRM node '/dev/dri/card2'
#      [gamescope] drm: selecting connector HDMI-A-1
#      [gamescope] drm: selecting mode 1920x1080@240Hz

# 3. The monitor is the screen that lit, not the panel:
wlr-randr 2>/dev/null || true
nvidia-smi --query-compute-apps=pid,name --format=csv
```

### 6.2 Realtime, in both directions

```sh
grep -E '^Cap(Eff|Prm|Amb):' /proc/self/status     # in the Gaming Mode session
getcap "$(command -v gamescope)"                    # expect: nothing, today
apex gaming | grep -E 'realtime (limit|capability)'
#    expect: realtime limit yes, realtime capability no

# Whether --rt was passed. Read the session's OWN line, and nothing else:
grep -m1 'starting: gamescope' <the session log>
#    expect: no --rt in it, and the apex-gaming-session line above it saying
#            "CAP_SYS_NICE: absent".
```

**Do not check this by grepping for `CAP_SYS_NICE`,** which is what an earlier
version of this section invited. Measured on katana 2026-09-19 (§6.2 in
`ROADMAP/evidence/katana-image-qual-20260919.md`): gamescope prints

```
No CAP_SYS_NICE, falling back to regular-priority compute and threads.
```

**whether or not `--rt` was passed** — it is gamescope's own start-up
capability probe, and the identical two lines appear on an older image where
`--rt` *was* passed unconditionally. So that message discriminates nothing, and
a reader grepping for the string finds both the session's honest
`CAP_SYS_NICE: absent` and gamescope's warning and can conclude the opposite of
the truth. The `starting: gamescope …` line is the only witness to what was
actually passed.

If a later gamescope RPM does carry the capability, the same commands should
show `cap_sys_nice=ep` and `--rt` back in the `starting: gamescope …` line,
with no code change.

### 6.3 Steam inside gamescope — blocked, and on what

Two independent things stop this row, and only one of them is ours:

* **32-bit Vulkan.** `apex install steam` ships no `*_icd.i686.json` at all
  (§6.5), so Steam's 32-bit client has zero ICDs and `BInit - Unable to
  initialize Vulkan!`. The fix is in `files/system/libexec/apex-pkg`, owned by
  unit **pkg-share**. **This row cannot pass until that lands** — do not retest
  it before then.

  ```sh
  ls /usr/share/vulkan/icd.d/ | grep i686     # expect non-empty AFTER pkg-share
  ```

* **bwrap and capabilities.** Once Vulkan works, re-read the session log:

  ```sh
  grep -E 'capabilities:|permitted set' <the session log>
  grep -E 'bwrap|user namespaces' ~/.local/share/Steam/logs/console-linux.txt
  ```

  A zero `CapPrm` with the bwrap message still present means the cause is not
  the session's capabilities and the hunt moves to Steam's runtime. A non-zero
  `CapPrm` means it is, and the session now says so.

  `Unable to open X11 display` is expected to disappear on its own: it was
  downstream of §6.1, where gamescope had already died on `card1`.

### 6.4 Safe Graphics on the dGPU output

```sh
/usr/libexec/apex-safe-graphics check
#    expect: primary gpu card1 / can light eDP-1 / cannot light HDMI-A-1(card2)

# The real test is the emergency shape. Disable the panel, or just force it:
APEX_SAFE_GRAPHICS_DRM_DEVICE=/dev/dri/card2 /usr/libexec/apex-safe-graphics
#    expect: the monitor lights, foot appears on it, and the log has no
#    "Renderer did not support importing DMA-BUFs" for HDMI-A-1.
```

The automatic branch (every connected screen on one non-primary card) needs the
panel genuinely dark. With the lid shut and the machine docked, or with the
panel disabled in firmware, start Safe Graphics from the greeter and expect
`WLR_DRM_DEVICES=/dev/dri/card2` in its log with no override set.

### 6.5 The niri bar

**The precondition is the user manager starting, not a niri login.** The
transform runs from `apex-shell-firstrun.service`, which is a **user** unit and
starts with `user@<uid>.service` — on a machine with lingering or an ssh login
that is at boot, with no graphical session anywhere. Measured on katana
2026-09-19: the line was rewritten at 18:29:35, five minutes after a reboot and
four hours before any niri session (§3.6 of the evidence). So the check below
**confirms** the end state; the niri login does not cause it, and on a machine
whose user manager has already run once there is nothing left for a login to
do.

```sh
# On a machine that had the old config, after its user manager has started
# once — a niri login is sufficient but not necessary:
grep -n 'waybar' ~/.config/niri/config.kdl
#    expect exactly one line, commented, ending in the APEX marker.
pgrep -a -u "$USER" waybar          # expect: nothing
pgrep -a -u "$USER" quickshell      # expect: one
ls ~/.config/niri/config.kdl.pre-apex-bar.bak
niri validate --config ~/.config/niri/config.kdl
```

### 6.6 A Gaming Mode session destroyed *without cooperation*

This is the row §5a exists for, and **it needs an image that carries the
change**: the owner watch lives in `apexd`, so nothing on a machine running an
older build will do anything differently. `apex game status` printing
`owner_pid` is how you know the image is new enough.

Arm a Gaming Mode session the way the qualification run did — the greetd
helpers and the dead-man restore timer are in
`ROADMAP/state/agents/katana-image-qual.md`, and **always arm the restore timer
first.** Then, from ssh while the session is up:

```sh
apex game status
#    expect: active : true, and owner_pid : <the apex-gaming-session pid>
#    owner_pid : 0 means this image predates the watch — stop here, the row
#    cannot pass and the session log says so too.
pgrep -f '^/usr/libexec/apex-gaming-session'   # must equal that owner_pid
```

Record the three things that must come back, **while game mode is on**:

```sh
cat /sys/fs/cgroup/apex-game/cpuset.cpus    # the p-core list
cat /sys/kernel/sched_ext/state             # expect: enabled
apex game status | grep -E '^(tier|prior_tier)'
```

Now destroy the session the way nothing can cooperate with — no signal to the
script, no `apex game stop`, no trap:

```sh
sudo systemctl restart greetd
```

Within a few seconds, with **nothing having asked**:

```sh
apex game status | head -3
#    expect: active : false
test -d /sys/fs/cgroup/apex-game && echo STILL THERE || echo removed
#    expect: removed
cat /sys/kernel/sched_ext/state             # expect: disabled
sudo journalctl -u apexd -b -o cat | grep -m1 'session owner is gone'
#    expect: apexd: game: the session owner is gone (/proc/<pid> is gone)
#            — releasing game mode.
```

**The governor is NOT a discriminator on katana and must not be quoted as
one.** Its profile's AC default tier is already `performance`, so `prior_tier`
and `tier` are both `performance` and `scaling_governor` reads `performance`
before, during and after. The readings that actually move on this machine are
the cgroup, sched-ext and `active`. On a machine whose default tier is
`balanced`, `scaling_governor` is a fourth witness.

Two more worth taking while you are there:

```sh
# The trap's own failure is now visible instead of silent.
sudo journalctl -b -t <session tag> -o cat | grep -A2 'could not release game mode'
#    expect polkit's own AccessDenied message, and a line naming apexd as what
#    releases it instead. BOTH lines, or the log is back to hiding the cause.

# And the belt-and-braces path still works: end a session by SIGTERMing
# gamescope instead, and the trap should release it immediately.
```

### 6.7 The overlay is gone and the dump store is bounded

```sh
grep -m1 'starting: gamescope' <the session log>
#    expect: --expose-wayland present, --mangoapp ABSENT.
grep -m1 'NOT passing --mangoapp' <the session log>
#    expect: the explanation, naming WAYLAND_DISPLAY.

# Nothing crashed, for the whole session — the number must not move.
coredumpctl list --no-pager | grep -c mangoapp      # before and after
journalctl -b -t <session tag> -o cat | grep -c 'Glfw Error'   # expect: 0
journalctl -b -t <session tag> -o cat | wc -l
#    for scale: the 2026-09-19 two-hour session took 144 187 lines.

# And the guard that holds whatever crashes next:
systemd-analyze cat-config systemd/coredump.conf | grep -E '^(MaxUse|KeepFree)='
#    expect: MaxUse=256M and KeepFree=2G. Neither line appears on an image
#    without the drop-in, because Fedora ships every value commented out.
du -sh /var/lib/systemd/coredump
```

To confirm the gate works the other way on real hardware rather than only in
the suite, arm one session with `APEX_GAMING_EXPOSE_WAYLAND=0` in the Exec
environment: `--mangoapp` should be back in the `starting:` line,
`--expose-wayland` gone, and the overlay should actually render — that is the
one thing no machine has ever seen it do here.

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

Two changes. The probe now runs *after* the device selection and asks about the
connector this session is actually going to use — the old glob would have
enabled adaptive sync on the strength of the laptop panel while the session ran
on the monitor. And all three outcomes are logged distinctly: the output says
`1`, the output says `0`, and nothing on this machine publishes the property at
all.

```sh
for p in /sys/class/drm/*/vrr_capable; do [ -e "$p" ] && echo "$p = $(cat "$p")"; done
```

Empty on katana. If VRR matters for Gaming Mode on NVIDIA, the property has to
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
#    and the session log to say "CAP_SYS_NICE: absent" and NOT pass --rt.
```

If a later gamescope RPM does carry the capability, the same two commands
should show `cap_sys_nice=ep` and `--rt` back in the `starting: gamescope …`
line, with no code change.

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

```sh
# After one login to the niri session on a machine that had the old config:
grep -n 'waybar' ~/.config/niri/config.kdl
#    expect exactly one line, commented, ending in the APEX marker.
pgrep -a -u "$USER" waybar          # expect: nothing
pgrep -a -u "$USER" quickshell      # expect: one
ls ~/.config/niri/config.kdl.pre-apex-bar.bak
niri validate --config ~/.config/niri/config.kdl
```

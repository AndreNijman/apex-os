# katana-image-qual — the two run-books that were waiting on an image

items: units `gaming-gpu` (P1-043, P1-038) and `pkg-share` (P0-001), both
RE-OPENED 2026-09-19 (round 33)
repo: apex-os
worktree: `/var/tmp/apex-work/wt-katana-qual3`, branch `task/katana-image-qual-2`
  (created 2026-09-19 21:57 AWST off `roadmap/v2.2` @ f8b8184d; the round-33
  agent's own `task/katana-image-qual` @ 1375efa8 is ALREADY LANDED as merge
  `c13ac17a` and its worktree `wt-katana-qual2` can be removed)
evidence file: `ROADMAP/evidence/katana-image-qual-20260919.md` — now 1213
  lines, §0 through §3, complete. The round-31 run is
  `ROADMAP/evidence/katana-qualification-20260919.md` and is a different file;
  read it, do not overwrite it.
scratch: `/var/tmp/apex-work/scratch-katana-image-qual/` (on katana)

## NEXT

**THE RUN-BOOK IS FINISHED. There is no next action on katana for this unit.**
§0–§3 of the evidence are written, committed as `ef0b95b2` + `60074e42` on
`task/katana-image-qual-2`, and pushed. P1-038 and P1-043 both carry the
hardware readings in `roadmap.yaml`. Both stay `partial`, and §3.8 plus the two
evidence fields say exactly which rows are left and why none of them is an
agent's.

**What a stranger should do with this card:** land
`task/katana-image-qual-2` (two commits, evidence only, no code) and close the
unit. Do **not** dispatch another agent at katana for §6 — every block has been
run on the real machine through a real greetd login and the readings are in the
file.

**greetd was left restored and verified**, and this is the thing to check first
if anything looks wrong:

```
sudo cmp /etc/greetd/config.toml /etc/greetd/config.toml.orig-qual2   -> identical
sudo grep -c initial_session /etc/greetd/config.toml                  -> 0
systemctl is-active greetd                                            -> active
systemctl list-timers qual-greetd-restore.timer                       -> 0 timers listed
loginctl show-seat seat0 -p ActiveSession                             -> ActiveSession=c5
```

The greeter (sway + quickshell) is live on tty1 and Andre can log in normally.
`apex game status` is `active: false`; no gamescope, steam, niri or labwc is
running. Cosmetic leftover: logind still lists closed greeter sessions `c1`–`c4`
in `State=closing` from the repeated greetd restarts — they clear on the next
boot and hold no device.

**If you DO need to arm a session again** (a future run-book, not this one), the
helpers are still on katana and they work:

```
ssh katana
sudo systemd-run --on-active=30min --unit=qual-greetd-restore \
    /usr/bin/bash -c 'cp /etc/greetd/config.toml.orig-qual2 /etc/greetd/config.toml && systemctl restart greetd'
sudo /var/tmp/apex-work/scratch-katana-image-qual/greetd-set.sh <session-id> "<optional exec override>" <tag>
sudo systemctl restart greetd            # greetd-set.sh already cleared /run/greetd.run
sudo journalctl -b -t <tag> -o cat
/var/tmp/apex-work/scratch-katana-image-qual/measure-session.sh <name>
sudo /var/tmp/apex-work/scratch-katana-image-qual/greetd-restore.sh    # ALWAYS LAST
```

Do **not** copy the old card's `--on-calendar='2026-09-19 20:45:00'`; it is in
the past and `systemd-run` will fire it immediately. Keep the unit name
`qual-greetd-restore` — `greetd-restore.sh` stops that timer by name.

## What was found, and what a follow-up round should pick up

Five things came out of §3 that are **not** matrix rows and want owners. None is
fixed here; this unit qualifies, it does not patch.

1. **Gaming Mode cannot release itself when its session is torn down from
   outside.** `cleanup()` in `files/system/libexec/apex-gaming-session` calls
   `apex game stop`, and polkit action `org.apexos.apexd.manage-power` is
   `allow_active=yes` / `allow_inactive=auth_admin` / `allow_any=auth_admin`.
   The instant logind deactivates the session — a greetd restart, a VT switch,
   any logind-driven teardown — the trap's own call is refused, and the machine
   is left with a p-core cpuset, IRQ steering, the `performance` tier and
   `scx_lavd` installed with nothing able to undo them and no prompt anyone will
   see. Measured both ways: the trap logged `apexd game mode released` on a
   clean `SIGTERM` to gamescope and never completed on the greetd restart, and
   `apex game stop` from a seatless ssh session returns
   `org.freedesktop.DBus.Error.AccessDenied: not authorized for
   org.apexos.apexd.manage-power` while `sudo apex game stop` works.
   Two candidate remedies, neither chosen here: have `apexd` release game mode
   when the session that asked for it goes away (it already owns the cgroup), or
   give the *release* path its own `allow_inactive=yes` separate from
   *entering* game mode.
2. **`mangoapp` crash-loops at ~2 Hz for the whole of every Gaming Mode
   session.** 15 376 core dumps in one boot; 14 403 respawns and 57 612
   `Glfw Error 65537/65550: X11: Platform not initialized` lines in a 2-hour
   session (144 187 journal lines for one session). The overlay never renders.
   `--mangoapp` is added unconditionally whenever `mangoapp` is on PATH.
3. **gamescope segfaults on every exit** (`139`, core dumped). Cosmetic today —
   the trap still runs on the clean path — but it predates this image
   (`SIGSEGV` 2026-09-19 09:21 on the OLD image, `SIGABRT` 2026-08-23).
4. **`docs/gaming-and-sessions.md` §6.2 is ambiguous as written.** gamescope
   prints `No CAP_SYS_NICE, falling back to regular-priority` whether or not
   `--rt` was passed, so grepping for `CAP_SYS_NICE` cannot tell the two apart.
   Only the `starting: gamescope …` line discriminates. Worth one doc edit.
5. **§6.5's precondition is stated too narrowly.** The niri bar transform runs
   from `apex-shell-firstrun.service` at **user-manager start** (18:29:35 here,
   four hours before any niri session), not at a niri login. The doc says "after
   one login to the niri session".

Untested and named rather than claimed: **Safe Graphics' automatic dGPU
branch** (needs the panel genuinely dark — lid shut and docked, or disabled in
firmware) and **any actual game launch** (none was started this round).

## DONE

- **22:07 — §3 WRITTEN, COMMITTED `ef0b95b2` (+ `60074e42`), PUSHED. gaming-gpu PASSES on
  hardware.** §6.1, §6.2, §6.3, §6.4 (forced path) and §6.5 all read what the
  run-book predicted, through real greetd logins on the rebased image.
  §6.3 — the row the unit existed for — reached a Steam Big Picture UI inside
  gamescope on the RTX 3070 for the first time on this machine.
  P1-038 and P1-043 evidence extended with `set-status.py` (old text carried
  forward whole; both stay `partial` on grounds §3.8 states).
- **22:00–22:07 — the instrumented run.** Tag `qual-gaming-r2`, 3 789 lines.
  Five gamescope lines verbatim (`NVIDIA GeForce RTX 3070 Laptop GPU`,
  `/dev/dri/card2`, `HDMI-A-1`, `1920x1080@240Hz`); **0** occurrences of `card1`
  or `eDP-1` in the whole log; `CapEff=CapPrm=CapAmb=0`; `--rt` not passed;
  live `nvidia-smi` showing gamescope `C+G` and `steamwebhelper` `G` at 265 MiB;
  `steamwebhelper -uimode=4`. Ended by `SIGTERM` to gamescope → trap released
  game mode → greeter back on tty1 by itself. Then §6.4 (`qual-safegfx`:
  `WLR_DRM_DEVICES=/dev/dri/card2 (forced …)`, labwc holds only card2,
  `wlr-randr` lists HDMI-A-1 as the ONLY output, `grim` on eDP-1 says
  `unknown output`, 699 distinct colours off the monitor, 0 DMA-BUF failures)
  and §6.5 (`qual-niri`: waybar 0, quickshell 1, one commented `spawn-at-startup
  "waybar"` with the APEX marker, `.pre-apex-bar.bak` present, `niri validate`
  clean; niri drives BOTH cards, unlike Gaming Mode).
- **21:55 — the predecessor's session was found already in the journal and it is
  the long run.** Tag `qual-gaming-new`, 144 187 lines, 18:34:01 → 20:45:01
  (2 h 11 m), ended by its own dead-man timer. It agrees line for line with the
  instrumented run and is what proves Steam **survives**: the client stayed up
  the whole 2 h 11 m, its background update loop completing and Fossilize
  replay on the dGPU still going at 20:40:32, 4 m before the cut. Reading it first is what made a short second run enough.
- 21:54 — greetd verified restored **before** anything was changed: the 20:45
  dead-man timer had fired, `config.toml` was byte-identical to
  `config.toml.orig-qual2`, no `initial_session`, greeter live. The fail-safe
  worked; nothing was assumed either way.
- (everything below this line is the round-33 agent's, kept verbatim)
- **18:42 — pkg-share PASSES ON HARDWARE.** `multilib: carrying 2079 of 4382
  file(s) from the 32-bit set (2151 already owned by the image, 152 already
  placed by this set's native packages)` — the discriminator line is present,
  so the new engine built it, and the image-owned count dominates 14:1.
  **Shadow count 177 → 0**, **32-bit ELF shadows 14 → 0**, **i686 ICDs 0 → 13 of
  26**, **GStreamer 2 features → 1344**, fc-list still 538. Evidence §2, commit
  `1375efa8`.
- **18:32 — REBASED.** `bootc switch` pulled 6.2 GB, deployed in 11 s, clean
  reboot, up at 18:29:42. Booted digest
  `sha256:be3bdd0c63848811fbbd3a76f17a50f40e90b1eeb8614b93993ec69419f3aafb`,
  version `apex (2026-09-19T10:09:10Z)`, rollback `apex-266dcc57`.
  **§0.2 confirmed on the machine**: `apex update` delivers the pkg-share fix to
  nobody — the sysext rebuild service short-circuits and the old extension
  re-merges with 0 i686 ICDs. Evidence §1, commit `86e68aab`.
- 18:02 — the landed selector answers `--prefer-vk-device 10de:249d
  --prefer-output HDMI-A-1` on katana's real sysfs (via `APEX_ROOT`, **not**
  `APEX_SYS_ROOT`). Evidence §0.10, commit `ba41fd1c`.
- 17:36 — old-image control column complete (`5036a4f4`): §6.4 printed 4 lines
  with no GPU/output rows, §6.5 ran waybar **and** quickshell. Evidence
  §0.8/§0.9.
- **17:33 — §6.3's bwrap cause attributed to the round-31 harness, not the
  image** (`f6470e36`). All three `bwrap: Unexpected capabilities` lines are
  timestamped 09:19–09:21, i.e. `systemd-run --property=PAMName=login`.
- **17:30 — the greetd route proven end to end** (`431f0e84`): a greetd-launched
  process as `andre` has `CapPrm=CapEff=CapAmb=0`, `CapInh=cap_wake_alarm`,
  `Seat=seat0 TTY=tty1`, and bare `bwrap` returns 0.
- 17:21 — evidence §0 written (`b00e2ae6`): 3 716 extension files, 266 030 image
  paths, **177 shadows**, 14 of them 32-bit ELF, **0 i686 ICDs**,
  `gst-inspect-1.0` → **2 features**.

## FOUND

Everything in the round-33 agent's FOUND list still holds and is not repeated.
The ones a future run on this machine will need again:

- **Katana's NVMe device names are not stable** — address disks by serial
  (`220534D1CB81` = APEX Micron, `240023925111005` = Windows SPCC), PCI
  function, PARTUUID or label. Never `/dev/nvme*n1`. **APEX's `Boot0000` lives
  on the WINDOWS ESP**; do not tidy EFI entries.
- **THE GREETD RUNFILE IS THE SILENT TRAP.** greetd runs an `initial_session`
  only on the first start since boot, checked by `/run/greetd.run`. A restart
  with the runfile present starts the **greeter** and logs nothing unusual.
  `greetd-set.sh` removes it; do not restart greetd by hand instead.
- **greetd does NOT send the session's stderr to the journal** — it dup2s the VT
  onto the session's stdio. `greetd-set.sh` wraps the Exec in
  `systemd-cat -t <tag>`; `systemd-cat` **execs** its target, so the process
  chain, uid and capability sets are unchanged. That redirection is the one
  divergence and it is stated in the evidence.
- **`grim` and `wlr-randr` do not work under gamescope** (its DRM/Wayland
  backend is its own, not wlroots). Render proof for Gaming Mode is therefore
  who holds `/dev/dri/card2` plus `nvidia-smi`, never a screenshot. They work
  fine under labwc (§6.4) and niri (§6.5); `/var/home/andre/qual/shot.py <tag>
  <outputs…>` needs a tag argument and `XDG_RUNTIME_DIR=/run/user/1000` — and
  read the socket name, it is `wayland-0` under labwc and `wayland-1` under
  niri.
- **Do NOT assert `card1-eDP-1/dpms = Off`** during Gaming Mode. It reads
  `enabled=enabled dpms=On` the whole time, because nobody holds card1 once the
  greeter is torn down and the panel keeps its last framebuffer. It is not a
  failure and it is not proof of anything.
- Only two connectors are `connected`: `card1-eDP-1` (`0x8086:0x46a6`) and
  `card2-HDMI-A-1` (`0x10de:0x249d`, Lenovo R25f-30, 1920x1080@239.964).
- `pgrep -x gamescope` finds nothing while Gaming Mode is up — the process
  `comm` is `gamescope-wl`. Use `pgrep -f '^gamescope'`.
- **`apex game stop` over ssh is refused by polkit** (seatless session →
  `auth_admin`). `sudo -n apex game stop` works and raises no prompt, because no
  polkit agent is registered on an ssh session. Use that to reset the baseline.
- katana has **passwordless `sudo`** for `andre` over ssh, which is what makes
  the whole greetd route reachable without Andre's password. No `pam_cap`
  anywhere and no `/etc/security/capability.conf`, on the new image too — that
  is the fidelity argument for skipping only the `auth` stack.
- **`ROADMAP/set-status.py` REPLACES evidence.** Read the existing text and
  carry it forward whole. Both P1-038 and P1-043 were extended that way and
  checked afterwards for the `break_on_hyphens` corruption (none).
- The Bash tool's 600 s cap silently backgrounds a longer command. Wait on the
  journal instead: `timeout N sudo journalctl -b -f -t <tag> -o cat | grep -m1
  '<marker>'` blocks without `sleep` and returns the moment the line appears.

## BLOCKED ON

Nothing. The tag landed, the rebase happened, both run-books ran.

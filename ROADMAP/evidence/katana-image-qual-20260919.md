# katana — qualifying the image that carries gaming-gpu and pkg-share, 2026-09-19

Round 33. Two units closed on 2026-09-19 with the same sentence — RE-OPEN AFTER
AN IMAGE BUILD. Their code is landed and headlessly verified; what was missing
was one run on a machine that carries it.

* `gaming-gpu`, merge `78f04717` — run-book `docs/gaming-and-sessions.md` §6.
* `pkg-share`, merge `5de97037` — the three confirmation numbers in §2 below.

The round-31 run is `ROADMAP/evidence/katana-qualification-20260919.md`. This
file does not replace it; it measures the same machine on a **newer image** and
answers the rows that one could not.

---

## 0. The starting state, measured rather than assumed

Katana was free: uptime 6:37, greetd on `seat0` and `tty1`, nobody logged in, no
`steam`/`gamescope`/`proton`/`wine` process, `rpm-ostree status` idle
(2026-09-19 17:12 AWST).

```
$ rpm-ostree status
* ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2
                   Digest: sha256:ba263890b69619532b13b9c64e0ac5dcb8eced78bffdc9fd542ccac92607503d
                  Version: apex (2026-09-18T14:33:05Z)
  ostree-unverified-registry:ghcr.io/andrenijman/apex-os:gaming-nvidia   (rollback)
                   Digest: sha256:308127d9cefeada90414ae37bdc8175d011c1f851ea9dde1661279a5da5bd89b
                 Unlocked: hotfix
```

`266dcc57` is **131 commits behind** the integration tip and
`git merge-base --is-ancestor` says NO for both `78f04717` and `5de97037`. A
run-book executed on this deployment measures the wrong build, so the rebase is
step 1.

Disk, checked before pulling a multi-GB image:

```
$ df -h /var
/dev/nvme0n1p3  954G  890G   60G  94% /var        (/var, /sysroot and /boot are one filesystem)
```

60 G available, so no `rpm-ostree cleanup -r` — that would drop the
`gaming-nvidia` rollback for no benefit, and `bootc switch` retires the oldest
deployment by itself.

> The device name above is printed by `df` and is **not** how anything in this
> run identifies a disk. Katana's NVMe names reorder across ordinary reboots.
> Nothing here writes to a block device.

### 0.1 The false-negative trap, named before it could fire

A system extension was already merged: `/var/lib/extensions/apex-user.raw`.

```
$ stat -c '%n %s %y' /var/lib/extensions/apex-user.raw
/var/lib/extensions/apex-user.raw 540012544 2026-09-19 09:43:04 +0800
$ jq -r '{os_version_id, pkg_compat_level, built, image_sha256, nres:(.resolved|length)}' \
      /var/lib/apex/pkg/state.json
{ "os_version_id": "43", "pkg_compat_level": 2, "built": "2026-09-19T01:43:06Z",
  "image_sha256": "99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed",
  "nres": 223 }
$ cat /var/lib/apex/pkg/requested
chromium gamemode gamescope libgcc.i686 libSM.i686 mangohud steam steam-devices
```

The standing warning was that this is the 2026-09-06 extension (219 packages,
~337 MB). **It is not** — round 31 rebuilt it at 01:43 UTC today. The substance
is unchanged and matters more than the date: it was built by the engine **in the
image**, which predates pkg-share, and a system extension survives a rebase and
re-merges at boot. Reading `ls /usr/share/vulkan/icd.d/ | grep -c i686` before a
genuine rebuild therefore returns 0 and looks exactly like pkg-share failing.

**The discriminator is the line `multilib: carrying` in the install log.** Only
the new engine emits it (`merge_multilib`, `files/system/libexec/apex-pkg`
~line 698). Its absence means the old extension was measured, not that the fix
failed.

### 0.2 DEFECT (new, found by reading the engine before running it) — the pkg-share fix does not reach an existing machine through `apex update`

`rebuild_extension` stops early when the resolved rpm set, `os_version_id` and
`pkg_compat_level` all match and the sysext is merged:

```bash
    if [ -f "$STATE" ] && merged && [ -n "$(current_payload)" ] \
       && [ "$(jq -r '.os_version_id // empty' "$STATE")" = "$(os_version)" ]; then
        old_set="$(jq -r '.resolved[]?' "$STATE" | sort)"
        if [ "$new_set" = "$old_set" ]; then
            msg "already up to date"
```

and `apex-sysext-rebuild.service` — the unit whose whole job is to notice that
the OS moved — runs `rebuild --if-needed`, which returns 0 on the same
three-way match. `os_version()` is `VERSION_ID` out of `/usr/lib/os-release`,
and on katana that is **`43` before and after an APEX image build**. The
extension's `pkg_compat_level` is 2 and the engine's constant is still 2.

So none of the three changes across this rebase, and an existing machine keeps
its old-engine extension — with 0 i686 ICDs and 14 32-bit shadows — after
`sudo apex update`. Verified on the machine in §2.1 rather than left as a
reading.

**And the obvious remedy does not work, which is the more useful half of this
finding.** `PKG_COMPAT_LEVEL` exists for exactly this case — its own comment
says "increment whenever a newly baked image package may overlap existing user
extensions" — but bumping it 2→3 would *appear* to fix this and would not,
because **the two guards check different things**:

| guard | checks `os_version_id` | checks resolved set | checks `pkg_compat_level` |
|---|---|---|---|
| `cmd_rebuild --if-needed` (~line 1613) | yes | no | **yes** |
| `rebuild_extension`'s "already up to date" (~line 1306) | yes | yes | **no** |

After a bump, `rebuild --if-needed` would log "extension compatibility changed
… — rebuilding", call `rebuild_extension`, re-download every rpm, hit "already
up to date" because the resolved set is unchanged, and then call `write_state`
— **which stamps the new level into `state.json`**. The next boot sees a
matching level and no-ops. One wasted download, no rebuild, and the marker
consumed: a fix that hides the fact that it did nothing.

The real fix is to add the compat-level comparison to `rebuild_extension`'s own
short-circuit (or give bare `rebuild` an explicit force path that skips it),
*then* bump the level, with a test that fails without the first half. Left for a
follow-up unit; not patched here, because this unit's deliverable is the
hardware run and the deadline is real.

### 0.3 The §3.1 shadow baseline, re-measured by this unit on the old image

Round 31's numbers are reproduced here by this agent, on this deployment, so
the post-rebase numbers are a same-agent like-for-like delta. `LC_ALL=C` on
every `sort` and `comm`.

```
$ sudo mount -o ro,loop /var/lib/extensions/apex-user.raw /mnt/apexext-qual2
$ sudo find /mnt/apexext-qual2/usr -mindepth 1 \( -type f -o -type l \) -printf "/usr/%P\n" \
    | LC_ALL=C sort > ext-pre.txt
$ rpm -qal | LC_ALL=C sort -u > image-pre.txt
$ LC_ALL=C comm -12 ext-pre.txt image-pre.txt | wc -l
177
```

| | old image, 2026-09-19 17:20 AWST |
|---|---|
| extension files + symlinks | 3 716 |
| image paths (`rpm -qal`) | 266 030 |
| **paths the extension shadows** | **177** |
| of those, 32-bit ELF over a 64-bit image binary | **14** |
| `ls /usr/share/vulkan/icd.d/ \| grep -c i686` | **0** (13 files, all `x86_64`) |
| `/usr/share/vulkan/icd.d/` inside the extension | **does not exist** |
| `gst-inspect-1.0 \| tail -1` | `240 plugins (239 blacklist entries not shown), 2 features` |

The 14 are the same 14 round 31 named — at-spi2's bus launcher and registryd,
`dconf-service`, `gio-launch-desktop`, `glib-pacrunner`, four glycin loaders,
four GStreamer helpers including `gst-plugin-scanner`, and `p11-kit-remote`.

### 0.4 The greetd question, and the route this run takes

Round 31 could not log in through greetd (§4 there): `apex-session-select`
deliberately arms no autologin and that unit did not have Andre's password.
Neither does this one, and it must not ask. `sudo -n true` on katana **does**
succeed, so the route is a temporary `initial_session` in
`/etc/greetd/config.toml`, backed up, restored and `cmp`-verified afterwards.

greetd's initial session skips `pam_authenticate` and still runs `acct_mgmt` →
`setcred` → `open_session`. The fidelity argument is a measurement, not an
assertion:

```
$ sudo grep -rn -i 'pam_cap\|capability' /etc/pam.d/{greetd,system-auth,postlogin,login}
(no output)
$ ls /etc/security/capability.conf
ls: cannot access '/etc/security/capability.conf': No such file or directory
```

Nothing in the `auth` stack on this machine can grant a capability, so skipping
it cannot change `CapPrm` — which is the only thing §6.3 asks the login path
about. For the same reason the greeter's own process already reads an empty
permitted set:

```
$ sudo grep -E '^Cap(Inh|Prm|Eff|Bnd|Amb):' /proc/<sway, user greetd>/status
CapInh: 0000000000000000   CapPrm: 0000000000000000   CapEff: 0000000000000000
CapBnd: 000001ffffffffff   CapAmb: 0000000000000000
$ systemctl show greetd -p AmbientCapabilities
AmbientCapabilities=
```

This grep is repeated on the new image in §3 before any capability number is
believed.

### 0.5 The negative control — five readings that must flip

Taken on the old deployment at 17:20 AWST so "the image changed" is a
measurement rather than a hope:

```
$ apex gaming --gamescope-device-args ; echo "rc=$?"
error: unexpected argument '--gamescope-device-args' found
rc=2
$ grep -c 'prefer-vk-device\|prefer-output' /usr/libexec/apex-gaming-session   -> 0
$ grep -c 'CapEff\|CapPrm\|CapAmb'         /usr/libexec/apex-gaming-session   -> 0
$ getcap "$(command -v gamescope)" ; echo "rc=$?"                              -> (empty) rc=0
$ for p in /sys/class/drm/*/vrr_capable; do [ -e "$p" ] && echo "$p"; done      -> (no matches)
```

The first three must change on the new image; `getcap` and `vrr_capable` must
**not** — nothing in either unit grants a file capability or invents a sysfs
attribute, and §6.2 and §7.2 both depend on that staying true.

One thing already correct on the old image and worth recording because round 31
called it out: the EACCES-vs-absent distinction on the `sudoers rule` row is
present — it reads `not measured — could not read the path: Permission denied
(os error 13)`, not a bare `no`.


### 0.6 The greetd route, proven end to end — and §6.3's capability question answered

Run on the old deployment at 17:29 AWST, before the rebase, to de-risk the
mechanism. It answered more than it was meant to.

**The trap that made the first attempt look like a failure**, worth writing
down because it is silent: greetd starts the initial session **only on the
first greetd start since boot**, and `greetd(5)` says this "is checked through
the presence of the runfile". A `systemctl restart greetd` with
`[initial_session]` armed and `/run/greetd.run` present starts the **greeter**
instead, logs nothing unusual, and looks exactly like the config being ignored.
`rm -f /run/greetd.run` before each restart is the whole trick, and it is now
inside `greetd-set.sh`.

The second detail: greetd dup2s the VT onto the session's stdin/stdout/stderr,
so a session's own log is painted on tty1 and is unreadable over ssh. The
armed command is therefore wrapped in `/usr/bin/systemd-cat -t <tag>`, which
**execs** its target — the process chain, uid and capability sets are unchanged
— and the log reads back with `journalctl -b -t <tag>`. That redirection is the
only divergence from an ordinary greetd login, and it is downstream of
everything §6.3 asks about.

With that, a greetd-launched process running as `andre`:

```
Sep 19 17:29:08 apex qual-probe[…]: id: uid=1000(andre) gid=1000(andre) groups=1000(andre),10(wheel)
                                    context=unconfined_u:unconfined_r:unconfined_t:s0-s0:c0.c1023
CapInh: 0000000800000000      <- cap_wake_alarm, and only that
CapPrm: 0000000000000000
CapEff: 0000000000000000
CapBnd: 000001ffffffffff
CapAmb: 0000000000000000
NoNewPrivs: 0   Seccomp: 0
--- its parent, greetd itself ---
Name: greetd   Uid: 0
CapPrm: 000001ffffffffff   CapEff: 000001ffffffffff   CapAmb: 0000000800000000
```

`capsh --decode=0000000800000000` → `cap_wake_alarm`. greetd runs as root with
`cap_wake_alarm` in its **ambient** set; ambient is dropped when it drops to
uid 1000, and what survives into the session is an *inheritable* bit and
nothing else.

logind classifies the result on the seat the greeter had:

```
Id=309 User=1000 Name=andre Seat=seat0 TTY=tty1 Class=user Active=yes State=active
```

**and bwrap runs:**

```
$ getcap /usr/bin/bwrap                                   -> (nothing)
$ bwrap --ro-bind / / --dev /dev /bin/true ; echo rc=$?    -> rc=0
```

#### What this attributes

`docs/gaming-and-sessions.md` §6.3 states the test: "A zero `CapPrm` with the
bwrap message still present means the cause is not the session's capabilities
and the hunt moves to Steam's runtime. A non-zero `CapPrm` means it is."

**`CapPrm` is zero on the real login path, and a plain `bwrap` succeeds in it.**
So the permitted set round 31 suspected is not a property of an APEX login —
the session it measured was started by `systemd-run --property=PAMName=login`,
which is where any non-zero permitted set came from. The remaining candidate is
the bwrap inside Steam's own runtime, which is a different binary from
`/usr/bin/bwrap`. §3 re-runs this on the new image through the actual Gaming
Mode session, whose script now logs the same three values itself.

Also confirmed here, and consistent with round 31 §4: `XDG_SESSION_TYPE=tty` in
the process environment even with `Seat=seat0`/`TTY=tty1`, because `pam_systemd`
sets it for a session that has not yet declared a compositor. That is a
property of the environment variable, not of the session; the logind `Type`
field is the real one.

`/etc/greetd/config.toml` was restored from `/etc/greetd/config.toml.orig-qual2`
and `diff` reports the two identical; greetd is `active` with the greeter back
on tty1.


### 0.7 §6.3's bwrap failure is ATTRIBUTED — it was the round-31 harness, not the image

The probe in §0.6 said the permitted set is zero. This is the same question
asked the expensive way: **Gaming Mode itself, launched by greetd**, on the old
deployment, at 17:31 AWST. It is a same-harness negative control for §6.1 and
§6.2 as well, so it is worth all of its lines.

`greetd-set.sh apex-gaming` → `[initial_session] command = "/usr/bin/systemd-cat
-t qual-gaming-old /usr/libexec/apex-gaming-session"`, `rm /run/greetd.run`,
`systemctl restart greetd`.

**§6.1 reproduced through the real login path** (round 31 saw this under
`systemd-run`; it is not an artefact of that):

```
[apex-gaming-session] starting: gamescope -e -f --expose-wayland --rt --mangoapp -- steam -gamepadui
[gamescope] vulkan: Intel device detected, forcing general queue family instead of compute-only queue
[gamescope] vulkan: selecting physical device 'Intel(R) Iris(R) Xe Graphics (ADL GT2)': queue family 0
[gamescope] drm: opening DRM node '/dev/dri/card1'
[gamescope] drm: Connector eDP-1 -> AUO -
[gamescope] drm: Connectors:
[gamescope] drm:   eDP-1 (connected)
[gamescope] drm: selecting connector eDP-1
[gamescope] drm: selecting mode 1920x1080@144Hz
```

`HDMI-A-1` is not in gamescope's connector list at all, because it belongs to
card2. Who held a DRM node while the session ran:

```
Xwayland      /dev/dri/card1
gamescope-wl  /dev/dri/card1
systemd-logind /dev/dri/card1
```

Nothing touched card2. The machine's own sysfs says what the selector has to
work from, and it is unambiguous:

```
card1-eDP-1     connected   card1  vendor=0x8086 device=0x46a6   (Intel Iris Xe)
card2-HDMI-A-1  connected   card2  vendor=0x10de device=0x249d   (RTX 3070 Laptop)
```

Those are the only two connectors that exist on this machine, connected or not.

**§6.2 reproduced**: `No CAP_SYS_NICE, falling back to regular-priority compute
and threads. Performance will be affected.` — the old script passes `--rt`
unconditionally and the old image logs no capability line at all.

#### And the row this unit was sent to answer

Steam's log is append-only, so the two runs are side by side in one file.
**Every** bwrap/user-namespace line in it belongs to round 31:

```
$ grep -nE 'bwrap|user namespaces' ~/.local/share/Steam/logs/console-linux.txt
37218:[2026-09-19 09:19:06] steam-runtime-check-requirements[24113]: W: Child process exited with code 1:
                            bwrap: Unexpected capabilities but not setuid, old file caps config?
37220:[2026-09-19 09:19:06] steam.sh[23995]: Error: Steam now requires user namespaces to be enabled.
37230:[2026-09-19 09:20:32]  … same, second attempt
37242:[2026-09-19 09:21:26]  … same, third attempt
```

Three lines, three timestamps, all of them 09:19–09:21 — the
`systemd-run --property=PAMName=login` runs. The greetd run starts at line
37522:

```
37522:[2026-09-19 17:31:41+0800] srt-logger[19379]: Log opened
37523:[2026-09-19 17:31:41] steam.sh[19367]: Running Steam on fedora 43 64-bit
…
[2026-09-19 17:31:44] bus_name=com.steampowered.PressureVessel.LaunchAlongsideSteam
```

**Not one bwrap error, and pressure-vessel came up.** The requirements check
that failed three times under the harness passes on the real login path.

**Verdict: §6.3's bwrap cause is the harness.** `systemd-run
--property=PAMName=login` gave the session a non-empty permitted set; a greetd
login gives it `CapPrm=0` (§0.6) and Steam's runtime is satisfied. Nothing in
the image needed fixing for this, and the row can stop being carried as an open
defect. The instruction in `docs/gaming-and-sessions.md` §6.3 — "run everything
from a greetd login, not from `systemd-run`" — is the finding, now with the
measurement behind it.

What still fails in the same run is the **other** half of §6.3, exactly as
predicted:

```
[2026-09-19 17:31:46] Vulkan missing requested extension 'VK_KHR_surface'.
[2026-09-19 17:31:46] Vulkan missing requested extension 'VK_KHR_xlib_surface'.
[2026-09-19 17:31:46] BInit - Unable to initialize Vulkan!
```

That is the 32-bit ICD defect pkg-share fixes, and §2 is where it is retested.

#### Cleanup, again

```
$ apex game status        (during)  -> active : true   cgroup /sys/fs/cgroup/apex-game
$ apex game status        (after)   -> active : false
$ pgrep -c -u andre -x gamescope    -> 0
$ diff /etc/greetd/config.toml /etc/greetd/config.toml.orig-qual2   -> identical
$ systemctl is-active greetd        -> active
```

The `trap cleanup EXIT HUP INT TERM` released game mode on the greetd path too,
and the greeter came back on tty1 by itself.


### 0.8 The remaining two blocks, on the old image, through the same greetd harness

**§6.4 — Safe Graphics has no idea which GPU it is on.** The whole output of
`check` on the old deployment:

```
$ /usr/libexec/apex-safe-graphics check ; echo rc=$?
config      /usr/share/apex/safe-graphics
compositor  /usr/bin/labwc
terminal    foot
renderer    pixman (software)
rc=0
$ grep -c WLR_DRM_DEVICES /usr/libexec/apex-safe-graphics                 -> 0
$ grep -c 'primary gpu\|can light\|cannot light' /usr/libexec/apex-safe-graphics -> 0
```

Four lines, none of them about a GPU or an output. The `primary gpu` / `can
light` / `cannot light` rows the run-book expects do not exist yet, and neither
does the `WLR_DRM_DEVICES` branch.

**§6.5 — the two-bar defect, reproduced through greetd.** `greetd-set.sh niri`,
runfile cleared, greetd restarted:

```
$ pgrep -a -u andre waybar       -> 31621 waybar
$ pgrep -a -u andre quickshell   -> 31630 quickshell -c /usr/share/apex-shell
$ pgrep -a -u andre -x niri      -> 31531 niri --session
$ grep -n waybar ~/.config/niri/config.kdl
270:// This line starts waybar, a commonly used bar for Wayland compositors.
271:spawn-at-startup "waybar"
$ ls ~/.config/niri/config.kdl.pre-apex-bar.bak
ls: cannot access '…': No such file or directory
```

Two bars, and the precondition for the fix is intact — the old image's
`apex-shell-firstrun` contains no `pre-apex-bar` or `NIRI_STOCK_WAYBAR` string
at all (`grep -c` → 0), so it left the user's config alone, as it must.

Both outputs are live in this session, which is the baseline §6.1 has to beat:

```
eDP-1     AU Optronics 0x978F              1920x1080 @ 144.028 Hz   Adaptive Sync: disabled
HDMI-A-1  Lenovo Group Limited R25f-30     1920x1080 @ 239.964 Hz   (serial URW0PNMC)
```

greetd restored byte-identical and restarted after each of these runs.

### 0.9 Summary of the old-image controls

Everything below was measured by this unit, on this machine, through the same
greetd harness the new-image run will use. It is the left-hand column of every
row in §2 and §3.

| row | old image, 2026-09-19 17:2x–17:3x |
|---|---|
| `apex gaming --gamescope-device-args` | `error: unexpected argument`, rc 2 |
| gamescope's Vulkan device | `Intel(R) Iris(R) Xe Graphics (ADL GT2)` |
| gamescope's DRM node | `/dev/dri/card1` — card2 untouched by anything |
| gamescope's connector | `eDP-1` @ 1920x1080@144; `HDMI-A-1` not in its list |
| `--rt` | passed unconditionally; `No CAP_SYS_NICE, falling back` |
| session capability log | absent (the script does not log one) |
| greetd session `CapPrm` | `0000000000000000` |
| bwrap in the greetd run | **no error; pressure-vessel starts** |
| Steam Vulkan | `BInit - Unable to initialize Vulkan!` |
| i686 ICDs | 0 of 13 |
| extension paths shadowing the image | 177, of which 14 are 32-bit ELF |
| `gst-inspect-1.0` | 240 plugins, **2 features** |
| `apex-safe-graphics check` | 4 lines, no GPU or output rows |
| niri session | waybar **and** quickshell — two bars |
| `apex game` cleanup | released every time, 0 gamescope left |


### 0.10 What the landed selector says about katana, before katana has it

The image was still building, so the selector was run against katana's **real
sysfs**, copied off the machine, using the code from this branch. `choose_display`
takes the sysfs root as an argument and the CLI honours `APEX_ROOT`, so this is
the shipped rule on the shipped inputs — not a fixture somebody wrote to pass.

The snapshot, every file of it (`/sys/class/drm` on katana at 17:59 AWST):

```
class/drm/card1-eDP-1/status     = connected      class/drm/card1/device/vendor = 0x8086
class/drm/card1-eDP-1/enabled    = enabled        class/drm/card1/device/device = 0x46a6
class/drm/card2-HDMI-A-1/status  = connected      class/drm/card2/device/vendor = 0x10de
class/drm/card2-HDMI-A-1/enabled = enabled        class/drm/card2/device/device = 0x249d
```

```
$ cargo build --bin apex                                   # branch task/katana-image-qual
$ APEX_ROOT=<snapshot> ./target/debug/apex gaming --gamescope-device-args ; echo rc=$?
HDMI-A-1 on card2 is an external display; 2 connected output(s) across 2 card(s)
no connector on this machine publishes vrr_capable at all, so this is 'the driver does not
say' and NOT 'the display has no VRR'. On NVIDIA that is expected; the property has to come
from somewhere else
--prefer-vk-device
10de:249d
--prefer-output
HDMI-A-1
rc=0

$ APEX_ROOT=<snapshot> ./target/debug/apex gaming | grep -A4 'the screen Gaming Mode will use'
── the screen Gaming Mode will use ──
output             : HDMI-A-1 on card2 (NVIDIA)
gamescope device   : --prefer-vk-device 10de:249d
why                : HDMI-A-1 on card2 is an external display; 2 connected output(s) across 2 card(s)
adaptive sync      : not published — no connector on this machine has vrr_capable
```

That is the §6.1 expectation stated as a number before the run, and the §7.2
wording confirmed against hardware that genuinely publishes nothing.

**A free single-GPU parity check fell out of it.** The same binary run against
the L16's own `/sys` — one card, one connected output, an AMD Radeon 780M —
answers `output: eDP-1 on card1 (AMD)`, `--prefer-vk-device 1002:1900`,
`why: eDP-1 on card1 is the built-in panel; 1 connected output(s) across 1
card(s)`. So the rule needs no special case for a machine with one GPU: it
names the only card it can see and still emits a `--prefer-output`, which is
the claim `docs/gaming-and-sessions.md` §1 makes. Nothing about `10de:249d` is
hardcoded — the two machines produce different ids from the same code path.


---

## 1. The rebase

```
$ sudo bootc switch --transport registry \
      ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e
layers already present: 69; layers needed: 44 (6.2 GB)
Deploying...done (11 seconds)
Queued for next boot: ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e
  Digest: sha256:be3bdd0c63848811fbbd3a76f17a50f40e90b1eeb8614b93993ec69419f3aafb
$ sudo systemctl reboot        # clean reboot, never a hard reset
```

The tag was published at **18:18:30 AWST**; CI 35433705393's tiers finished
`rust` → `changes` → `core` (~53 min) → `base` → `image`. `bootc switch` did
**not** refuse despite the rollback deployment being `Unlocked: hotfix`, so the
2026-09-17 hand-copied `/usr` overlay needed no special handling.

**Landed, read back after the reboot:**

```
$ rpm-ostree status
* ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e
                   Digest: sha256:be3bdd0c63848811fbbd3a76f17a50f40e90b1eeb8614b93993ec69419f3aafb
                  Version: apex (2026-09-19T10:09:10Z)
  ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2   (rollback)
                   Digest: sha256:ba263890b69619532b13b9c64e0ac5dcb8eced78bffdc9fd542ccac92607503d
```

Up at 18:29:42, `State: idle`, the previous deployment kept as the rollback.

### 1.1 The five controls, flipped — and the two that must not

Same commands as §0.5, same machine, 18:30 AWST:

| reading | old image | new image |
|---|---|---|
| `apex gaming --gamescope-device-args` | `error: unexpected argument`, rc 2 | `--prefer-vk-device 10de:249d` / `--prefer-output HDMI-A-1`, **rc 0** |
| `grep -c 'prefer-vk-device\|prefer-output'` in the session script | 0 | **3** |
| `grep -c 'CapEff\|CapPrm\|CapAmb'` in the session script | 0 | **7** |
| `getcap $(command -v gamescope)` | empty, rc 0 | empty, rc 0 — **unchanged, as required** |
| any `vrr_capable` in sysfs | none | none — **unchanged, as required** |

**The selector's answer on the real machine is byte-identical to the prediction
§0.10 made from the sysfs snapshot**, including the prose:

```
── the screen Gaming Mode will use ──
output             : HDMI-A-1 on card2 (NVIDIA)
gamescope device   : --prefer-vk-device 10de:249d
why                : HDMI-A-1 on card2 is an external display; 2 connected output(s) across 2 card(s)
adaptive sync      : not published — no connector on this machine has vrr_capable
```

and §6.2's two rows already read the way the run-book wants, from ssh, before
any session is started:

```
realtime limit     : yes
realtime capability: no (/proc/self/status)
```

`pam_cap` is still absent from `/etc/pam.d/{greetd,system-auth,postlogin,login}`
on the new image and `/etc/security/capability.conf` still does not exist, so
§0.6's fidelity argument for the `initial_session` route holds here too.

### 1.2 §0.2 CONFIRMED ON THE MACHINE — the image update delivered the fix to nobody

This is the first thing measured after the reboot, before anything was touched:

```
$ sudo journalctl -u apex-sysext-rebuild -b
Sep 19 18:29:40 apex systemd[1]: Starting apex-sysext-rebuild.service - Rebuild APEX user packages…
Sep 19 18:29:40 apex systemd[1]: Finished apex-sysext-rebuild.service - Rebuild APEX user packages…
$ systemctl show apex-sysext-rebuild -p Result -p ExecMainStatus
Result=success   ExecMainStatus=0
```

Started and finished **in the same second**, successfully, having done nothing.
The proof that it did nothing is the extension itself:

```
$ sudo sha256sum /var/lib/extensions/apex-user.raw
99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed   <- byte-identical to §0.1
$ sudo jq -r '{os_version_id,pkg_compat_level,built,image_sha256}' /var/lib/apex/pkg/state.json
{ "os_version_id": "43", "pkg_compat_level": 2, "built": "2026-09-19T01:43:06Z",
  "image_sha256": "99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed" }
$ systemd-sysext status
/usr   apex-user   Sat 2026-09-19 18:29:33 AWST      <- the OLD extension, re-merged
$ ls /usr/share/vulkan/icd.d/ | grep -c i686
0
```

**A machine that takes this image update still has zero 32-bit Vulkan ICDs.**
The engine on disk is fixed; the extension built by the old engine is what is
merged over `/usr`, and nothing in the update path rebuilds it. §0.2 predicted
this from reading the two guards; this is the measurement.


## 2. pkg-share — CONFIRMED, and by a wider margin than the closure claimed

`sudo apex install steam`, run under `systemd-run --unit=qual-install-steam
--property=TimeoutStartSec=infinity` so it could not be cut short by an ssh
that went away.

### 2.1 Which engine ran — established before any count was believed

```
$ grep 'multilib: carrying' apex-install-steam-merged.log
apex-pkg: multilib: carrying 2079 of 4382 file(s) from the 32-bit set
          (2151 already owned by the image, 152 already placed by this set's native packages)
```

That line exists only in the new `merge_multilib`, so the extension measured
below was built by the fixed engine. **The image-owned count dominates the
native-pass count by 14:1** — 2151 paths dropped because the running image
already provides them, 152 because this transaction's own 64-bit pass had
already placed them. That ratio is the shape the rule predicts: most of what an
i686 dependency closure carries is a second copy of something the OS already
has.

**Why it rebuilt at all, stated honestly:** not because the update path asked it
to (§1.2 shows it did not), but because Fedora's repositories had moved since
01:43 — the log shows new EVRs such as `NetworkManager-libnm-1:1.54.3-3.fc43` —
so the resolved set differed and `rebuild_extension`'s "already up to date"
comparison failed. **On a machine whose repositories had not drifted, this
command would have printed `already up to date` and changed nothing.** The
rebuild was luck, not design, and §0.2 remains a defect.

### 2.2 The three numbers

Same script, same machine, same `LC_ALL=C`, against the new `.raw`:

| | old engine (§0.3) | new engine |
|---|---|---|
| extension files + symlinks | 3 716 | 3 564 |
| image paths (`rpm -qal`) | 266 030 | 266 093 |
| **paths the extension shadows** | **177** | **0** |
| **32-bit ELF over a 64-bit image binary** | **14** | **0** |
| **`ls /usr/share/vulkan/icd.d/ \| grep -c i686`** | **0** (13 files) | **13** (26 files) |
| `gst-inspect-1.0 \| tail -1` | 240 plugins, **2 features** | 240 plugins, **1344 features** |
| `fc-list \| wc -l` | 538 | 538 |
| extension size | 540 012 544 B | 535 928 832 B |

`comm -12` between the extension's file list and the image's `rpm -qal` returns
**nothing at all**. Not "fewer" — the set is empty, and so is the 32-bit subset
of it.

### 2.3 The thirteen manifests, by name

```
$ ls /usr/share/vulkan/icd.d/ | grep i686
asahi_icd.i686.json      broadcom_icd.i686.json   dzn_icd.i686.json
freedreno_icd.i686.json  intel_hasvk_icd.i686.json intel_icd.i686.json
lvp_icd.i686.json        nouveau_icd.i686.json    nvidia_icd.i686.json
panfrost_icd.i686.json   powervr_mesa_icd.i686.json radeon_icd.i686.json
virtio_icd.i686.json
```

`nvidia_icd.i686.json` is the file the 2026-09-17 live recovery had to
hand-write, because the image does not ship it and the old engine threw it away
with the blanket `--excludepath /usr/share`. It now arrives from the rpm that
owns it. **This is the gate on P1-038 row 6 (Steam Big Picture), and it is
open.**

### 2.4 The GStreamer regression is gone, with the binary as the witness

```
$ file -b /usr/libexec/gstreamer-1.0/gst-plugin-scanner
ELF 64-bit LSB pie executable, x86-64 … BuildID[sha1]=af21c6a3ef0db9e767c7c4d8dc22ec8a6e281db6
$ rm -rf ~/.cache/gstreamer-1.0; gst-inspect-1.0 | tail -1
Total count: 240 plugins, 1344 features
```

240 plugins and 1344 features with the image's own scanner in place — the same
number round 31 could only reach by pointing `GST_PLUGIN_SCANNER` at the
pristine deployment by hand. Every other binary the old extension shadowed is
64-bit again, checked one at a time:

```
/usr/bin/fc-list                     ELF 64-bit LSB pie executable, x86-64
/usr/libexec/at-spi-bus-launcher     ELF 64-bit LSB pie executable, x86-64
/usr/libexec/dconf-service           ELF 64-bit LSB pie executable, x86-64
/usr/libexec/p11-kit/p11-kit-remote  ELF 64-bit LSB pie executable, x86-64
/usr/libexec/gio-launch-desktop      ELF 64-bit LSB pie executable, x86-64
```

The greeter's accessibility bus (§3.4 of round 31) therefore stops being a
32-bit binary on the next greeter start, without anyone touching it.

### 2.5 What moved in the extension, and what that does and does not prove

56 paths are carried that were not before and 208 are gone. **That comparison
is contaminated by the repository drift of §2.1** — different EVRs mean
different `.build-id` paths and different library sonames — so it is recorded,
not leaned on. What is clean is the intersection with the image, because both
sides of it were measured on their own machine at their own moment: 177 → 0.
The gone-208 are dominated by `/usr/i686-w64-mingw32/sys-root/mingw/share/locale/*`
`.mo` files, i.e. cross-compiler locale data the image already owns, which is
precisely what the new rule is for.

**Verdict: pkg-share PASSES on hardware.** The three confirmations the closure
asked for are 13 (was 0), 0 (was 177), and an image-owned count that dominates
the native-pass count 2151 to 152.


## 3. gaming-gpu — docs/gaming-and-sessions.md §6

Run on the rebased machine (`apex-7f647470…`, digest `sha256:be3bdd0c…`, version
`apex (2026-09-19T10:09:10Z)`, up since 18:29:42, **no reboot between §2 and
this section**). Preconditions re-read immediately before arming, so every
number below is against a machine that already has pkg-share's output merged:

```
$ ls /usr/share/vulkan/icd.d/ | grep -c i686          -> 13
$ sudo sha256sum /var/lib/extensions/apex-user.raw
eb3b8ba056d8f6f3caca2c3ed3204bda077f5f5f2fa426ccf264e385622c28a6   (§0.1's was 99749240…)
$ systemd-sysext status        -> /usr  apex-user  Sat 2026-09-19 18:31:25 AWST
$ wc -l ~/.local/share/Steam/logs/console-linux.txt    -> 37868   (the marker)
```

### 3.0 The harness, and the two sessions this section reads

Both Gaming Mode runs went through greetd's `initial_session`, the route proven
in §0.6 and argued for in §0.4 — same PAM service, same `acct_mgmt` →
`setcred` → `open_session`, same `setresuid`, only the auth conversation
skipped, and `pam_cap` still absent on this image (§1.1). The Exec is wrapped in
`systemd-cat -t <tag>` because greetd dup2s the VT onto the session's stdio;
`systemd-cat` **execs** its target, so the process chain, uid and capability
sets are untouched. That redirection is the one divergence, stated again here
because everything below is read out of the journal.

Two runs exist and they are kept apart by tag:

| tag | armed | ended | lines |
|---|---|---|---|
| `qual-gaming-new` | 18:34:01 | 20:45:01, by the dead-man restore timer | 144 187 |
| `qual-gaming-r2` | 22:00:40 | 22:03:49, by a deliberate `SIGTERM` to gamescope | 3 789 |

The first is the **long** run — the session was up 2 h 11 m and Steam was still
working 2 h 06 m in — and it is what
answers "does this survive". The second is the **instrumented** run: the machine
was measured live while it was up, which the first could not be. They agree line
for line on everything §6 asks.

### 3.1 §6.1 — Gaming Mode opens the card the monitor is on. It does.

The session's own log, `qual-gaming-r2`, in full (the `[apex-gaming-session]`
lines; identical in `qual-gaming-new`):

```
[apex-gaming-session] apexd game mode engaged (cpuset/IRQ/clocks/tier/sched-ext)
[apex-gaming-session] capabilities: CapEff=0000000000000000 CapPrm=0000000000000000 CapAmb=0000000000000000
[apex-gaming-session] CAP_SYS_NICE: absent; soft RLIMIT_RTPRIO is 20.
[apex-gaming-session]   NOT passing --rt: gamescope needs the capability, not the rlimit, and would
[apex-gaming-session]   log 'No CAP_SYS_NICE' and run at ordinary priority anyway. Frame pacing under
[apex-gaming-session]   load is worse; nothing else is affected. See evidence §6.2.
[apex-gaming-session] gpu-select: HDMI-A-1 on card2 is an external display; 2 connected output(s) across 2 card(s)
[apex-gaming-session] gpu-select: no connector on this machine publishes vrr_capable at all, so this is 'the driver does not say' and NOT 'the display has no VRR'. On NVIDIA that is expected; the property has to come from somewhere else
[apex-gaming-session] GPU/output: --prefer-vk-device 10de:249d --prefer-output HDMI-A-1
[apex-gaming-session] starting: gamescope -e -f --expose-wayland --prefer-vk-device 10de:249d --prefer-output HDMI-A-1 --mangoapp -- steam -gamepadui
```

and gamescope's answer:

```
[gamescope] vulkan: selecting physical device 'NVIDIA GeForce RTX 3070 Laptop GPU': queue family 2 (general queue family 0)
[gamescope] drm: opening DRM node '/dev/dri/card2'
[gamescope] drm: Connector HDMI-A-1 -> LEN - R25f-30
[gamescope] drm: Connectors:
[gamescope] drm:   HDMI-A-1 (connected)
[gamescope] drm: selecting connector HDMI-A-1
[gamescope] drm: selecting mode 1920x1080@240Hz
```

**All five lines the run-book predicted, verbatim, and the prediction was
written down before the run** (card `## NEXT`, §0.10, §1.1). `drm: Connectors:`
lists exactly one entry — gamescope on card2 never even enumerates the panel.
The whole 3 789-line log contains **0** occurrences of `card1` or `eDP-1`.

Render proof, measured live at 22:03 while the session was up. `grim` and
`wlr-randr` do not work under gamescope (§0, FOUND), so the proof is who holds
the node and who is on the GPU:

```
=== drm clients (who holds a node) ===
Xwayland        /dev/dri/card2
gamescope-wl    /dev/dri/card2       <- and NOT card1
gamescope-wl    /dev/dri/renderD128
gamescope-wl    /dev/dri/renderD129
steam           /dev/dri/renderD129
steamwebhelper  /dev/dri/renderD128
steamwebhelper  /dev/dri/renderD129
fossilize_repla /dev/dri/card1       <- shader compiler only, no KMS
```

```
$ nvidia-smi
|    0   N/A  N/A          247577    C+G   gamescope                                82MiB |
|    0   N/A  N/A          247612      G   Xwayland                                  3MiB |
|    0   N/A  N/A          247741      G   ...share/Steam/ubuntu12_32/steam          6MiB |
|    0   N/A  N/A          248108      G   ./steamwebhelper                         20MiB |
|    0   N/A  N/A          248140      G   ...am/ubuntu12_64/steamwebhelper        265MiB |
|    0   N/A  N/A          248387    C+G   .../ubuntu12_64/fossilize_replay         35MiB |   (x5)
```

The **whole** Steam stack — compositor, Xwayland, client and the CEF UI process
holding 265 MiB — is on the RTX 3070 as a graphics context, and the KMS master
for the monitor is gamescope on card2. Session 1041: `Seat=seat0 TTY=tty1
Type=wayland Class=user Active=yes State=active`.

As §0 warned, `card1-eDP-1` is **not** asserted dark: it read
`enabled=enabled dpms=On` throughout, because nobody holds it once the greeter
is gone and the panel keeps its last framebuffer. That is why the proof above
is fd ownership and nvidia-smi rather than a dpms reading.

**§6.1 PASSES.**

### 3.2 §6.2 — realtime, in both directions

```
in-session, from the session's own log:
  capabilities: CapEff=0000000000000000 CapPrm=0000000000000000 CapAmb=0000000000000000

live /proc of the two session processes (22:03):
  gamescope           pid 247622   CapInh 0000000800000000  CapPrm 0  CapEff 0  CapAmb 0  CapBnd 000001ffffffffff
  apex-gaming-session pid 247449   CapInh 0000000800000000  CapPrm 0  CapEff 0  CapAmb 0  CapBnd 000001ffffffffff
  steam               pid 252927   CapInh 0                 CapPrm 0  CapEff 0  CapAmb 0

$ getcap "$(command -v gamescope)"     -> (empty), rc 0
$ apex gaming | grep -E 'realtime (limit|capability)'
realtime limit     : yes
realtime capability: no (/proc/self/status)
```

`CapInh=0000000800000000` is `cap_wake_alarm` alone, inherited from greetd's own
ambient set and demoted to inheritable on the uid change — exactly what §0.6
measured on the old image. Nothing on this machine grants CAP_SYS_NICE, and
nothing pretends to.

**`--rt` is not passed.** The only occurrence of the string `--rt` in the entire
3 789-line session log is the session script's own prose line
`NOT passing --rt: …`; the `starting: gamescope …` line quoted in §3.1 does not
carry it. Limit yes / capability no, as the run-book asked.

*Reading that disagrees with a naive expectation, recorded because the reading
wins:* gamescope still prints

```
No CAP_SYS_NICE, falling back to regular-priority compute and threads.
Performance will be affected.
```

**even though `--rt` was not passed.** So that message is emitted by gamescope's
own startup capability probe and is **not** evidence that `--rt` was requested —
§0.7 saw the same two lines on the old image, where `--rt` *was* passed
unconditionally. The discriminator is the `starting:` line, not gamescope's
warning. Worth keeping in the doc: §6.2's sentence "the session log to say
'CAP_SYS_NICE: absent' and NOT pass `--rt`" is right, but a reader checking it
by grepping for `CAP_SYS_NICE` will find gamescope's line too and could conclude
the opposite.

**§6.2 PASSES.**

### 3.3 §6.3 — Steam inside gamescope. It reaches a UI, for the first time.

This is the row the unit exists for, and it is the first run in which it *could*
pass: §0.7 attributed the `bwrap` half to the round-31 harness, and §2 put 13
i686 ICDs on `/usr` for the Vulkan half. Both halves were retested together.

Criterion stated before the run, because the doc does not define one: no
`BInit`/`VK_KHR_surface` failure after the marker, `steamwebhelper` alive, and
the Steam processes on the NVIDIA card as `G` contexts.

**The Vulkan half — the defect is gone.**

```
$ grep -cE 'BInit|VK_KHR_surface|Unexpected capabilities' <lines 37869..EOF>
0
$ grep -n 'BInit - Unable' console-linux.txt | tail -1
37590:[2026-09-19 17:31:46] BInit - Unable to initialize Vulkan!     <- the LAST one ever, on the old image
```

Every previous attempt on this machine ended at that line: 2026-09-06 16:59:26,
2026-09-17 17:58:58, 2026-09-19 09:25:29, 2026-09-19 17:31:46. After the
pkg-share extension landed there are **none**, in either run.

**The bwrap half — nothing to attribute any more.**

```
[2026-09-19 22:00:41] steam.sh[247623]: Steam client's requirements are satisfied
```

Zero `bwrap: Unexpected capabilities but not setuid` lines in either run, with
`CapPrm=0` from greetd. §0.7's verdict holds on the new image.

**What actually came up.**

```
[2026-09-19 22:00:41] Startup - Steam Client launched with: '…/ubuntu12_32/steam' '-srt-logger-opened' '-gamepadui'
[2026-09-19 22:00:42] Verifying installation... / Verification complete
[2026-09-19 22:00:43] Running query: 1 - GpuTopology
                        gpus { id: 2  name: "NVIDIA GeForce RTX 3070 Laptop GPU"
                               driver_id: k_EGpuDriverId_NvidiaProprietary  580.178.4 }
[2026-09-19 22:00:44] Steam Runtime Launch Service: steam-runtime-launcher-service is running pid 248073
[2026-09-19 22:00:44] bus_name=com.steampowered.PressureVessel.LaunchAlongsideSteam
[2026-09-19 22:00:45] [Gamescope WSI] Executable name: vulkandriverquery
[2026-09-19 22:00:45] [Gamescope WSI] Forcing on VK_EXT_swapchain_maintenance1.
```

and live at 22:03, the UI process itself:

```
248108 ./steamwebhelper … -uimode=4 … -clientui=…/clientui …
248140 …/ubuntu12_64/steamwebhelper                          265MiB on the RTX 3070 (G)
```

`-uimode=4` is Big Picture. gamescope's X side logged
`xwm: Embedded, no cursor set` and then
`xwm: got the same buffer committed twice, ignoring` — an X client mapping and
committing buffers, i.e. something is being composited, not an empty root.

**The long run is the durability half.** `qual-gaming-new` held the same stack
up from 18:34:02 to 20:45:00 — **2 h 11 m** — with Steam's background update loop
completing normally at 18:36:07 (`HTTP 304 Not Modified`, `Nothing to do`) and
Fossilize shader-cache replay ticking on the dGPU right through to 20:40:32.
That last Fossilize line is 2 h 06 m after Steam started and 4 m 28 s before
the teardown, so the client was still working when the run was cut. It ended
only because this unit's own dead-man timer restarted greetd.

**§6.3 PASSES. The row can be closed.** For the record, `bwrap` still takes
SIGSYS core dumps during pressure-vessel's seccomp probing (16 this boot, and
the same pattern exists in this log back to 2026-07-30) — pre-existing, benign,
and *not* the `Unexpected capabilities` failure; the requirements check passes.

### 3.4 §6.6 — cleanup, and a defect the run-book's own harness exposed

**Clean exit (`qual-gaming-r2`).** `SIGTERM` to gamescope — the documented
end-of-session signal, "gamescope exiting is the signal that Gaming Mode is
over":

```
[gamescope] launch: Primary child shut down!
/usr/libexec/apex-gaming-session: line 312: 247577 Segmentation fault (core dumped) gamescope … -- steam …
[apex-gaming-session] gamescope exited with 139
[apex-gaming-session] apexd game mode released          <- the EXIT trap ran

$ apex game status        -> active : false
$ pgrep -c -f '^gamescope' -> 0        $ pgrep -c -x steam -> 0
$ loginctl                -> greeter back on tty1 by itself, ActiveSession=c5
```

**Killed exit (`qual-gaming-new`).** When the 20:45 dead-man timer restarted
greetd instead, the session script logged `gamescope exited with 139` and then
**nothing**. `apexd game mode released` never appeared, and Gaming Mode was
still engaged 75 minutes later when this agent picked the machine up:

```
$ apex game status
active : true   tier : performance   prior_tier : performance
cpuset_policy: p-cores   irq_policy: away-from-game   sched-ext: scx_lavd …
$ sudo journalctl -b -t qual-gaming-new -o cat | grep -c 'apexd game mode released'
0
```

**DEFECT (new) — Gaming Mode cannot release itself when its session is torn
down from outside.** `cleanup()` calls `apex game stop`, which goes through
polkit action `org.apexos.apexd.manage-power`, whose defaults are
`allow_active=yes`, `allow_inactive=auth_admin`, `allow_any=auth_admin`
(`/usr/share/polkit-1/actions/org.apexos.apexd.policy`). The moment logind
stops treating the session as *active* — which is precisely what a greetd
restart, a VT switch away, or a logind-driven teardown does — the trap's own
call is refused. Measured directly, from a session with no seat:

```
$ apex game stop
apex: leaving game mode failed: org.freedesktop.DBus.Error.AccessDenied:
      not authorized for org.apexos.apexd.manage-power
$ sudo apex game stop
apex: game mode OFF                       <- root is not subject to the check
```

The machine is then left with a p-core cpuset, IRQ steering, the `performance`
tier and `scx_lavd` still installed, with no session that can undo it and no
prompt anyone will ever see. Two candidate remedies, neither implemented here
(this unit owns qualification, not the fix): let `apexd` release game mode when
the session that requested it goes away (it already knows the cgroup), or give
the release path a `allow_inactive=yes` of its own separate from *entering* game
mode. Filed as a follow-up below.

Two second-order readings from the same teardown, recorded because they are
real and neither is caused by this image:

* **gamescope segfaults on exit, every time.** `SIGSEGV` at 20:45:01 and
  22:03:48 on the new image, and at 09:21:26 on the old one; `SIGABRT` on
  2026-08-23. The session script reports it faithfully (`exited with 139`) and
  the trap still runs, so nothing downstream breaks.
* **`mangoapp` crash-loops for the entire session.** `gamescopereaper --respawn
  -- mangoapp` respawns it at roughly 2 Hz: **15 376 mangoapp core dumps this
  boot**, 14 403 respawns in the 2-hour run (57 612 `Glfw Error 65537/65550:
  X11: Platform not initialized` lines, 364 respawns in the 3-minute run). The
  MangoHud overlay never appears, the journal takes 144 187 lines for one
  gaming session, and `coredumpctl` is 15 000 entries deep. `--mangoapp` is
  added unconditionally by the session script whenever `mangoapp` is on PATH.
  Filed as a follow-up below.

**§6.6 PASSES for the clean path** and fails for the torn-down path, which is
the defect above.

### 3.5 §6.4 — Safe Graphics on the dGPU output

**The control flip.** §0.8 measured the old image: four lines, no GPU or output
rows, and `grep -c WLR_DRM_DEVICES /usr/libexec/apex-safe-graphics` → 0. The new
image, same command over ssh:

```
$ /usr/libexec/apex-safe-graphics check                              rc=0
config      /usr/share/apex/safe-graphics
compositor  /usr/bin/labwc
terminal    foot
renderer    pixman (software)
primary gpu card1
can light   eDP-1
cannot light HDMI-A-1(card2) — on a secondary GPU; the software
            renderer cannot import DMA-BUFs to reach them
            force one with APEX_SAFE_GRAPHICS_DRM_DEVICE=/dev/dri/<card>

$ grep -c WLR_DRM_DEVICES /usr/libexec/apex-safe-graphics              -> 6
$ grep -c APEX_SAFE_GRAPHICS_DRM_DEVICE /usr/libexec/apex-safe-graphics -> 7
```

Byte-for-byte the three rows the run-book predicts, including naming the escape
hatch in the same breath as the limitation.

**The forced-device run**, through the same greetd mechanism
(`env APEX_SAFE_GRAPHICS_DRM_DEVICE=/dev/dri/card2 /usr/libexec/apex-safe-graphics`,
tag `qual-safegfx`, 22:04:36):

```
[apex-safe-graphics] primary gpu: card1; connected on it: eDP-1
[apex-safe-graphics] WLR_DRM_DEVICES=/dev/dri/card2 (forced by APEX_SAFE_GRAPHICS_DRM_DEVICE)
```

```
$ grep -c 'did not support importing DMA-BUF' <the log>   -> 0
$ pgrep -a labwc  -> 262292 labwc -C /usr/share/apex/safe-graphics
$ pgrep -a foot   -> 262369 foot
drm holders: labwc /dev/dri/card2      (card1 held by nobody but systemd/logind)
```

**Render proof — the monitor is the screen that lit.** `grim` works here because
labwc is wlroots:

```
$ wlr-randr
HDMI-A-1 "Lenovo Group Limited R25f-30 URW0PNMC (HDMI-A-1)"
  Enabled: yes   1920x1080 px, 239.964005 Hz (preferred, current)
$ wlr-randr --json | names   -> ['HDMI-A-1']            <- the ONLY output

$ python3 ~/qual/shot.py safegfx-card2 HDMI-A-1 eDP-1
  HDMI-A-1     1920x1080  distinct_colours=699  mean_rgb=(7.8, 7.8, 7.8)
  eDP-1        grim FAILED rc=1 unknown output 'eDP-1'
```

The compositor does not merely prefer the monitor — with `WLR_DRM_DEVICES`
forced to card2 the panel **does not exist in this session at all**, and the
frame that came back off HDMI-A-1 has 699 distinct colours (foot on labwc's
black root), not a blank scanout.

**Stated rather than claimed: the automatic branch was NOT reached.** It needs
every connected screen to be on a non-primary card, i.e. the panel genuinely
dark — lid shut and docked, or the panel disabled in firmware. `card1-eDP-1`
read `status=connected enabled=enabled` throughout, so this run could only
exercise the override. What the override proves is the *mechanism* the automatic
branch would use; what it does not prove is the detection that chooses it.

**§6.4 PASSES for the forced path; the automatic path remains untested.**

### 3.6 §6.5 — the niri bar

**The precondition was consumed before the login, and by design.** §0.8 recorded
that at 17:36 on the old image `config.kdl` line 271 was untouched and there was
no `.pre-apex-bar.bak`. On the new image the transform had **already happened at
18:29:35**, five minutes after the reboot and four hours before any niri
session:

```
Sep 19 18:29:35 apex apex-shell-firstrun[1588]:
  [apex-shell-firstrun] disabled niri's stock waybar spawn (line 271) -> ~/.config/niri/config.kdl
```

`apex-shell-firstrun.service` is a **user** unit and runs when `user@1000.service`
starts, which on this machine is at boot — not at a graphical login. So the doc's
"after one login to the niri session" understates it: the transform lands on any
machine whose user manager starts, session or no session. Recorded as a
divergence: the niri login below **confirms** the end state, it did not trigger
it.

The niri session was launched through the same harness (tag `qual-niri`,
22:05:48, `niri --session`, "starting version 26.04") and read live:

```
$ grep -n waybar ~/.config/niri/config.kdl
270:// This line starts waybar, a commonly used bar for Wayland compositors.      <- niri's own upstream comment
271:// spawn-at-startup "waybar"   // disabled by APEX: quickshell is this system's bar

$ pgrep -a -u andre waybar      -> (none)                      was 31621 on the old image
$ pgrep -a -u andre quickshell  -> 267286 quickshell -c /usr/share/apex-shell   (exactly one)
$ ls ~/.config/niri/config.kdl.pre-apex-bar.bak   -> present, 28144 B
$ niri validate --config ~/.config/niri/config.kdl
INFO niri: config is valid
```

Exactly one `spawn-at-startup "waybar"` line, commented, ending in the APEX
marker; the other match is niri's own upstream comment above it.

Free render reading while it was up, which also separates niri from Gaming Mode:

```
$ python3 ~/qual/shot.py niri-new HDMI-A-1 eDP-1
  HDMI-A-1     1920x1080  distinct_colours=164551  mean_rgb=(133.7, 142.3, 153.7)
  eDP-1        1920x1080  distinct_colours=164551  mean_rgb=(133.7, 142.3, 153.7)
drm holders: niri /dev/dri/card1  AND  niri /dev/dri/card2
```

niri drives **both** cards and paints both screens; Gaming Mode deliberately
takes only card2. Two different correct behaviours, and §6.1's reading is not an
artefact of the machine only being able to light one screen.

**§6.5 PASSES** — two bars became one, and it was one before the login.

### 3.7 Follow-up defects this section found

1. **Gaming Mode stays engaged when its session is torn down from outside.**
   `apex game stop` is `allow_active=yes` / `auth_admin` otherwise, so the EXIT
   trap's release is refused the moment logind deactivates the session. Measured
   twice: the trap succeeded on a clean gamescope exit and never ran to
   completion on a greetd restart, leaving p-core cpuset + IRQ steering +
   `performance` + `scx_lavd` installed with nothing able to undo it.
2. **`mangoapp` crash-loops at ~2 Hz for the whole of every Gaming Mode
   session** — 15 376 core dumps this boot, 14 403 respawns in one 2-hour
   session, 144 187 journal lines. The overlay never renders.
   `--mangoapp` is passed unconditionally whenever `mangoapp` is on PATH.
3. **gamescope segfaults on every exit** (`139`). Cosmetic today — the trap
   still runs on the clean path — but it is a core dump per session and it
   predates this image.
4. **§6.2's check is ambiguous as written.** gamescope prints
   `No CAP_SYS_NICE, falling back to regular-priority` whether or not `--rt` was
   passed, so grepping for `CAP_SYS_NICE` cannot distinguish the two. The
   `starting: gamescope …` line is the only discriminator.
5. **§6.5's precondition is stated too narrowly** — the transform runs from
   `apex-shell-firstrun.service` at user-manager start, not at niri login.

### 3.8 Verdict, and the state the machine was left in

**gaming-gpu PASSES on hardware.** §6.1, §6.2, §6.3, §6.4 (forced path) and
§6.5 all read what the run-book predicted, on the monitor, on the RTX 3070,
through a real greetd login. §6.3 — the row this unit existed for — reached a
Steam Big Picture UI inside gamescope on the dGPU for the first time on this
machine, and stayed up for two hours. Two paths remain untested and are named
rather than claimed: Safe Graphics' **automatic** dGPU branch (needs the panel
genuinely dark) and any actual **game** launch (none was started).

greetd, left explicitly:

```
$ sudo /var/tmp/apex-work/scratch-katana-image-qual/greetd-restore.sh
restored: byte-identical to backup
greetd restarted
$ sudo cmp /etc/greetd/config.toml /etc/greetd/config.toml.orig-qual2   -> identical
$ sudo grep -c initial_session /etc/greetd/config.toml                  -> 0
$ systemctl is-active greetd                                            -> active
$ systemctl list-timers qual-greetd-restore.timer                       -> 0 timers listed
$ loginctl show-seat seat0 -p ActiveSession                             -> ActiveSession=c5
$ pgrep -a -u greetd sway
272321 sway --unsupported-gpu -c /usr/share/apex-greet/sway-greet.conf
$ apex game status -> active : false ;  gamescope/steam/niri/labwc: none running
```

The greeter is back on tty1 and Andre can log in normally. One cosmetic
leftover: logind still lists closed greeter sessions `c1`–`c4` in `State=closing`
from the repeated greetd restarts; `c5` is the live one and is the active
session on seat0. They clear on the next boot and nothing holds a device.

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

*(pending)*

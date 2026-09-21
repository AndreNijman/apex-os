# katana, final image: the rows only a booted machine can answer

Measured 2026-09-22 on katana, booted on
`ghcr.io/andrenijman/apex-os:apex-44c9a5cb6ba07b5892ee3b1e34ec77403e7b446a`
(digest `sha256:06ba23c3a9d666780b01bb8774a56693cf66e68e49d35232a52223e756596112`),
kernel `7.2.6-cachyos1.apex1.fc43.x86_64` — APEX's own kernel tier. Nothing was
installed, no reboot, no `bootc` call, no NVRAM access. Raw logs are on katana
in `/var/lab/scratch/katana-final-qual/`.

`could-not-run` below is a verdict, not a gap in the work: every one names the
thing that could not be reached.

---

## 1. P1-043 — Gaming Mode loads a sched-ext scheduler THROUGH apexd

`ROADMAP/evidence/katana-schedext-fixed-20260922.md` already proved the kernel
half by running `scx_rustland` **by hand**. What had never run anywhere is
`apexd`'s own path: the `gaming-scx` landing made `syswriter.rs` read the verb
off `/sys/kernel/sched_ext/state` instead of hardcoding `switch`, and until this
machine existed that code could not reach its success path on any hardware in
this program.

Run-book rows are `docs/gaming-and-sessions.md` §6.8. Each row below was run
with a `systemd-run --on-active` dead-man armed first (`kfq-deadman`, which runs
`apex game stop; scxctl stop`), and disarmed and asserted gone afterwards.

### Row 0 — can this kernel take a scheduler?

| | |
|---|---|
| `apex game status \| grep '^scx_btf'` | **`ok`** |
| verdict | **PASS**. Rows A and C are runnable as written, for the first time. |

Every APEX image before this one answered `implicit-args`.

### Row A — a scheduler actually attaches (`start` branch)

`rowAB.log`, 03:27:47–03:28:14 AWST. Nothing attached beforehand.

| reading | before | during | after `stop` |
|---|---|---|---|
| `/sys/kernel/sched_ext/state` | `disabled` | **`enabled`** | `disabled` |
| `enable_seq` | 1 | **2** | 2 |
| `switch_all` | 0 | **1** | 0 |
| `nr_rejected` | 0 | 0 | **0** |
| `root/ops` | kobject absent | **`lavd_1.1.3_x86_64_unknown_linux_gnu`** | absent |
| `/sys/fs/cgroup/apex-game/cpuset.cpus` | cgroup absent | **`0-11`** | cgroup absent |
| `apex game status` → `active` | `false` | **`true`** | `false` |
| `apex game status` → `scx_state` | `not loaded` | **`loaded`** | `not loaded` |

`sudo journalctl -u apexd --since T0 | grep -c 'no scx scheduler running'` →
**0**. The verb was chosen from the kernel, which is the whole of the §5c fix,
and the refusal that appeared on every boot of three earlier images is gone.

Still `enabled` at t+5 s and t+20 s. `owner_pid : 0` throughout — `sudo apex
game start` with no `--owner-pid` arms no owner watch, so nothing released the
session when the invoking shell exited.

**Row A: PASS.** First time in this program that Gaming Mode has loaded a
sched-ext scheduler.

### Row B — it goes away again

`sudo apex game stop` → `state` `disabled`, `nr_rejected` **0**, the `root/`
kobject gone, the `apex-game` cgroup gone, `scx_state : not loaded`, and the
journal line the run-book asks for:

```
apexd: game: sched-ext after exit: sched_ext/state is disabled — nothing is attached
```

**Row B: PASS.**

### Row A finding — `root/ops` is NOT `lavd`

`gaming-scx-20260920.md` §5 predicted the kernel publishes the bare struct_ops
name (`lavd`, `rusty`), said so was an *expectation* rather than a measurement,
and asked §6.8 Row A to record the string verbatim. Recorded, from three
different schedulers on this machine:

| scheduler | `/sys/kernel/sched_ext/root/ops` |
|---|---|
| `scx_lavd` | `lavd_1.1.3_x86_64_unknown_linux_gnu` |
| `scx_rusty` | `rusty_1.1.3_x86_64_unknown_linux_gnu` |
| `scx_rustland` | `rustland_1.1.3_x86_64_unknown_linux_gnu` |

The name carries the scx build ID. `scx_ops_matches()` strips only a leading
`scx_` and then compares for equality, so **every successful load on this
machine is reported as the wrong scheduler**:

```
apexd: scx: asked for scx_lavd, kernel reports root/ops
       'lavd_1.1.3_x86_64_unknown_linux_gnu' — attached, but not the scheduler
       that was asked for
```

and the same clause lands in `scx_detail`, in `notes`, and in the `game mode ON`
journal line. The expectation was honestly labelled as one and it was wrong;
this is the measurement it asked for.

### Row C — the `switch` branch, which had never executed anywhere

`rowC.log` and `rowC1-timing.log`. Two sub-rows, because there are two ways for
a scheduler to already be attached and they are not the same case.

#### C1 — attached through `scx_loader` (`sudo scxctl start -s scx_rusty`)

| | |
|---|---|
| after `scxctl start` | `state=enabled`, `enable_seq` 2→3, `root/ops` `rusty_…` |
| `sudo apex game start` | rc 0, `apex: game mode ON` |
| 'no scx scheduler running' in apexd since | **0** — the verb WAS `switch` |
| "use 'switch' instead of 'start'" since | 0 |
| kernel 2 s later | `state=enabled`, `enable_seq` **4**, `root/ops` **`lavd_…`** |
| `apex game status` → `scx_state` | **`not loaded`** |
| `apex game status` → `scx_detail` | `…scxctl reported success; sched_ext/state is disabled — nothing is attached` |

**The verb selection passed and the status surface failed.** `scx_lavd` was
attached and running; `apex game status` said nothing was.

#### C1 instrumented — the window, measured at 50 ms

`rowC1-timing.sh` samples `state` and `root/ops` every 50 ms across the call.
`t=0` is immediately before `sudo apex game start`:

```
  -1.001s  state=enabled    ops=rusty_1.1.3_x86_64_unknown_linux_gnu
  +0.129s  state=disabling  ops=rusty_1.1.3_x86_64_unknown_linux_gnu
  +0.183s  state=disabled   ops=-
  +1.547s  state=enabled    ops=lavd_1.1.3_x86_64_unknown_linux_gnu
  (apex game start returned at +0.162s)
```

So a `switch` on this hardware is **~1.42 s of teardown-and-reattach**, and
`scxctl` returns 162 ms in, long before it is over. Two distinct defects fall
out, and both are the shape §5c exists to stop:

1. **`scx_settle_until(|s| Enabled{..})` is satisfied by the scheduler being
   replaced.** At the first poll the old `rusty` is still attached, so the
   predicate is true immediately, `scx_load` returns `Landed`, and the
   "read-back" confirmed a state that was already true *before* the command
   ran. `SCX_SETTLE = 2 s` is ample — it is never spent, because the wrong
   question is being asked. On a `switch` the predicate must be *enabled AND
   `root/ops` is the scheduler that was requested*.
2. **`scx.observed` is a second, later read** (`game.rs:479`), taken after the
   cpuset/IRQ/GPU work. On this timeline that lands in the `disabled` trough at
   +0.18 s…+1.55 s, so the report says `not loaded` / "nothing is attached"
   about a session that has `scx_lavd` running. A false negative, which is the
   mirror image of the false positive §5c removed.

These two interact: fixing (1) alone would still leave (2) free to sample the
trough, and fixing the settle predicate needs `scx_ops_matches` to tolerate the
build-ID suffix first, or the predicate can never become true at all.

**C1 verdict: verb selection PASS; status surface FAIL (defect, new, recorded
above).**

#### C1 exit — the documented limitation, observed

```
apexd: game: sched-ext was already running before this session
       (rusty_1.1.3_x86_64_unknown_linux_gnu) and exit STOPPED it rather than
       putting it back — scheduling is now: sched_ext/state is disabled
```

That is `gaming-scx-20260920.md` §10's named limitation reaching hardware for
the first time, and the line says exactly which case it was. **Not graded a
defect**; it is a limitation with a witness now.

#### C2 — attached by running the binary directly, so the loader disagrees with the kernel

`scx_loader` keeps its own bookkeeping. Running `/usr/bin/scx_rustland` under
`systemd-run` bypasses it, so the kernel says `enabled` and the loader believes
nothing is running. This is the branch where the single named-verb retry is the
only thing that can work, and it had never executed anywhere.

```
apexd: scxctl switch -s scx_lavd failed (exit status: 1): error: no scx
       scheduler running, use 'start' instead of 'switch'
apexd: scxctl asked for 'start' instead of 'switch' — retrying once
```

**The retry fired, exactly once, on the verb the loader named.** `apex game
start` returned 0 and game mode came up.

What the retry could not do is replace the scheduler: `enable_seq` stayed at 5
and `root/ops` stayed `rustland_…` throughout, so `scx_lavd` never attached.
`apex game status` reported `scx_state : loaded` with the mismatch NOTE, which
is `gaming-scx` §5's deliberate choice ("something attached is a different
problem from nothing attached") — correct by design, and unreadable in practice
while the build-ID bug makes that NOTE fire on every correct load too.

On exit, `scxctl stop` could not remove a scheduler the loader never started,
and apexd said so rather than claiming a restore:

```
apexd: game exit: sched-ext stopped (back to the kernel scheduler) refused —
       scxctl stop reported success, but a scheduler is still attached
       (rustland_1.1.3_x86_64_unknown_linux_gnu) after 2s
```

**C2 verdict: retry branch PASS (first execution anywhere). Reported honestly
at every step.**

### Also observed while there, and not defects

- **26 of 73 IRQ affinity writes are refused `EPERM` on katana even as root** —
  managed IRQs, whose affinity the kernel owns. `apex game status` reports
  `irqs_attempted: 73 / irqs_steered: 47 / irqs_refused: 26` and the journal
  names the first one. Honest; recorded so the next reader does not chase it.
- `gpus_locked: 0` and `gpus_lock_attempted: 0` in `apex game status` are the
  GPU **index list** `[0]`, not counts; the journal says `1/1 GPU(s) locked` for
  the same session. It reads like a contradiction and is not.
- The two non-discriminators the card warned about held: `tier` and `prior_tier`
  are both `performance` before, during and after, so the governor proves
  nothing on this machine. `active`, the `apex-game` cgroup, `enable_seq` and
  `root/ops` are the witnesses that moved.

### P1-043 scorecard

| row | verdict |
|---|---|
| Row 0 — `scx_btf : ok` | **pass** |
| Row A — `start` branch attaches, kernel confirms | **pass** |
| Row A — `root/ops` recorded verbatim | **pass, and it refutes the prediction** |
| Row B — detaches, `nr_rejected 0` | **pass** |
| Row C1 — `switch` verb chosen from the kernel | **pass** |
| Row C1 — status reports the switch correctly | **fail** — race, measured at 1.42 s |
| Row C2 — named-verb retry, loader/kernel disagreement | **pass** |
| exit STOPS rather than restores a pre-existing scheduler | **limitation, witnessed** |

---

## 2. P2-005 / P2-006 / P2-007 — the lines only a booted machine answers

`ROADMAP/evidence/P2-005-007-device-maturity.md` closes with Andre's checklist.
Everything below follows it in order. **Graded off what `apex devices` prints**,
because the whole defect that round removed was the tool saying "could not
ask"; the raw `systemctl`/`nmcli` readings are the cross-check, not the
criterion. Log `item2.log` / `item2b.log`, 03:34–03:36 AWST.

### Step 1 — on the new image

`bootc status --json` → the 44c9a5cb image named at the top of this file.
Not re-derived; it is the same reading every section here is taken on.

### Step 2 — the six lines. All six, with the checklist's own expected answers

| line | checklist says | katana printed | verdict |
|---|---|---|---|
| `links` | a count, then one row per interface | `4`, then `wlo1 wifi connected` / `lo loopback connected (externally)` / `p2p-dev-wlo1 wifi-p2p disconnected` / `enp5s0 ethernet unavailable` | **pass** |
| `connectivity` | `full, and checked` | `full, and checked` | **pass** |
| `hotplug (udev)` | `systemd-udevd is running` | `systemd-udevd is running` | **pass** |
| `paired devices` | a count or `none` | `none` | **pass** — see the finding below |
| `this machine serves` | `nothing (no smbd, no nfs-server)` | `nothing (no smbd, no nfs-server)` | **pass** |
| `auto-mount` | `udisks2 running` | `udisks2 running` | **pass** |

None of the six says "could not be asked". On every container run of this tool,
and on the L16 before the update, several of them did.

Cross-check, all agreeing: `apex-firewall active`, `avahi-daemon active` and
`enabled`, `systemd-udevd active`, `udisks2 active`, `bluetooth active`,
`cups.socket active`, `smb`/`smbd`/`nfs-server` all `inactive`,
`nmcli -t -f CONNECTIVITY general` → `full`.

**The `connectivity` line is the one that could have silently failed.** It reads
`full, and checked` rather than "not detected, because nothing checked", and
`21-apex-connectivity.conf` is on disk at
`/usr/etc/NetworkManager/conf.d/21-apex-connectivity.conf` — owned by **no rpm**,
which is correct for a file the Containerfile writes — and its `/etc` merge copy
is **byte-identical** (`cmp`). So the drop-in landed from the image rather than
having been put there by hand.

#### Finding: the `paired devices` count in the card's own starting sweep was wrong

`/var/lab-scratch/katana-measure/after.txt` reported `paired devices : 2`, taken
as `bluetoothctl devices | wc -l`. On katana:

```
bluetoothctl devices          2   (9C:44:3D:5D:EE:4D, 14:5A:FC:5C:1D:A2 "50PUT7605/79")
bluetoothctl devices Paired   0
bluetoothctl devices Bonded   0
per-device from the bus:      Paired=no Bonded=no Trusted=yes   (both)
```

`bluetoothctl devices` lists every device the adapter *knows*, paired or not.
`apex-devices` asks `bluetoothctl devices Paired`, which is the right question,
and `none` is the right answer. **The tool is correct and the sweep was not** —
recorded because the sweep is what a reader would have quoted.

### Step 3 — the line that is a safety claim

`hotspot / tethering` → **`nothing known is in the way`**, and the forbidden
reading `not known — the firewall could not be asked` does **not** appear.
`sudo apex firewall status` answers on this machine and says
`not sharing this machine's connection on any link`, so the firewall genuinely
was asked. **pass.**

### Step 4 — avahi, printing and scanning

avahi is `active` and `enabled` on katana; it was never masked here, so the
unmask has nothing to do. `mDNS (avahi)` → `running`. **pass.**

Two driverless printers are on this LAN, both found over mDNS:

| | |
|---|---|
| `Canon TR4600 series` | `cA4D05300000.local` → `192.168.1.121:631`, `rp=ipp/print`, `Scan=T` |
| `FUJIFILM ApeosPrint C325/328 dw` | `FF-1C7D224E44ED.local` → `192.168.1.213:631`, `rp=ipp/print` |

`apex devices print` → `queues none configured` with the hint that names mDNS.
Correct: no CUPS queue has been added, and adding one changes the machine.

`apex devices scan` → `1 found`,
`airscan:e0:Canon TR4600 series is a eSCL Canon TR4600 series ip=192.168.1.121`,
which `scanimage -L` independently repeats. **pass.**

#### The IPP path proved end to end, without printing anything

The P2-005 evidence's own limit was "no print job has been sent". A job needs
paper and a person. `ipptool` proves the protocol path instead — the machine
opens 631 outbound through the shipped firewall, speaks IPP and gets a real
response:

```
ipptool -t ipp://192.168.1.121/ipp/print get-printer-attributes.test   [PASS]
ipptool -t ipp://192.168.1.213/ipp/print get-printer-attributes.test   [PASS]
```

and the attributes are the printers' own, not a stub:

| | Canon | FUJIFILM |
|---|---|---|
| `printer-make-and-model` | `Canon TR4600 series` | `FUJIFILM ApeosPrint C325/328 dw` |
| `printer-state` | `idle` | `idle` |
| `printer-state-reasons` | `none` | **`toner-low-warning`** |
| `ipp-versions-supported` | `1.1,2.0` | `2.0,1.1,1.0` |

The FUJIFILM reporting its own toner level is the part a fixture could not
fake. **Two vendors, one image, no paper.** P2-005 printing moves from
"discovery reaches the machine" to "the IPP conversation completes"; sending a
job is still not done and still needs a person.

### Step 5 — the hotspot, end to end: **COULD-NOT-RUN**

Two independent reasons, both named rather than worked around:

1. **katana has exactly one usable link.** `nmcli` shows `wlo1` (wifi,
   connected) and `enp5s0` (ethernet, **unavailable** — no cable).
   `nmcli device wifi hotspot ifname wlo1` tears down the station connection
   this session arrives over, so it severs ssh and leaves the machine off the
   network with nobody at it. The card forbids exactly this class of step.
2. The acceptance criterion needs **a phone to join the hotspot and resolve a
   name**, which no agent has.

What *can* be said without running it is already above: the firewall answered,
`apex firewall status` reports `not sharing this machine's connection on any
link`, and `hotspot / tethering` names no blocker.

### Step 6 — the 802.1X exposure: **does not exist on katana**

`enterprise Wi-Fi (802.1X)` → **`no saved profile uses it`**, and
`system CA trust store` → `readable`. The exposure on record is the **L16's**
two saved profiles, and the L16 is Andre's daily machine. Nothing was modified.
**not applicable here; the L16 row stays open.**

### Step 7 — sharing a printer

`sharing a printer` → `off`; `this machine serves` → `nothing (no smbd, no
nfs-server)`; `apex firewall status` → `exceptions you have added: (none)`.
Consistent across the firewall, the tool and systemd. Starting a share changes
the machine, so the `THE FIREWALL IS WHY` branch was not provoked.
**pass on the reading; the failure branch stays fixture-proved.**

### What `apex devices all` answered that the checklist did not ask about

Worth recording because each is a real reading from a real machine, and several
had never been taken outside a container:

- `SD card slot` → `none — this kernel registered no MMC host`, with the note
  that a USB card reader is not one of these.
- `this session's seat` → `none`, with the explanation that udisks2 refuses to
  mount for a seatless session. That is *this ssh session* being described
  correctly, which is the tool distinguishing "absent" from "refused".
- `camera (PTP) tooling` → `no — gphoto2 is absent`. A genuine gap in the image,
  stated as one.
- `Thunderbolt` → `no controller on this machine`; `USB-C ports` → `the kernel
  exposes no Type-C class`, with the caveat that the ports may still work.
- `headset codecs` → `aac aptx faststream g722 lc3 ldac opus-g opus sbc`.
- `SMB/WebDAV/NFS/MTP/cameras (file manager)` all `yes`; `mount.cifs` and
  `mount.nfs` both present. These are the packages the P2-005 round added,
  present on a booted image for the first time.

### P2-005/006/007 scorecard

| item | criterion | verdict |
|---|---|---|
| P2-005 | printing — discovery | **pass** (two printers, mDNS, `_ipp`) |
| P2-005 | printing — IPP path end to end | **pass** (`ipptool`, both vendors, no paper) |
| P2-005 | printing — a job on paper | **could-not-run** — needs a person and paper |
| P2-005 | scanning | **pass** (eSCL Canon TR4600, two independent tools) |
| P2-005 | SMB/NFS/WebDAV/MTP/cameras floor | **pass** (all present on the booted image) |
| P2-005 | a share actually mounted from a server | **could-not-run** — no server credentials |
| P2-006 | `links`, `connectivity` | **pass** |
| P2-006 | hotspot end to end | **could-not-run** — one link; would sever ssh; needs a phone |
| P2-006 | 802.1X | **not applicable on katana** — no saved enterprise profile |
| P2-007 | `hotplug (udev)`, `auto-mount` | **pass** |
| P2-007 | Thunderbolt / USB-C / dock | **could-not-run** — no controller, no Type-C class on this hardware |
| P2-007 | bluetooth adapter, radio, codecs, paired | **pass** |

---

## 3. P2-003 — the shell publishes a real accessibility tree, and nothing turns it on

**The card's prediction was right, and the route to measuring it was not the one
the card described.** No greetd teardown was needed and none was done: the
greeter *is* a live quickshell session.

### What was already running, before anything was touched

```
greetd  1518  sway --unsupported-gpu -c /usr/share/apex-greet/sway-greet.conf
greetd  1600  /usr/libexec/at-spi-bus-launcher
greetd  1611  /usr/libexec/at-spi2-registryd
greetd  1627  dbus-daemon --config-file=…/at-spi2/accessibility.conf
              --address=unix:path=/run/user/973/at-spi/bus
greetd  1791  qs -p /usr/share/apex-greet/shell.qml
```

`quickshell-git-0.3.1^860.gitc6a5160-1.fc43`, `at-spi2-core-2.58.8-1.fc43`,
`orca-49.7-1.fc43`, seat0 `ActiveSession=c1`.

**`/var/lab-scratch/katana-measure/after.txt` graded this COULD-NOT-RUN with the
reason "no quickshell process". The process is named `qs`.** `pgrep -x
quickshell` finds nothing on a machine whose greeter is quickshell. The same
wrong question is in `measure.sh`. Recorded because it is the second time in
this qualification that the starting sweep asked for the wrong string (the
first was `bluetoothctl devices`, §2).

### The first reading: zero, not one

`tests/atspi-walk.py` at the tip (sha256 `8593c3f8…`, copied to katana and the
hash compared there) run as the `greetd` user against that bus:

| | |
|---|---|
| `atspi-walk.py --count` | **0** |
| nodes in `--dump` | **0** |
| `org.a11y.Status` | `IsEnabled: false`, `ScreenReaderEnabled: false` |

`run-lockscreen-atspi.sh` records the symptom as "the shell publishes ONE node,
itself". On the shipped greeter it publishes **none**, because Qt's AT-SPI
bridge does not register at all while `org.a11y.Status.IsEnabled` is false. The
one-node reading came from a harness that had accessibility on.

### The second reading: flip the flag a screen reader flips

Orca sets `org.a11y.Status.ScreenReaderEnabled` when it starts. That property
was set to `true` over the greeter's own session bus — nothing was launched,
nothing spoke, no window was opened — and the tree re-walked 3 s later:

| | before | with the flag on |
|---|---|---|
| applications on the bus | 0 | **1** (`quickshell`) |
| nodes | 0 | **17** |
| max depth | — | **2** |
| nodes with a name | — | 11 |
| nodes exposing `org.a11y.atspi.Action` | — | **12** |

```
root | role=application | name=quickshell | desc=/usr/bin/quickshell
  2147483664 | role=frame
    2147483648 | role=text         | name=Username | desc=The account to log in as            | actions=SetFocus
    2147483649 | role=push button  | name=Keyboard layout: English (US)                        | actions=Press,SetFocus,Press
    2147483650 | role=text         | name=         | desc=Password. Press Enter to log in, …   | actions=SetFocus
    2147483651 | role=alert message| name=
    2147483652 | role=push button  | name=Previous session                                     | actions=Press,SetFocus,Press
    2147483653 | role=label        | name=Session: APEX Tiling                                 | actions=SetFocus
    2147483654 | role=push button  | name=Next session                                         | actions=Press,SetFocus,Press
  2147483662 | role=frame                                                                       (the same seven again)
```

**This is the single largest open accessibility defect in the repository
closing, measured rather than argued.** `run-lockscreen-atspi.sh` records the
tree as one node and nothing beneath it; the build on this image carries
upstream `916a0dd` ("launch: avoid creating multiple QApplications") and
publishes a frame per output with every control under it.

**Do not expect `tests/check-quickshell-a11y-cause.sh` to go red because of
this.** That suite's own header says it is a pin on *Qt's* behaviour — mode A
reproduces the QCoreApplication-destruction shape in a 168-line program with no
quickshell in it, and that shape still produces a null root. What changed is
that quickshell no longer has that shape. The thing to revisit is
`run-lockscreen-atspi.sh` §5, whose assertions are skipped on the grounds that
there is nothing under the root to read back. On this image there is.

Four things worth reading off that tree rather than the headline:

- **The password field has no name and keeps its description.** `Accessible.
  passwordEdit` makes Qt's bridge return an empty name, which is exactly the
  behaviour `check-lockscreen-a11y.sh` asserts on the QML side and which had
  never been confirmed on the bus. A screen reader is told what the field is
  for and is not told what is in it.
- **Two frames, one per output — checked, not inferred.** sway's own IPC
  (`swaymsg -t get_outputs` on `/run/user/973/sway-ipc.973.1518.sock`) reports
  **2 outputs, both `active`, both `scale 1.0`**: `HDMI-A-1` 1920x1080@239.96
  and `eDP-1` 1920x1080@144.03. DRM agrees (`card2-HDMI-A-1`, `card1-eDP-1`).
  So the greeter publishes the whole login form twice, once per panel.
- **12 of the 17 nodes expose the `Action` interface** with `Press` and
  `SetFocus`, so the tree is operable and not merely readable. No action was
  invoked: pressing "Next session" at a live greeter changes what the machine
  would log in to.
- The `alert message` node is present with an empty name — the error line, with
  no error to show.

### Orca at the greeter, and greeter audio (P2-003 queue items 1 and 2)

Both are **could-not-run**, with the reason stated and the parts that *are*
measurable measured:

| | |
|---|---|
| `orca` installed | yes, `orca-49.7-1.fc43.noarch` |
| `speech-dispatcher` | yes, `0.12.1-5.fc43` |
| `espeak-ng` | yes, `1.51.1-12.fc43` |
| `python3-speechd` | yes |
| `spd-say` | `/usr/bin/spd-say` |
| ALSA cards | 2 |
| pipewire in the greetd session | **inactive** |

Starting Orca makes the machine talk out loud, and nobody is at katana to hear
it or stop it; "greeter audio" is a claim about a sound a person has to hear.
What can be said: **the software chain is complete and pipewire is not running
in the greeter session**, which is the thing to look at first when somebody does
run it.

### The gate, and the entry point that turns out to exist

The chain is: Orca starts → it sets `org.a11y.Status.ScreenReaderEnabled` → Qt's
bridge registers → the tree appears. Measured above, end to end.

A first draft of this section said there was no way into that chain except
finding a terminal. **That was wrong, and reading the machine is what corrected
it.** `/usr/share/apex-greet/sway-greet.conf` binds exactly one key, and it is
this one:

```
bindsym --to-code Mod4+Mod1+s exec /usr/libexec/apex-screen-reader toggle
```

with a comment explaining that it is the host's only binding precisely because
"a user who cannot see the screen has no way to ask for a reader through a
client they cannot read", that `--to-code` binds the physical key so the
shortcut survives a layout that has not been chosen yet, and that Orca sets the
gating property itself. `/usr/libexec/apex-screen-reader` is on the image, 8159
bytes, and `apex-screen-reader status` run as the `greetd` user answers `off`
with exit 1 — the documented contract. The design is complete; it was the
reading that was incomplete.

Confirmed rather than assumed, having got that wrong once: the greeter's
quickshell process (pid 1791) carries **no** `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`
and no other `QT_*`/`A11Y`/`NO_AT_BRIDGE` variable — its full environment is 40
keys and the only Qt one is `QT_QPA_PLATFORMTHEME=qt6ct`. So the gate really is
the only switch, and the keybind really is the only way a user flips it.

What this does mean, and it is worth stating because it is how both `after.txt`
and this section first read the machine wrongly: **anything that inspects the
a11y bus without claiming to be a screen reader sees an empty bus.** An audit,
a CI probe or a magnifier would report the shell as publishing nothing, and be
wrong. Any future check of this must set the flag first, as this one did.

### The machine was put back

| | |
|---|---|
| `org.a11y.Status` | `IsEnabled: false`, `ScreenReaderEnabled: false` — both as found |
| greetd | `active`, `qs` still pid 1791, `sway` still pid 1518 |
| `/etc/greetd/config.toml` vs `.orig-qual2` | `cmp` → **identical** (never touched) |
| seat0 `ActiveSession` | `c1`, unchanged |
| failed system units | 0 |
| `apex game status` | `active : false` |

**One residue, stated rather than glossed:** `atspi-walk.py --count` still
answers `1`. Qt's bridge does not *un*register when the flag goes back to
false, so quickshell stays on the a11y bus until the greeter next starts. It
holds nothing, costs nothing, and clears at the next greeter start — but it
means the pristine "0 applications" reading is only obtainable on a greeter
nobody has asked.

### P2-003 scorecard

| row | verdict |
|---|---|
| quickshell publishes an accessibility tree | **pass — 17 nodes, 2 frames, depth 2** |
| the tree is operable (`Action` interface) | **pass — 12 nodes, `Press`/`SetFocus`** |
| the password field does not leak its contents over the bus | **pass — empty name, description kept** |
| an entry point exists for a user who cannot see the screen | **pass** — `SUPER+ALT+S` → `/usr/libexec/apex-screen-reader toggle`, the greeter host's only keybind; `status` answers `off`/rc 1 as documented |
| the bus is empty until a screen reader asks | **by design, and a trap for auditors** — 0 applications while `IsEnabled` is false; any probe must set the flag first |
| Orca at the login screen (queue item 1) | **could-not-run** — pressing the key makes the machine speak, and nobody is at it |
| greeter audio (queue item 2) | **could-not-run** — same; pipewire is inactive in the greeter session, which is the first thing to check |
| magnifier / high-contrast / reduced-motion | **not measured this round** |

---

## 4. P1-038 — the session rows, and the agent that was already using the seat

A labwc session was armed the documented way and came up: `qual-greetd-restore`
dead-man first, then
`greetd-set.sh apex-labwc "" p1038-labwc`, then `systemctl restart greetd` (the
script clears `/run/greetd.run`, which is the whole trick — a bare restart
starts the greeter and logs nothing unusual). Session 68 on seat0/tty1,
`Type=wayland`, with `labwc` 9632, `quickshell -c /usr/share/apex-shell` 9702
and `Xwayland :0`.

**It was given up 6 minutes later, deliberately.** See the last subsection.

### 4.1 The real shell's accessibility tree — the reading this round was worth most

`run-lockscreen-atspi.sh` records the shell as publishing one node:
`root | role=application | name=quickshell | ChildCount=0`. On this image, in a
real labwc session, with `ScreenReaderEnabled` set:

| | |
|---|---|
| applications on the bus | **3** — `quickshell`, `polkit-mate-authentication-agent-1`, `xdg-desktop-portal-gtk` |
| `quickshell` nodes | **9** (application + **8 `frame`s**) |
| max depth | 1 |
| named nodes under the application | 0 |
| nodes exposing `Action` | 0 |
| re-walk at t+5 s and t+15 s | 11 total nodes both times — **not lazy** |

So `ChildCount` moved from 0 to 8 and the frames arrived; **what did not arrive
is anything inside them.** The bar publishes no controls.

Then the session was locked, which is the surface `run-lockscreen-atspi.sh` is
named for:

```
  2147483727 | role=frame        | states=active,enabled,sensitive,showing,visible
    2147483721 | role=text  | name= | desc=Type your password and press Enter to unlock.
               | states=editable,enabled,focusable,focused,sensitive,showing,visible | actions=SetFocus
    2147483722 | role=label | name= | states=enabled,focusable,read-only,sensitive,showing,visible | actions=SetFocus
  2147483728 | role=frame        (the same two again — one lock surface per output)
```

17 nodes while locked. **The lock screen IS reachable, and the password field
again has an empty name and keeps its description** — the same
`Accessible.passwordEdit` behaviour §3 measured at the greeter.

The conclusion is sharper than "P2-003 is fixed": **the markup that exists now
reaches the bus, and the bar has none.** The lock screen is marked up
(`check-lockscreen-a11y.sh` asserts it) and appears; the bar's eight frames are
empty because there is nothing attached to their contents. That is a concrete,
bounded next task, and it could not have been seen before this image.

#### Two traps for whoever measures this next

- **Ask for the bus address; do not guess the path.** In a user session it is
  `/run/user/1000/at-spi/bus_0`, not `…/at-spi/bus`, and **it does not exist
  until something calls `org.a11y.Bus.GetAddress`**. A probe that stats the
  path reports "no accessibility bus" about a perfectly healthy session. This
  round did exactly that and got a false negative before correcting it.
- **`loginctl unlock-session` does not unlock the APEX lock screen.**
  `LockedHint` stayed `yes` after it. That is arguably right — a lock any
  process on the bus can lift is not a lock — but it is undocumented and it
  strands an unattended run. Recorded, not graded.

### 4.2 Application floors

Floors only. None of these is the row; each is the part of the row that can be
read without a person looking at the screen.

| row | floor measured | verdict |
|---|---|---|
| 11 LibreOffice | `soffice --writer` maps `libvcllo.so` with `libgtk-3.so.0.2420.32` and `libwayland-client.so.0.26.0` in `/proc/<pid>/maps` — **the gtk3 VCL plugin, on Wayland** | **floor: pass.** The row (gtk3 *and* qt6 VCL) still needs a person: only the plugin actually loaded can be read this way |
| 12 Blender | `blender --factory-startup` runs and maps `libwayland-client`, `libX11` **and** `libxcb` | **floor: pass, backend undecided.** Blender links all three; the maps cannot say which GHOST backend it chose, and it printed no backend line |
| 8 Wine / XWayland | **blocked on a broken install — see below** | **could-not-run** |

#### `apex install wine` on katana produced a wine that cannot run anything

```
$ wine64 notepad
wine: created the configuration directory '/var/home/andre/.wine'
wine: could not exec wineserver
```

| binary | katana |
|---|---|
| `wine64` | `/usr/bin/wine64` |
| `wineserver` | **ABSENT** |
| `wineboot` | **ABSENT** |
| `winecfg` | **ABSENT** |
| `wine` | ABSENT (expected — not a Fedora binary name) |

`katana-p1038-apps-20260922.md` says "`/usr/bin/wine64`, `wineboot`, `winedbg`
and `/usr/bin/chromium-browser` are all present", and records as a finding that
a `command -v wine` answering ABSENT is the wrong question. **The right question
answers ABSENT too**: `wineserver` is the process every wine invocation execs
first, and it is not on this machine. Row 8 is blocked on the install, not on
the compositor — and the earlier evidence's binary list needs correcting.

### 4.3 Rows 13 / 14 / 15 — could-not-run, and the reason is another agent

Scale (1.25 / 1.5 / 1.0), transform (90 / normal) and mode (60 Hz / 240 Hz) were
scripted against `wlr-randr` with the shell's pid asserted unchanged after each.
`wlr-randr` had listed both outputs in full 90 seconds earlier — `HDMI-A-1`
Lenovo R25f-30 at 1920x1080@239.96 position 1920,0, `eDP-1` AU Optronics 0x978F
at 1920x1080@144.03 position 0,0, both `Transform: normal`, `Scale: 1.000000`,
`Adaptive Sync: disabled`. By the time the loop ran, every call answered
`unknown output` and `wlr-randr --json` answered `[]`.

The cause is not labwc. At **03:53:07** a second desktop session started on
tty2, `PAMName=login`, `XDG_VTNR=2`, `XDG_SESSION_DESKTOP=Hyprland`, as
`qual-sess-hyprland.service` running `/usr/bin/start-hyprland` — and `chvt 2`
took seat0 with it. An inactive session's compositor has no outputs, so
`wlr-randr` correctly reported none. Rows 13/14/15 measured nothing and are
graded **could-not-run**, reason: the seat was taken mid-run.

### 4.4 Another agent was on this machine at the same time

This is the finding of section 4 and it is about the program, not the image.

`sudo`'s own journal for 03:35–03:55 carries commands that are not this unit's:

```
 3  systemd-run --unit=qual-sess-hyprland … --property=PAMName=login
                --property=TTYPath=/dev/tty2 … /bin/sh -lc /usr/bin/start-hyprland
 5  systemctl stop qual-sess-hyprland.service
 3  chvt 2
 4  /usr/libexec/apex-greet-wallpaper
 3  gsettings get org.gnome.desktop.interface toolkit-accessibility
 2  python3 /tmp/a11y-probe.py
 3  rm -f /var/tmp/qual-sess-hyprland.log
```

That is a second roadmap agent running **P2-003 accessibility work on a
Hyprland session**, started, stopped and restarted three times while this unit
was arming a labwc session on the same seat. Neither unit's card mentions the
other. Two agents were dispatched at one physical seat in round 40.

Nothing was fixed here and nothing of theirs was touched. This unit stood down:

| step | reading afterwards |
|---|---|
| disarm this unit's `qual-greetd-restore` dead-man **first** — its action restarts greetd and would have stomped their session at 04:32 | 0 timers matching `qual-greetd-restore*` |
| restore `/etc/greetd/config.toml` from `.orig-qual2` **without** restarting greetd | `cmp` identical; sha256 `d7298f54fa4bf46de3a426e3a6abecac02b7d64a7e1f5bfe03b4662682514d3d`, the pre-arming value; **0** `initial_session` lines; `/run/greetd.run` present, so the next restart starts the greeter and not a session |
| `loginctl terminate-session 68` (this unit's own session only) | `labwc` count **0**; greeter back as `c2`, its `qs` running |
| `chvt 2` — hand the seat back | seat0 `ActiveSession=89`, their Hyprland session, where it was |
| whole machine | 0 failed units; `apex game status active: false`; `/sys/kernel/sched_ext/state` `disabled`; 0 leftover blender/soffice/wine; 0 timers of this unit's left |

`qual-sess-hyprland.service` was left running and was never stopped by this
unit.

### P1-038 scorecard

| row | verdict |
|---|---|
| 1–5 Firefox/Chromium sharing, OBS, Discord, Flatpak portals | **could-not-run** — a portal picker is a dialog somebody has to choose in; Discord and Chromium can also raise a keyring prompt on a screen nobody is at |
| 6–7 Steam, gamescope | **not re-run** — `katana-image-qual-20260919.md` §6.3 already reached Steam Big Picture inside gamescope on the RTX 3070 on this machine. Cited, not repeated |
| 8 Wine / XWayland | **could-not-run — broken install.** `wineserver`, `wineboot` and `winecfg` are absent; `wine64` dies with `could not exec wineserver` |
| 9 VS Code, 10 JetBrains | **could-not-run** — VS Code can raise a keyring prompt unattended; JetBrains was deliberately not installed |
| 11 LibreOffice | **floor: pass** (gtk3 VCL on Wayland). Row needs a person for the qt6 plugin |
| 12 Blender | **floor: pass** (runs, maps a display stack). Backend undecidable from maps |
| 13 fractional scaling, 14 rotation, 15 refresh-rate | **could-not-run** — the seat was taken by another agent's session mid-run; `wlr-randr` then reported no outputs |
| 16 VRR | **permanently could-not-run on katana** — no connector exposes `vrr_capable`. Confirmed again this round: `wlr-randr` reports `Adaptive Sync: disabled` on both outputs |
| 17 suspend/resume | **could-not-run** — suspending a machine nobody is at, with this program's record of a suspend that did not come back |
| 18 output hotplug | **could-not-run** — plugging a cable needs a person |

What section 4 does add to P1-038: the labwc session **comes up from greetd on
this image**, with the shell, Xwayland, the portal and the polkit agent all
registering; two real outputs at genuinely different DPI are present and
enumerable; and the accessibility half of the matrix has a real reading for the
first time.

---

## What this round changed, in one place

| | |
|---|---|
| **P1-043** | Gaming Mode has loaded a sched-ext scheduler through apexd, on hardware, for the first time. Two new defects: `root/ops` carries a build-ID suffix, and the `switch` branch has a 1.42 s race that reports a live scheduler as `not loaded` |
| **P2-003** | The greeter publishes 17 accessibility nodes; the shell publishes 9 and its lock screen 17; the bar publishes none. The one-node era is over |
| **P2-005** | The IPP path completes end to end against two vendors' printers without paper |
| **P2-006** | All the NetworkManager and firewall lines answer; the hotspot row stays could-not-run for two named reasons |
| **P2-007** | udev, udisks2 and bluetooth all answer; the tool's `paired devices` reading is right and the starting sweep's was wrong |
| **P1-038** | Two floors measured, one row blocked on a broken wine install, and three rows lost to a second agent on the same seat |

---

## 5. Reconciling with `katana-a11y-20260922.md`, which landed while this ran

The other agent's evidence reached `roadmap/v2.2` during this round. The two
runs agree on the headline and **disagree on the mechanism**, and the
disagreement is dated, so it is recorded here rather than resolved by picking
one.

### Where they agree, and it is a real cross-check

| | their run | this run |
|---|---|---|
| compositor | **Hyprland** | **labwc** |
| `quickshell` `ChildCount` | **8** | **8 frames** |
| other apps on the bus | `polkit-mate-authentication`, `xdg-desktop-portal-gtk` | the same two |
| shim needed | none | none |

Two different compositors, two different probes, the same eight. That is worth
more than either reading alone, and it is exactly the number round 30 could
only reach with an `LD_PRELOAD` shim.

### Where they disagree

`katana-a11y-20260922.md` concludes:

> Qt decides whether to publish when the application object is built. Turning
> the bridge on later does not retrofit it. So on a machine where
> `org.gnome.desktop.interface toolkit-accessibility` is false at login — which
> is the **default** — the shell ships an empty tree no matter which quickshell
> is installed.

**This run is a counter-example, and the timestamps are on the machine.**

| time (AWST) | event |
|---|---|
| 03:47:5x | labwc session 68 starts; `quickshell -c /usr/share/apex-shell` pid 9702 |
| ~03:48:5x | `org.a11y.Status` read: `IsEnabled: false`, `ScreenReaderEnabled: false`. `ScreenReaderEnabled` then set to `true` over the session bus |
| ~03:49 | the already-running pid 9702 publishes **8 frames** |
| ~03:49:3x | session locked; the same process publishes **17** nodes, lock surfaces included |
| **03:51:08** | the other agent runs `gsettings set org.gnome.desktop.interface toolkit-accessibility true` — `~/.config/dconf/user` is **created** at `03:51:08.251`, and it is the only file in that directory |

So for the whole of this run's measurement, `andre` had **no dconf user
database at all** and `toolkit-accessibility` was at its schema default. The
shell process had already started. It retrofitted anyway, and it retrofitted
twice — frames first, then the lock surfaces added live.

The environment reading is the other half: pid 9702, like the greeter's pid
1791, carried **no** `QT_LINUX_ACCESSIBILITY_ALWAYS_ON` and no other
accessibility variable.

### What the reconciliation probably is, stated as a hypothesis and not a result

The property Qt's bridge watches at runtime is `org.a11y.Status` on the
accessibility bus. The gsetting is what `at-spi-bus-launcher` mirrors *into*
that property; it is one writer of the gate, not the gate. Setting
`ScreenReaderEnabled` directly — which is what Orca does, and what this run did
— moves the gate without touching dconf, and the running process picks it up.

If that is right, the other run's first reading of `0` had some other cause
(the walk landing before the bridge had connected is the obvious candidate),
and the remedy its evidence proposes — ship the gsetting on, or set
`QT_LINUX_ACCESSIBILITY_ALWAYS_ON` for the shell — is a good idea for a
different reason: it makes the tree visible to an auditor who is *not* a screen
reader. It would not be needed to make Orca work.

**Neither claim should be quoted without the other until somebody runs the
discriminating experiment**, which is one session: start the shell with
`toolkit-accessibility` false and no `org.a11y.Status` write, walk (expect 0),
then set `ScreenReaderEnabled` alone and walk again. If the second walk is 8,
the gsetting is not the gate.

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

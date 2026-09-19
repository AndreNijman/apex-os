# P1-043 / unit `gaming-scx` — Gaming Mode now loads a sched-ext scheduler, and says so only when it did

2026-09-20, apex-os `task/gaming-scx` from `roadmap/v2.2` (`777ba028`).
**Pushed, not merged.** Katana and the L16 were both left alone: every row
below is a fixture, and the rows that genuinely need hardware are written into
`docs/gaming-and-sessions.md` §6.8 rather than claimed here.

## 1. The defect, as found

`ROADMAP/evidence/integrated-image-20260920.md` §7 recorded it during the
katana image qualification:

```
apexd: scxctl switch -s scx_lavd failed (exit status: 1):
       error: no scx scheduler running, use 'start' instead of 'switch'
apexd: game: sched-ext: scx_lavd for the session, kernel scheduler restored on exit
```

Two separate faults, and the second is the expensive one.

1. `apexd/apexd-core/src/syswriter.rs:739` ran `scxctl switch`. APEX loads no
   scheduler at boot, so the first entry into Gaming Mode always finds none,
   and `switch` is precisely the verb that cannot work from there. Present in
   `-b -1` and `-b -2` as well: not a regression, it has never worked.
2. The second line was `apex game status`'s **only** sched-ext output, and it
   was a sentence copied out of the *plan*. It asserted as fact the thing the
   line above it had reported as failed. Nobody could have noticed, because the
   only surface that reported sched-ext reported the intention.

## 2. The vocabulary, read rather than guessed

`scxctl` 1.1.2 (`scx-scheds-1.1.3-3.fc43`, the same build katana has) is
present on the L16, so `--help` answered without touching either machine:

```
Commands: get  list  start  switch  stop  restart  restore  help
start   <--sched <SCHED>|--mode <MODE>|--args <ARGS>>
switch  <--sched <SCHED>|--mode <MODE>|--args <ARGS>>
```

**`scxctl get` was deliberately NOT run.** `/usr/share/dbus-1/system-services/
org.scx.Loader.service` exists, so `get` is bus-activating: it would have
started `scx_loader` on Andre's daily machine, which is inactive and disabled
there. Checked before running anything.

Both refusal strings were read out of the shipped binary with `strings`, and
the first of them matches katana's journal verbatim:

| state | `start` | `switch` |
|---|---|---|
| nothing attached | attaches | `error: no scx scheduler running, use 'start' instead of 'switch'` |
| one attached | `error: scx scheduler already running, use 'switch' instead of 'start'` | replaces |

So **neither verb may be hardcoded.** `start` would have worked on exactly the
machines APEX ships today and failed on any machine already running a
scheduler — the same defect with a different spelling.

Also read off the L16 rather than assumed: `/sys/kernel/sched_ext/` publishes
`state`, `enable_seq`, `hotplug_seq`, `nr_rejected`, `switch_all`, and
**`root/` does not exist while `state` is `disabled`** — the scheduler's
kobject appears only when one is attached. `state` here reads `disabled`,
`enable_seq` 0, `nr_rejected` 0.

## 3. What changed

### The verb comes off the kernel

`/sys/kernel/sched_ext/state` decides: `enabled` → `switch`, anything else →
`start`. A **single** retry on whichever verb `scx_loader`'s own error names
covers the race and the unreadable-state case. One retry, capped, so a loader
that ping-pongs cannot spin; and only those two messages justify it, because
retrying on any failure would hide the real reason behind the second one's.

### `scxctl` exiting 0 is a fact about `scxctl`

After a successful call the writer waits, bounded at 2 s (`SCX_SETTLE`, a named
constant because BPF attach is asynchronous — reading `state` the instant
`scxctl` returns would report a working scheduler as a failure), then reports
**what it read**. A command that succeeds and changes nothing comes back
`Refused`, naming both halves.

### `Outcome` gains a third answer

`Landed` / `Refused(why)` could not hold "I could not tell", so it held it as
one of the other two. `Outcome::Unknown(why)` is the third, and `landed()` is
false for it — nothing that counts landings counts one.

### `ScxState`, rooted at `sys_root`

`Enabled{ops}` / `Disabled` / `InFlux(s)` / `Unsupported` / `Unreadable(why)`.
The old code hardcoded `/sys/kernel/sched_ext` **even on a writer built with an
explicit `sys_root`**, so the only machine that could exercise the path was the
one running the tests. `Unreadable` is not `Unsupported`: a refused read is not
an absent feature, which is this program's most-repeated defect.

### The status surface

`apex game status` gains `scx_requested`, `scx_state`, `scx_detail`.
`scx_state` is one of four words and **`loaded` is reachable only from a kernel
reading**. The keys are reported while game mode is OFF too, so "disabled
before, disabled during" reads as the non-answer it is.

## 4. What `apex game status` says in each case

Rendered exactly as the CLI's own generic renderer prints the Status dict
(`apexd/apex/src/main.rs` `GameCmd::Status`). Produced from the daemon's real
`game_status()` over `game_enter()` — a throwaway probe test, removed after,
source restored byte-identical and verified with `cmp`.

**Loaded** — the kernel says a scheduler is attached:

```
scx_requested: scx_lavd
scx_state : loaded
scx_detail: asked for scx_lavd; scxctl reported success; sched_ext/state is
            enabled, root/ops reads 'lavd'
notes     : sched-ext: loaded — asked for scx_lavd; scxctl reported success;
            sched_ext/state is enabled, root/ops reads 'lavd'
```

**Not loaded** — three different causes, one honest word, each keeping its own
reason:

```
# a) scxctl refused — the katana case, in the shape the journal recorded it
scx_state : not loaded
scx_detail: asked for scx_lavd; scxctl refused: scxctl start -s scx_lavd:
            error: no scx scheduler running, use 'start' instead of 'switch';
            sched_ext/state is disabled — nothing is attached

# b) the command said yes and the machine did not move — the general form of
#    the whole defect, and the row that matters most
scx_state : not loaded
scx_detail: asked for scx_lavd; scxctl reported success; sched_ext/state is
            disabled — nothing is attached

# c) the kernel cannot run one at all — a definite answer, not an unknown
scx_state : not loaded
scx_detail: asked for scx_lavd; scxctl refused: scxctl: this kernel has no
            sched_ext support (CONFIG_SCHED_CLASS_EXT); this kernel has no
            sched_ext (CONFIG_SCHED_CLASS_EXT) — no scheduler can attach
```

**Unknown** — neither, and never rounded to either:

```
# a) sched_ext present, state unreadable
scx_state : unknown
scx_detail: asked for scx_lavd; scxctl could not be confirmed: scxctl start -s
            scx_lavd reported success and the result could not be confirmed;
            sched_ext is present and could not be read:
            /sys/kernel/sched_ext/state: Permission denied (os error 13)

# b) mid-transition
scx_state : unknown
scx_detail: asked for scx_lavd; scxctl reported success; sched_ext/state reads
            'enabling' — mid-transition, so this is not an answer yet
```

And a profile with `scx = ""`: `scx_state : not requested`, `scx_requested`
empty, and **no sched-ext note at all** — silence, not a note about a step that
is not in the plan.

A live session carries **one** sched-ext note, the measured one. The plan's own
note is an intent pointing at `scx_state` for the answer; once there is an
answer, the measurement replaces it rather than sitting under it. `apex game
profile`, which renders a plan nobody applied, still shows the intent, because
there it is the only true thing available.

## 5. `root/ops` is the struct_ops name, and comparing it verbatim would be this defect inverted

The kernel publishes `lavd`, not `scx_lavd`; `rusty`, not `scx_rusty`. A
verbatim comparison would report a perfectly good scheduler as the wrong one.
`scx_ops_matches` strips the prefix from either side, and a genuine mismatch is
**reported** (`— NOTE: 'rusty' is not the scheduler that was asked for`) and
never treated as "not loaded", because something being attached is a different
problem from nothing being attached.

**This is an expectation, not a measurement.** No machine here can load a
scheduler to look at `root/ops`, so §6.8 Row A asks for the string verbatim.

## 6. The sibling audit the defect asked for

The shape is "a command whose result is assumed rather than read". Every
sibling in `syswriter.rs` was checked:

| writer | reads its own result? | verdict |
|---|---|---|
| `write_tolerant` / `write_forced` / `write_if_present` | yes, `Outcome` | fine |
| `write_policy_attr` (fan-out) | yes, any-landed | fine, documented as such |
| `fan_safe_restore` (ladder) | yes, branches on `landed()` | fine |
| `cgroup_ensure` / `cgroup_remove` | yes | fine |
| `run_nvidia_smi` | yes — **and nobody read it** | **DEFECT, fixed** |
| `run_scxctl` | yes — **and nobody read it** | **THE defect, fixed** |

`run_nvidia_smi` returned a perfectly good `Outcome::Refused` that nothing
consumed: `apex game status`'s `gpus_locked` was `plan.gpus_locked.clone()`,
the list the plan MEANT to lock. A GPU whose clock lock `nvidia-smi` rejected
was reported as locked. It now holds the GPUs **every one of whose** lock
writes landed (a card whose graphics clock locked and whose memory clock did
not is not a locked card), with `gpus_lock_attempted` beside it and the refusal
in `notes`.

Two more, fixed in passing:

* **`game_exit` discarded every outcome but a hard `Err`**, so a refused
  restore was silent underneath `game mode OFF — tier restored to …`. It now
  logs each non-landing exit action and a count.
* **The "game mode ON" journal line** said `{N} GPU(s) locked` from the plan's
  count and did not mention sched-ext. It is now `{landed}/{attempted}` and
  carries `sched-ext {verdict}`.

`apply_all` still discards per-action outcomes — by design, documented on the
trait: "some of this plan landed" is not a fact anything can act on. It is used
for tier plans, and no status surface reports a tier as *applied*; `apex power
status` reads the tier back out of sysfs. Left alone.

## 7. Gates

```
cargo test --locked --workspace --no-fail-fast   3450 passed, 0 failed
cargo clippy --locked --workspace --all-targets -D warnings   clean
tests/test-apex-gaming.sh            131 passed, 0 failed
tests/test-apex-modes.sh              67 passed, 0 failed
tests/test-apex-gaming-session.sh     46 passed, 0 failed, 0 skipped
tests/check-doc-verbs.sh      264 valid, 0 not a command, 0 stale, 0 undeclared
tests/check-suites-run-in-ci.sh       79 suites, 0 unrun and undeclared
tests/check-shellcheck-coverage.sh    170 scripts, 0 newly failing
tests/check-no-conflict-markers.sh    PASS
```

New assertions: 17 in `apexd-core/src/syswriter.rs` (`mod scx_tests`), 11 in
`apexd/src/game.rs`, 2 in `apexd-core/tests/gamemode.rs`. No new suite, so
nothing to add to `tests/suites-not-in-ci.txt`; all three files already run in
CI through `cargo test`.

**`cargo fmt` was deliberately not run**, and that is not an omission: the
workspace has never been rustfmt'd, `.github/workflows/build-image.yml:382`
says the `--check` step is absent for that reason and names the day to add it
back, and rustfmt is not installed here. Running it would have reformatted 24
unrelated files.

**Nothing in the suite can reach a real scheduler.** The fixture constructor
`RealWriter::for_scx_test` is `#[cfg(test)]`, because the existing
host-commands guard exists precisely because a live writer in a test once
reached the developer's own scheduler through a process spawn no `sys_root` can
redirect — and raised a burst of polkit prompts doing it. The fake `scxctl` is
a shell script rooted in the test's own scratch directory and it only ever
writes that directory's fixture sysfs.

One flake found and fixed while writing them: a sibling test's still-open write
handle to its own fake makes `execve` here fail with `ETXTBSY`, because `fork`
in another thread duplicates every open fd. The module takes one lock; it runs
in 0.67 s either way, and five consecutive runs were 17/17.

## 8. Mutation testing — 13 mutations, each named the row it turned red

Every source restored with plain `cp` and verified byte-identical with `cmp`.

| # | mutation | rows turned red |
|---|---|---|
| MB1 | the shipped defect back: `switch` from `disabled` | `a_kernel_with_no_scheduler_running_gets_start_and_not_switch`, `the_wrong_verb_is_retried…` |
| MB2 | return `Landed` without the read-back | `a_command_that_exits_zero_and_changes_nothing_is_refused_not_landed`, `a_scheduler_stuck_enabling…`, `a_state_that_cannot_be_read_back…` |
| MB3 | `Unreadable` verdict → `not loaded` | `read_scx_state_tells_absence_from_a_refused_read`, `the_three_verdicts_are_distinct_words` |
| MB4 | `scx_ops_matches` compares verbatim | `the_struct_ops_name_drops_the_scx_prefix_and_the_comparison_knows_it` |
| MB5 | `ScxReport::verdict` trusts `applied` over `observed` | `a_command_that_said_yes_over_a_kernel_that_says_disabled_is_not_loaded`, `a_plan_applied_through_the_recording_mock…` |
| MB6 | `gpus_locked` back to the plan's list | `gpus_locked_reports_what_nvidia_smi_accepted_and_not_what_was_planned` |
| MB7 | `MockWriter::scx_state` answers `Disabled` | `a_plan_applied_through_the_recording_mock_is_unknown_and_never_loaded` |
| MB8 | no retry on the verb the loader names | `the_wrong_verb_is_retried_on_the_one_the_loader_names_and_only_once` |
| MB9 | retry on ANY failure | `a_refusal_the_loader_does_not_name_a_verb_for_is_not_retried` |
| MB10 | `Unsupported` verdict → `unknown` | `the_three_verdicts_are_distinct_words`, `a_kernel_without_sched_ext_is_not_loaded_rather_than_unknown` |
| MB11 | the plan note asserts success again | `the_plan_note_asks_for_a_scheduler_and_does_not_claim_one` |
| MB12 | the plan note stops naming the scheduler | same row — it fails in both directions |
| MB13 | `scx = ""` still plans a switch | `a_profile_that_asks_for_no_scheduler_plans_no_note_about_one` + 3 pre-existing |
| MB14 | keep the plan's intent note beside the measurement | `a_live_session_shows_the_measurement_and_not_the_intent_beside_it` |

**MB11 escaped on the first pass.** Nothing pinned the plan-time note, so
reverting it to the sentence that had claimed a scheduler on three images
turned nothing red — a gate that inspects nothing, in this unit's own work.
Two assertions were added and MB11/MB12 then caught it in both directions.
Recorded because the near-miss is the finding.

## 9. Run-book rows changed

`docs/gaming-and-sessions.md`:

* **§6.6 destroy step** — `sudo systemctl restart greetd` → `kill -9` on the
  session owner. The restart takes the seat away, but the session script
  survives long enough to run its own `EXIT` trap, so game mode is released by
  the ordinary cooperative path. That is a good outcome and a **different
  row**; the row is "nothing can cooperate".
* **§6.6 `cat /sys/kernel/sched_ext/state  # expect: enabled`** — deleted. It
  read `disabled` during the session as well as after, so the row would have
  passed on a file that never moved. Replaced by
  `apex game status | grep -E '^(tier|prior_tier|scx_)'`.
* **§6.6 "the readings that actually move … are the cgroup, sched-ext and
  `active`"** — corrected. Sched-ext did not move. The readings that moved are
  the cgroup, `active` and the apexd journal line.
* **§6.6 non-discriminators** — the governor caveat was already there; it is
  now one of a *pair*, with `sched_ext/state` beside it and the reason each is
  one on katana specifically.
* **§5a** — the claim that katana sat with `scx_lavd` running for 75 minutes is
  corrected in place, marked as a correction rather than edited away. The
  cpuset, the IRQs and the tier were real.
* **§5c, NEW** — the defect, the two verbs, the read-back, the three states,
  the struct_ops prefix, the sibling defects.
* **§6.8, NEW** — the three rows that need hardware.

`docs/apexd-dbus.md`: the same 75-minute claim corrected; the new Status keys
documented, including why they are reported while game mode is off.

`tests/doc-verbs-undocumented`: `apex game start` dropped — it is named in a
document now, which is what that file's reverse pass is for.

## 10. NOT done, and named rather than glossed

* **No hardware row was run.** Katana is the record behind a completed
  qualification and the L16 is Andre's daily; neither was touched. §6.8 has the
  exact commands.
* **`root/ops` reading `lavd`** is an expectation. §6.8 Row A asks for it
  verbatim.
* **Game mode STOPS a scheduler it found running rather than restoring it.**
  `scxctl restore` exists and would be the fix. Not done here because no APEX
  image loads a scheduler at boot, so nothing has ever reached that branch —
  and a restore path nobody can exercise is worse than a named limitation. The
  exit log says which case it was; §6.8 Row C expects the line.
* **`scxctl start -m <mode>`** — `scx_loader` has `Gaming`, `LowLatency`,
  `PowerSave` and `Server` modes and APEX passes `-s` only. Whether `-m gaming`
  beats a bare `scx_lavd` is a tuning question for a machine with a game on it.
* **`SCX_SETTLE = 2 s`** is reasoned, not measured on hardware. §6.8 says to
  record the number if Row A comes back `unknown` on `enabling` rather than
  re-running until it passes. The constant's own doc comment says so too — it
  briefly claimed "measured against katana-class hardware", which nothing here
  had done, and writing that in the file that fixes an unverified claim would
  have been the defect one directory over.

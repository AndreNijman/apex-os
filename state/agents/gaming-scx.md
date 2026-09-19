# unit: gaming-scx — P1-043 (sched-ext)

**Status: COMPLETE and PUSHED, not merged.**
Repo `apex-os`, branch `task/gaming-scx`, worktree
`/var/tmp/apex-work/wt-gaming-scx`, from `roadmap/v2.2` (`777ba028`).

Four commits, tip `4bcf76cd`:

```
4bcf76cd fix(gaming): one sched-ext note in a live session, and a constant …
30cea72f docs(roadmap): evidence for gaming-scx …
a197d6ca docs(gaming): correct the two run-book rows that proved nothing …
015c3de1 fix(gaming): load the sched-ext scheduler, and stop claiming one …
```

`git merge-tree --write-tree origin/roadmap/v2.2 task/gaming-scx` → exit 0,
tree `f84c16bd`, **0 conflicts**, against `origin/roadmap/v2.2` at `777ba028`
(re-fetched 2026-09-20; the tip had not moved). Run, not assumed.

Evidence: `ROADMAP/evidence/gaming-scx-20260920.md` (tracked, on the branch).
P1-043 recorded `partial` with `set-status.py`: prior evidence read out with
`yaml.safe_load` first and carried forward **whole** — verified afterwards by
`prior in evidence` → True, 14 551 → 21 172 chars, and a per-item diff showing
P1-043 as the only row that changed. `partial` because this item's other open
half is untouched (safe GPU controls on AMD/Intel cannot distinguish "left at
default" from "no knob exists"; Safe Graphics' automatic dGPU branch needs the
panel genuinely dark).

## What was wrong

`apexd-core/src/syswriter.rs` ran `scxctl switch` where the machine needed
`scxctl start`, so Gaming Mode had **never once** loaded a sched-ext
scheduler — three images, every boot. Worse: `apex game status`'s only
sched-ext output was a sentence copied out of the *plan*, printed directly
beneath the journal line recording the refusal.

## What was done

* The verb is now read off `/sys/kernel/sched_ext/state`, with one retry on
  whichever verb `scx_loader`'s own error names. Neither verb is hardcoded —
  `start` alone would fail on any machine already running a scheduler.
* `Outcome` gains `Unknown(String)`; `landed()` is false for it.
* `ScxState` (Enabled{ops}/Disabled/InFlux/Unsupported/Unreadable), rooted at
  `sys_root` so fixtures reach all five. The old code hardcoded `/sys` even on
  a fixture-rooted writer.
* After a successful call the writer waits, bounded at 2 s, and reports what
  the kernel says. A command that exits 0 over a machine that did not move is
  `Refused`.
* Status gains `scx_requested`, `scx_state` (`loaded` / `not loaded` /
  `unknown` / `not requested`), `scx_detail`. Reported while game mode is off
  too. `loaded` is reachable only from a kernel reading.
* Sibling audit: `gpus_locked` had the identical defect (the plan's list) and
  is fixed the same way, with `gpus_lock_attempted` beside it. `game_exit`
  discarded every outcome but a hard `Err`; it now reports them.
* Run-book: §6.6's destroy step is now `kill -9` on the owner (a greetd
  restart lets the trap run, so it tested the wrong thing); the
  `sched_ext/state # expect: enabled` row is gone; §5c (the defect) and §6.8
  (the three hardware rows) are new. §5a and `docs/apexd-dbus.md` had both
  claimed katana ran `scx_lavd` for 75 minutes — corrected in place.

## Gates

3450 workspace tests / 0 failed, clippy clean, test-apex-gaming 131/0,
test-apex-modes 67/0, test-apex-gaming-session 46/0, check-doc-verbs 0 stale
0 undeclared, check-suites-run-in-ci 0 unrun, check-shellcheck-coverage 0
newly failing, no conflict markers. **14** mutations, each named the row
it turned red; sources restored with plain `cp` and verified with `cmp`.

MB11 escaped the first pass — nothing pinned the plan-time note, so reverting
it to the sentence that had claimed a scheduler on three images turned nothing
red. A gate that inspects nothing, in this unit's own work. Two assertions
were added; MB11 and MB12 then caught it in both directions. Recorded because
the near-miss is the finding.

`cargo fmt` was NOT run and that is not an omission: the workspace has never
been rustfmt'd, `.github/workflows/build-image.yml:382` records that the
`--check` step is absent for that reason, and rustfmt is not installed here.

**Hardware: NOTHING was touched.** Not katana, not the L16. `scxctl --help`
on the L16 is the only thing run, and `scxctl get` was deliberately avoided
because `org.scx.Loader.service` is bus-activating and `get` would have
started `scx_loader` on Andre's daily.

## NEXT

Nothing is outstanding in the repository. The remaining work is a machine and
a merge, in that order of interest:

1. **Land the branch.** `merge-tree` was clean against `roadmap/v2.2`
   `777ba028` (see above); re-check if the tip has moved since. Landings on
   this program are merges, not rebases.
2. **Run `docs/gaming-and-sessions.md` §6.8 on katana, from an image that
   carries this branch.** Three rows: (A) a scheduler attaches from `disabled`
   — and **record `root/ops` verbatim**, expected `lavd`, an expectation no
   machine here can check; (B) it goes away again; (C) the `switch` branch,
   which needs `sudo scxctl start -s scx_rusty` first because it is the one
   branch a fixture cannot honestly stand in for. On an older image
   `scx_state` is absent from `apex game status` entirely, which is how you
   tell the image is too old for the row.
3. **Re-run §6.6 with the corrected recipe** (`kill -9` the owner, not a
   greetd restart) and record `scx_state` as the fourth discriminator. It was
   not one before, because nothing ever loaded.

Known limitations, written down rather than left to be rediscovered:

* **Game mode STOPS a scheduler it found already running instead of restoring
  it.** `scxctl restore` exists and is the fix. Not done here: no APEX image
  loads a scheduler at boot so the branch has never been reached, and a
  restore path nobody can exercise is worse than a named limitation. The exit
  log says which case it was; §6.8 Row C expects that line.
* **`scxctl start -m <mode>`** is unused — `scx_loader` has `Gaming`,
  `LowLatency`, `PowerSave`, `Server`. A tuning question for a machine with a
  game on it, not a correctness one. Deliberately not folded into the row that
  proves the scheduler loads at all.
* **`SCX_SETTLE = 2 s`** is reasoned, not measured on hardware. If §6.8 Row A
  returns `unknown` on `enabling`, raise it and record the number — do not
  re-run until it passes.

**Memory vault was unreachable the whole session** (`claude-memory` MCP,
CONNECT_TIMEOUT), so no journal entry was appended and no note was written to
it. Two things from this round belong there when it is back, both of the
"a checked fact, not an assumption" kind: `scxctl get` is bus-activating via
`org.scx.Loader.service`, so running it to inspect state STARTS `scx_loader` on
whatever machine you are on; and a Rust test that writes an executable and then
runs it can fail with `ETXTBSY` because `fork` in a sibling test thread
duplicates that still-open write fd — one module-level mutex fixes it.

If you are picking this up cold: `apexd` is the cargo root (cargo from the
repo root exits 101), and the scx tests live in `apexd-core/src/syswriter.rs`
`mod scx_tests` rather than in `tests/` because the fixture constructor is
`#[cfg(test)]` on purpose — a production constructor that lets a live writer
run host commands against a fixture would reopen the door the host-commands
guard was built to close.

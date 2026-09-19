# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

items: P2-003, P2-004
repo: both
worktree: /var/tmp/apex-work/wt-p2-b4
branch: task/p2-b-round31
second_worktree: /var/tmp/apex-work/wt-p2-b4-sh
second_branch: task/p2-b-round31
scratch: /var/tmp/apex-work/scratch-p2-b/round31/

The branch name moved to `task/p2-b-round31` this round (both repos, both cut
from `roadmap/v2.2` and pushed empty at the start: apex-os `da6fc0fb`,
apex-shell `8eccbfa`). It must exist and be pushed in BOTH repos even when a
repo gets no commits, because apex-shell's CI looks for a matching branch name. Full
pre-prune cards: `scratch-p2-b/p2-b.card.pre-round27-prune.md` and
`scratch-p2-b/p2-b.card.pre-round28.md`.

## NEXT

**Round 31 is COMPLETE and pushed in both repos.** Nothing on it is half-done.
apex-shell tip `f9b189e` on `task/p2-b-round31` (`1003192` is the last commit
that touches anything CI executes; `f9b189e` is a comment-only `ci.yml`
correction); apex-os tip `43ffce13` on the
same branch name (the branch has to exist in both repos because apex-shell's CI
matches on branch name; apex-os got one commit this round, the RTL link-4
guard). CI run 35415236747 is read and recorded at the end of the DONE section.

Do NOT re-run any of the following on this laptop. Each is CLOSED here and the
numbers are in the DONE section: the RTL mirror suite (`run-rtl-test.sh` 30/0/0,
`mutate-rtl.sh` 13/13), `mutate-lockscreen-atspi-shim.sh` (8 applied / 6 CAUGHT
/ 0 SURVIVED / 2 HELD, re-earned in `73566cd`), and the recovery read-back pair
(`run-recovery-atspi-shim.sh` 33/0/1, `mutate-recovery-atspi-shim.sh` 15 applied
/ 12 CAUGHT / 0 SURVIVED / 3 HELD). Each mutation harness costs eight to ten
minutes of full bring-ups, because every mutant is a complete compositor,
two-bus, whole-shell bring-up.

Next action for whoever picks this up, in order:

1. **Decide what CI should do about `run-rtl-test.sh`'s Arch-runner red, which
   is no longer an open question.** That step has been red since it landed and
   the standing queue has carried it as "cause unknown" for four rounds. FOUND
   26 closed the cause, and the one line of it that had been inferred rather
   than measured is now CONFIRMED FROM THE RUNNER, twice (CI 35388802498 and
   35415236747): Arch's upstream `qt6ct 0.11-8` links no `libKF6I18n.so.6`, and
   that library is what loads the Qt catalogue and flips the layout direction.
   Fedora's build (`qt6ct-0.11-13.20250907git23a985f.fc43`) links it, so APEX
   inherits right-to-left from a packaging choice and from nothing APEX
   declares. The runner's three reds are therefore **truthful about that
   machine** — they are exactly the three rows that need a right-to-left
   application direction, and that machine has no process that loads the
   catalogue. It is now 19 passed / 3 failed / 3 skipped there (was 17/3/2):
   same three reds, two more assertions passing.

   So this is a decision, not an investigation, and it is worth taking to the
   orchestrator rather than guessing:
   * leave it red, and accept a permanent red step in `arch-validate`; or
   * make the runner's configuration match what APEX actually ships — install
     `ki18n` there and `LD_PRELOAD=/usr/lib/libKF6I18n.so.6` for that step only.
     That is defensible because it reproduces APEX rather than hiding a gap, and
     the route is already MEASURED to work (FOUND 26's fourth row: no platform
     theme at all + that preload gives RightToLeft). It has to be written so the
     suite still reports WHICH mechanism supplied the direction, or it becomes a
     gate that inspects nothing.

   **Do NOT close it by making section 1 SKIP.** That section IS the
   discriminator; making it skip destroys the only instrument, and it has been
   refused three times.

2. **Standing queue item 7 — the honest remaining half of RTL.** 0 of 14 window
   roots carry `LayoutMirroring` or `layoutDirection`, so the shell's own
   windows do not mirror even though the shared settings surface does. Untouched,
   and nothing measures it yet.

3. **Queue items 1 and 2 need HARDWARE and were not reachable** — Orca at the
   login screen, and greeter audio. Say so rather than simulating them. On
   2026-09-19 katana was owned by two other agents (a hardware qualification and
   a TPM run) and was explicitly off limits.

**Two things to hand on rather than to do here.**

* **FOUND 28 is a PRODUCT defect, not an accessibility one**, and it is fixed on
  this branch (`5327bc6`): the factory reset could not be committed by anybody —
  mouse, keyboard or screen reader — for as long as that code has existed.
  Whoever owns the recovery flow should know it was found and how, and should
  read `mutate-recovery-atspi-shim.sh`'s R12 before touching
  `RecoveryService._onPlan()` or the page's phase-change re-ack: the guard is
  now two-deep on purpose and either half alone is sufficient, which is why that
  mutant needs two edits (FOUND 30).
* **The qtdeclarative one-liner (FOUND 20) is still unfiled**, deliberately.
  Filing on a public tracker is an outward-facing action and Andre's call, not
  an agent's. It is flagged to him. Until it lands, a real screen reader still
  gets ONE node from the shell on a real machine, and every read-back this unit
  has ever done is under a test-only LD_PRELOAD that ships nowhere.

## IN PROGRESS

Nothing. Round 31 is finished and pushed in both repos, and the two worktrees
(`wt-p2-b4`, `wt-p2-b4-sh`) are clean with their HEADs matching their remotes.

## DONE

Round 31 (2026-09-19). **The recovery screen is audited, both halves, and the
audit found a product defect that had nothing to do with accessibility.**

apex-shell `task/p2-b-round31`, seven commits on `roadmap/v2.2`'s `f068f24`,
all pushed (the seventh, `f9b189e`, is a comment-only `ci.yml` correction and is
described with the CI result below):

* `6c277ac` — the RTL half. `tests/run-rtl-test.sh` grows three rows and its
  round-23 "the theme supplies the direction" prose is corrected: the iff
  (direction flips ⇔ the theme's plugin drags in a Qt translation loader), a
  self-test proving that predicate can answer NO, and the mechanism pinned past
  its carrier (no theme + `LD_PRELOAD` of libKF6I18n → RightToLeft).
  **33 passed / 0 failed / 0 skipped** here under `env -i HOME PATH USER TMPDIR`
  (was 30/0/0). Both new assertions were proven able to fail by running the
  suite with a stubbed tool on PATH. Section 1's three existing assertions are
  untouched and it is NOT made to skip. FOUND 26.
* `aafff20` — the recovery screen's markup, and the group every Config page
  lacked. **1 → 90 nodes on the bus**, 96 with the loss list rendered, the tree
  grouped instead of flat, the Erase button reachable. FOUND 25 is the
  measurement it was written from.
* `e640744` — the source-level pair. `check-recovery-a11y.sh` and
  `mutate-recovery-a11y.sh`. **Current numbers, re-measured on the final tip
  and on the Arch runner, are 34/0/0 and 17 applied / 14 CAUGHT / 0 SURVIVED /
  0 MISSCORED / 0 UNSCORABLE / 3 HELD / 0 FALSE-RED** — NOT the 31/0/0 and
  14/11/3 this card carried mid-round, which were taken before `5327bc6` added
  three assertions and three mutants for the FOUND 28 fix. Identical on both
  machines.
* `73566cd` — FOUND 27. `mutate-lockscreen-atspi-shim.sh`'s S4 and G3 built a
  private-use character with `printf '\U000f033e'`, which is TEN ASCII BYTES
  under the C/POSIX locale `env -i` and the CI container both give. Round 30's
  CAUGHT for S4 was only true because this laptop's locale is UTF-8. Both now
  build it with `python3` from the number and **ABORT** rather than scoring
  anything if the result is not four bytes. Re-measured in full rather than
  assumed — eight mutants, each a compile plus a complete bring-up:
  **8 applied, 6 CAUGHT / 0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE, 2 HELD /
  0 FALSE-RED**, baseline 23/0/1, identical to round 30 with S4's CAUGHT now
  earned against a character that is really there.
* `5327bc6` — FOUND 28, and it is a PRODUCT defect, not an accessibility one:
  **the factory reset could not be committed by anybody.** `_onPlan()` assigned
  `plan` before `resetPhase`; QML notifications are synchronous, so the loss
  list acknowledged itself while the phase was still `"planning"`;
  `acknowledgeLossList()` refuses there at its first guard; nothing ever
  acknowledged again, so `commitReady` was false for ever and the Erase button
  was never visible to anyone — mouse, keyboard or screen reader. Found by
  pressing that button over AT-SPI, which is the only place it was reachable
  while invisible (FOUND 16). Fixed both ways round: the assignment order, and
  a phase-change re-ack on the page so the order stops being load-bearing.
* `93192e9` — the runtime read-back pair. `tests/run-recovery-atspi-shim.sh`
  brings up a private headless labwc, private session and a11y buses, loads the
  SHIPPED `shell.qml` under the round-30 preload, opens Nexus at the recovery
  page over the shipped IPC handler, and then **walks the whole destructive
  flow with nothing but DoAction**: open the disclosure, choose the scope, run
  the dry run, commit. The machine it interrogates is an `apex` stub answering
  from `recovery-test.js`'s captured payloads, and it **records its argv** —
  which is what makes both directions checkable: press Erase before the loss
  list exists and no `--commit` may appear; press it after and the exact argv
  must, confirm token included. The control is INSIDE the run (§4 requires
  exactly one node before the factory install). Wired into `ci.yml` and the
  REQUIRED list: **68 suites / 68 reachable**, every REQUIRED path stat-ed /
  0 missing, `shellcheck -S warning -x tests/*.sh` clean.
* `1003192` — FOUND 29 and 30. The pair's FIRST full run scored 11 CAUGHT /
  **1 MISSCORED**, and the MISSCORED was a defect in the SUITE: one assertion
  called by two different names depending on how it came out, so the harness
  verified the mutant's target against the `ok` wording and then could not find
  it in the `FAIL` wording. A real defect, correctly detected, reported as a
  broken expectation. Four rows had drifted; only one had a mutant pointed at
  it. All four fixed, and the class GATED by a new §0 that sits above every
  skip-out — proven to fail in both directions with a drifted probe copy
  (32/1/1, naming the title), and carrying its own negative control.

Final numbers on this laptop, measured twice (once interactive, once under
`env -i HOME PATH USER TMPDIR`, identical):

* `run-recovery-atspi-shim.sh` — **33 passed / 0 failed / 1 skipped.**
  1 node before the factory install, **90 after**. The one SKIP is
  `Accessible.announce()`: a description changing is not speech, and that is a
  could-not-run, not a pass.
* `mutate-recovery-atspi-shim.sh` — fifteen mutants, each a compile of the
  instrument plus a complete bring-up, eight and a half minutes:
  **applied=15 caught=12 survived=0 misscored=0 unscorable=0 held=3
  false-red=0.**

apex-os `task/p2-b-round31`, one commit on `roadmap/v2.2`'s `36535383`, pushed:
`43ffce13` gives `tests/test-apex-platform-theme.sh` link 4 in its live section
with the same can-it-answer-NO self-test — **20 passed / 0 failed / 0 skipped**
on this booted host (was 18/0/0) — and `mutate-platform-theme.sh` re-runs
unchanged at **13 applied, 10 CAUGHT / 0 SURVIVED / 0 MISSCORED, 3 HELD /
0 FALSE-RED**. FOUND 10's corollary applies: that harness aborts "tree dirty" on
an uncommitted edit, so run it after the commit.

**The new mutation harness was kill-tested rather than assumed safe.** Its trap
code is the proven `20a1613` shape, but its `FILES` now includes
`src/components/config/CfgSection.qml`, which every Config page instantiates, so
a restore that works by accident would be wide. Killed with SIGTERM once it was
past the baseline and into R1, and all five properties asserted: all four files
restore byte-identical to a sha256 taken before the run; there is **no totals
line** (it stopped rather than carrying on to score a mutant it never ran);
there is **no `cp:` error** (it stopped by design rather than by choking); it
exits **143**; and no snapshot directory is left in `TMPDIR`.

**FROM THE GITHUB ARCH RUNNER, not this laptop: CI run 35415236747** on
`1003192` — the last commit that touches anything CI executes. The one commit
after it corrects two stale COMMENTS in `ci.yml` (it named R7 as the FOUND-28
regression mutant, which is R12 and takes two edits, and still described the
source-level pair as eleven-plus-three when `5327bc6` made it fourteen-plus-
three). Comment-only, `ci.yml` re-parsed as YAML, not re-dispatched. `Repo Structure Sanity` **success** (so every REQUIRED path
exists there, the two new ones included) and `NixOS` **success**. `arch-validate`
red on **exactly one step — `Right-to-left layout baseline`** — the red this
unit has carried since round 28, which a control run on `roadmap/v2.2`
(35351740747) already proved is no branch's doing. Every other step passed, and
the totals lines rather than the ticks:

* `recovery-a11y: passed=34 failed=0 skipped=0` and
  `mutate-recovery-a11y: applied=17 caught=14 survived=0 misscored=0
  unscorable=0 held=3 false-red=0` — identical to this laptop, so the
  source-level pair is proved on two machines.
* `recovery-atspi-shim: passed=1 failed=0 skipped=1`. **The one pass is §0**,
  and that is the whole reason §0 was put above the skip-outs: the runner has no
  quickshell, so every other assertion in that suite is unreachable there, and
  without §0 the step would have measured NOTHING on the machine the gate runs
  on. It reported `every assertion is called by the same name whether it passes
  or fails — 30 ok / 33 FAIL / 12 SKIP titles`.
* `mutate-recovery-atspi-shim: applied=0 …` and exit 0 — the FOUND 18 baseline
  guard fired rather than scoring fifteen mutants against a suite that made one
  assertion. Working as intended on the machine that taught it.
* `run-rtl-test: 19 passed, 3 failed, 3 skipped` — was 17/3/2. The same three
  reds, and two more assertions passing. **The iff PASSED there, in the opposite
  branch to the one it passes in here**: *"it did NOT flip, and
  /usr/lib/qt6/plugins/platformthemes/libqt6ct.so links no libKF6I18n, so this
  machine's red above is that build of qt6ct and not APEX."* The LD_PRELOAD pin
  correctly reported COULD-NOT-RUN. So FOUND 26's one inferred line is measured
  on the machine it is about, twice now (also run 35388802498).
* The round-30 lock-screen shim pair reports its named refusals unchanged:
  `lockscreen-atspi-shim: passed=0 failed=0 skipped=1` and the mutator's guard
  at `applied=0`.

**What round 31 does NOT claim.** Not that the shell is accessible. The factory
is put back by a test-only `LD_PRELOAD` that ships nowhere, and on a real
machine a screen reader still gets ONE node from the shell. What is proved is
that this page's markup is correct all the way to the bus and that its flows are
completable through it, so the whole remaining defect is the upstream one in
FOUND 20 — still unfiled, deliberately, because filing is Andre's call.

Round 30 (2026-09-19). **FOUND 14 is closed**: named cause, standalone
reproduction, in-situ confirmation, and the lock screen's markup read back off
the bus for the first time. See FOUND 20–23.

apex-shell `task/p2-b-round30`, four commits on `roadmap/v2.2`'s `d5c781a`,
all pushed:

* `273c1fb` — `tests/quickshell-a11y-cause.cpp` (the five-mode reproduction),
  `tests/check-quickshell-a11y-cause.sh` (**14 passed / 0 failed / 0 skipped**)
  and `tests/mutate-quickshell-a11y-cause.sh` (**13 applied, 11 CAUGHT /
  0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE, 2 HELD / 0 FALSE-RED**). The suite
  is a PIN: mode A must produce a null root, so the day Qt or qtdeclarative
  fixes this it goes RED and says to delete the pin and write the real
  read-back. It needs no compositor, no bus and no quickshell, which is why it
  can run on the Arch runner where nothing else in this unit's shell half can.
* `b01a276` — `tests/quickshell-a11y-shim.cpp` (the LD_PRELOAD),
  `tests/run-lockscreen-atspi-shim.sh` (**23 passed / 0 failed / 1 skipped**,
  six runs) and `tests/mutate-lockscreen-atspi-shim.sh` (**8 applied,
  6 CAUGHT / 0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE, 2 HELD / 0 FALSE-RED**).
  Both wired into `ci.yml`, all six new files in the structure-check REQUIRED
  list, `check-suites-run-in-ci.sh` 66 suites / 66 reachable / 0 exempt,
  `shellcheck -S warning -x tests/*.sh` clean, `check-no-conflict-markers.sh`
  PASS, and every REQUIRED path stat-ed (110 checked, 0 missing).

Both mutation harnesses now restore from their **trap**, not only after each
mutant: before that, a CI step timeout would have left a mutated file in place
for every later step in the same job, and because these files are in the
REQUIRED list the structure check would still have passed. The first version of
that (in `b01a276`) restored correctly **by accident**, and `20a1613` fixes it —
see FOUND 24. Do not repeat the claim `b01a276`'s message makes.

* `20a1613` — `trap put_back EXIT INT TERM` does not END the script on a
  signal. The kill test now asserts three things instead of one: the file is
  restored, there is **no totals line** (it stopped rather than carrying on to
  score a mutant it never ran) and there is **no `cp` error** (it stopped by
  design rather than by choking). All three pass; the harness exits 143;
  unkilled it is unchanged at 13 applied / 11 CAUGHT / 0 SURVIVED / 2 HELD.

`56d7521` is prose only — comments, `echo` lines and skip reasons in
`run-lockscreen-atspi.sh` — and it was verified as such rather than asserted:
the suite is 17/0/6 after it, unchanged, and round 29's whole mutation pair
re-runs at **11 applied / 8 CAUGHT / 0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE /
3 HELD / 0 FALSE-RED**, also unchanged.

apex-os `task/p2-b-round30`: cut from `6b531503`, pushed, **no commits** —
nothing this round needed apex-os. The branch exists because apex-shell's CI
matches on branch name.

P2-003's roadmap evidence carries the `273c1fb` half. The `b01a276` half and
FOUND 22/23 go in next.

**FROM THE GITHUB ARCH RUNNER, not this laptop.** Two runs: **35382910849** on
`b01a276` and **35383306764** on the final tip `56d7521`, which reproduces it
exactly. Taking the first: `Repo Structure Sanity` **success** (so all
110 REQUIRED paths exist there), `NixOS` **success**, and `arch-validate` red
on **exactly one step — `run-rtl-test.sh` at 17/3/2**, the pre-existing red this
unit has carried since round 28 and which a control run on `roadmap/v2.2`
already proved is nobody's branch's doing. Every new step passed, and the
important one is the first:

* `check-quickshell-a11y-cause.sh` — **14 passed / 0 failed / 0 skipped ON THE
  ARCH RUNNER**, `note: Qt 6.11.2, compiler g++`. All five modes RAN there.
  So FOUND 20 is not a Fedora fact or a Qt 6.10.3 fact: the same
  `~QCoreApplication` clears the same factory list on a different
  distribution and a different Qt minor. That is the whole reason this suite
  was built to need no compositor, no bus and no quickshell.
* `mutate-quickshell-a11y-cause.sh` — **13 applied, 11 CAUGHT / 0 SURVIVED /
  0 MISSCORED / 0 UNSCORABLE, 2 HELD / 0 FALSE-RED** on the runner, identical
  to here. The pin is mutation-proved on two machines.
* `run-lockscreen-atspi-shim.sh` — `SKIP: no quickshell, so nothing here was
  measured.` and `passed=0 failed=0 skipped=1`. The named refusal, as designed.
* `mutate-lockscreen-atspi-shim.sh` — `applied=0 … held=0 false-red=0` and exit
  0: its baseline guard fired rather than scoring eight mutants against a suite
  that never ran (FOUND 18 working as intended on the machine that taught it).
* `run-lockscreen-atspi.sh` — 8/0/1, unchanged by `56d7521`'s prose edit.

Runs **35383306764** (`56d7521`) and **35384106245** (the final tip `20a1613`)
give the identical lines and the identical single red step, so the round lands
green apart from a failure older than it.

Round 29 (2026-09-18).

apex-shell `task/p2-b-round29`, five commits on `roadmap/v2.2`'s `379eef8`,
pushed, NOT yet merged: `d710bfb` the AT-SPI read-back suite and the ported
harness, `f66db73` the mutation pair and the CI wiring, `c81c421` the
provenance self-check for the two copied files, `e65d378` the harness guard the
Arch runner made necessary (FOUND 18) plus the at-spi path list (FOUND 19), and
`5ae27c3` the per-mutant half of that guard — the WANTS array's comment claimed
a property only an array nobody forgets to update would have, so `mutate()` now
asks the same question about its own `want` and scores UNSCORABLE, which fails
the run. Final harness run: 11 mutants, **8 red CAUGHT / 0 SURVIVED /
0 MISSCORED / 0 UNSCORABLE, 3 green HELD / 0 FALSE-RED**.

apex-os `task/p2-b-round29`, two commits on `roadmap/v2.2`'s `1668ed9c`, pushed:
`4aae9249` the `GSETTINGS_BACKEND=memory` fix to `tests/lib/atspi.sh`, and
`cb02019f` the same at-spi path list. The two copies of that file still diff
empty apart from the provenance block.

New in apex-shell:

* `tests/run-lockscreen-atspi.sh` — **17 passed / 0 failed / 6 skipped** here.
  It brings up a private headless labwc, a private session bus and a private
  a11y bus, loads the shipped `shell.qml`, engages the lock through the shipped
  IPC handler, and reads the tree back over D-Bus.
* `tests/mutate-lockscreen-atspi.sh` — 11 mutants, each a full bring-up:
  **8 red CAUGHT / 0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE, 3 green HELD /
  0 FALSE-RED.**
* `tests/lockscreen-atspi-control.qml` — the control fixture.
* `tests/lib/atspi.sh` and `tests/atspi-walk.py`, ported from apex-os with a
  PROVENANCE header carrying the `diff` command that proves they are still
  copies (both empty today).
* Both suites wired into `.github/workflows/ci.yml` and all five files added to
  the structure-check REQUIRED list. `check-suites-run-in-ci.sh`: 64 suites,
  64 reachable, 0 exempt.

apex-os, re-run after the `atspi.sh` change and unchanged from the ledger:
`test-apex-greet-atspi.sh` 31/0/0, `mutate-greet-atspi.sh` 8 applied / 8 CAUGHT
/ 0 SURVIVED, `test-apex-greet-session-bus.sh` 37/0/0.

**From the runner, not this laptop.** CI run 35361832221 on
`task/p2-b-round29`: `structure-check` and the NixOS job green; `arch-validate`
red on two steps. One is `run-rtl-test.sh` at **17/3/2**, the pre-existing red
this unit has been carrying since round 28 — unchanged, and not this branch's
doing. The other was the new mutation step, and it was a REAL defect in the new
harness, not an environment problem: see FOUND 18. Both are fixed in `e65d378`.

Re-dispatched as **35362877551** and again on the final tip as
**35363876000** (both green on the two new steps), and this is the
second-machine result:
`run-lockscreen-atspi.sh` is **8 passed / 0 failed / 1 skipped on the Arch
runner**. The whole §1 control passed there — labwc came up headless, the
private a11y bus came up, `ScreenReaderEnabled` read back true, the frame came
back MAPPED and the `Accessible.name` came back verbatim. The one SKIP is the
shell half: the runner has no `quickshell` (AUR), so §2–§5 cannot run there, and
`mutate-lockscreen-atspi.sh` printed its new guard — naming the three
assertions the baseline did not make — and exited 0. **So the harness itself is
proven on two machines and the empty-tree finding is not a laptop artefact.**
The only red left in that run is `run-rtl-test.sh` at 17/3/2, which is older
than this branch.

## FOUND (still true; do not re-derive)

1. **The RTL switch is driven by a TRANSLATION CATALOGUE, not the locale.** Qt
   translates `QT_LAYOUT_DIRECTION` and compares to `RTL`. Measured under
   `env -i`: `ar/he/fa` + `QT_QPA_PLATFORMTHEME=qt6ct` → mirrored; `ur_PK` → NOT
   (no `qt_ur.qm`); `ar` with the theme unset → NOT. Mirroring on APEX is a side
   effect of a theming choice made for a dark palette. Guarded round 26.
2. **`qt6-qttranslations` reached the image only as a weak `Recommends` of
   `qt6-qtbase-gui`**, named in neither Containerfile, and owns `qt_ar.qm` —
   the whole mechanism. `apex-pkg` has a `--no-weak-deps` flag, so a routine
   slimming pass would have deleted RTL silently. Fixed in `578914fd`.
3. **A QTranslator needs a HOST change; no QML can do it.** `/usr/bin/quickshell`
   constructs a bare `QQmlEngine`, never `QQmlApplicationEngine`, and calls
   neither QTranslator nor `installTranslator`. The process does get a
   translator — from the platform theme. What it lacks is a route to OUR
   catalogue. `run-i18n-test.sh` asserts the host facts so a future quickshell
   with the call makes the suite say so.
4. **The CJK "will tofu" claim was false** and sat in the ledger 19 rounds.
   `QFontEngineMulti` asks fontconfig for a font owning the glyph. RTL is a
   LAYOUT gap, not a font gap. Same for Hebrew/Thai/Devanagari.
5. **An assertion whose truth comes from the ambient environment is the same
   defect class as a gate that inspects nothing.** Run every suite under
   `env -i HOME PATH USER TMPDIR` before believing it.
6. **A mutation harness can misscore.** Verdicts are FOUR-way — CAUGHT /
   MISSCORED / **CRASHED** / SURVIVED — and the harness must be shown capable of
   all four. CRASHED exists because a mutant that makes the suite exit FATAL
   prints no totals line, and a harness that only counts failures reads that as
   "nothing failed": a working suite reported as a broken one.
7. **Never rely on D-Bus activation for the a11y bus on APEX.** `org.a11y.Registry`
   activation fails `Permission denied` — SELinux (`gnome_atspi_exec_t`), no AVC
   logged (dontaudit). Exec `at-spi-bus-launcher` directly; `tests/lib/atspi.sh` does,
   and that file now exists in BOTH repos.
8. **No `lupdate`/`lrelease` on this box** (`qt6-qttools` absent). `qmltestrunner-qt6`,
   `quickshell`, `labwc`, `sway`, `cage`, `Xvfb`, `xdotool`, `wtype`, `gdb` are
   present. `qmllint-qt6` is too, and it RESOLVES Quickshell's types.
9. **A new suite is not a gate until it is wired into a workflow.** apex-shell's
   workflow is `.github/workflows/ci.yml` (one `arch-validate` job on
   `archlinux:latest`, plus `structure-check` with a REQUIRED file list) and
   since round 28 `tests/check-suites-run-in-ci.sh` enforces REACHABILITY there.
   Every suite exits 0 on a SKIP: **read the totals line, never the tick.**
10. **Restore mutants authoritatively, and prove the restore before using it.**
    `git checkout --` CANNOT work in apex-shell CI: `arch-validate` runs Checkout
    BEFORE it installs git, so the workspace has no `.git` at all. Both
    `mutate-lockscreen-*.sh` restore from a pristine copy in a per-run
    `mktemp -d` and VERIFY sha256. Note the corollary found this round: apex-os's
    `mutate-greet-atspi.sh` DOES use git and therefore **aborts with "tree dirty"
    if you have an uncommitted edit to any file it mutates** — commit first.
11. **Design fork, settled round 2, option (b):** an installer-picked keyboard
    layout reaches the running session only by re-execing cage with
    `XKB_DEFAULT_LAYOUT`. There is no xkb layout in the APEX input model.
12. **A description is not speech.** A screen reader does not generally speak a
    `description-changed` event on the object that already holds focus.
    `Accessible.announce(message, politeness)` is the fix; Qt **6.8+**, image
    ships **6.10.3**, measured with a runtime probe.
13. **labwc 0.9.6 does not merely advertise `ext_session_lock_manager_v1` — it
    ACKNOWLEDGES the lock.** Measured round 29 from outside the process:
    `LockState.locked = true` over IPC makes `WlSessionLock.secure` flip, which
    starts a second `LockedHintService` chain, which the runner's `loginctl`
    recorder sees. `tests/run-lockscreen-atspi.sh` §3 asserts it and two mutants
    (R6, R7) prove the assertion is about `secure` and not about `locked`.
14. **quickshell publishes NO windows to AT-SPI. The shell's entire
    accessibility markup is unreachable at runtime.** The bus answers
    `name=quickshell, ChildCount=0` — the application node and nothing beneath
    it. Measured 2026-09-18 and controlled: the same harness, same compositor,
    same buses, same run, publishes a full three-node tree for a plain Qt Quick
    `Window` under `qml-qt6`; a plain `Window` created INSIDE quickshell is
    equally absent, so it is the process and not the window type;
    `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` changes nothing. In gdb, in the
    quickshell process: `QGuiApplication::topLevelWindows()` has 2 entries,
    `type(w0) == 1` (`Qt::Window`, so neither of the two kinds Qt filters),
    `QAccessible::isActive() == 1`, and **`QQuickWindow::accessibleRoot(w0)`
    returns NULL**. `QAccessibleApplication::childCount()` is the size of
    `topLevelObjects()`, which keeps only windows with a non-null accessible
    root (qtbase `src/gui/accessible/qaccessibleobject.cpp`), so a null root on
    every window IS a ChildCount of 0. The same gdb expressions against the
    control answer `AROOT 0x5587847b8570`, so the instrument reports non-null
    when there is something to report. quickshell 0.3.1's source contains no
    accessibility code at all (checked, the whole tree), so this is not
    something quickshell does on purpose. **This is now the biggest single
    P2-003 defect and it is upstream of everything this unit can write in QML.**
15. **The a11y status flags cannot be WRITTEN on a private HOME, and the failure
    is silent.** `at-spi-bus-launcher` backs `org.a11y.Status` with the GSettings
    key `org.gnome.desktop.interface toolkit-accessibility`. Writing it through
    the dconf backend needs `ca.desrt.dconf`, which cannot be activated on
    `atspi.sh`'s deliberately service-less private bus, so the D-Bus `Set`
    returns `()` and changes nothing, the flags read back false, and Qt publishes
    **an empty tree for every application**. That is indistinguishable from a
    program with no markup — the first draft of round 29's suite reported the
    whole shell as publishing nothing on a harness that was itself broken. Fixed
    by running the launcher with `GSETTINGS_BACKEND=memory` (scoped to the
    launcher) in BOTH copies of `atspi.sh`. Invisible until now because the
    greeter suites run with the real HOME, where a desktop has already set both
    flags true, and CI's bare fedora container has no gsettings schemas at all,
    where the launcher keeps the value internally and the write always worked.
16. **An unmapped Qt Quick window still publishes its whole subtree to AT-SPI.**
    `topLevelObjects()` filters on window TYPE and on having an accessible root
    and on nothing else. Measured: a `Window { visible: false }` came back as
    `role=frame` with `states=enabled,sensitive` — no `showing`, no `visible` —
    with its label child intact. So "a frame is on the bus" is NOT "a window is
    on screen"; the STATES are the only discriminator. This was found by a
    mutant that SURVIVED, and the survival was the harness's wrong expectation,
    not the suite's defect.
17. **The `LockedHintService` guard is measured, not assumed.** Under a private
    `XDG_RUNTIME_DIR`, `loginctl show-user $USER -p Display --value` yields no
    session id, the service logs `LockedHintService: loginctl show-user failed`,
    and the chain stops at step 1 — so `busctl --system … SetLockedHint` is never
    reached, before OR after a real lock engages. Recording stubs for both tools
    prove it, and mutant R5 proves the assertion can fire. **Two traps for
    anyone repeating this:** `headless_begin` makes `bin/loginctl` a SYMLINK to
    the shared `_stub`, so `cat > …/loginctl` writes THROUGH it and turns every
    other stub into a loginctl recorder — `rm -f` first. And `headless_begin`
    does not stub `busctl` at all, so `PowerProfileService` talks to the real
    system bus in every headless suite in this tree that does not add one.

18. **A mutation harness must check that the baseline ASSERTED what each mutant
    is aimed at, not merely that it was green.** Measured the hard way on the
    GitHub Arch runner, 2026-09-18: the a11y helpers were at a path
    `atspi.sh` did not know, `run-lockscreen-atspi.sh` skipped out before its
    first assertion and printed `passed=0 failed=0 skipped=0`, and
    `mutate-lockscreen-atspi.sh` read that as a clean baseline and scored
    ELEVEN mutants against it — 8 SURVIVED, 3 HELD, a red CI step full of
    confident verdicts about nothing. "Is the baseline green" is not the
    question; "did the baseline print an `ok` line containing this mutant's
    `want` string" is. The harness now checks exactly that, for every want,
    and names the missing ones. Negative control run here: with `quickshell`
    hidden from `PATH` it lists the three assertions that did not run and exits
    0 instead of scoring anything. **This is the gate-on-the-gate version of the
    defect family this program keeps meeting, and it will be in every mutation
    harness in both repos that only asks whether the baseline was green.**
19. **The two at-spi helpers are never on `$PATH` and their directory differs by
    distribution.** Fedora: `/usr/libexec/at-spi-bus-launcher`. Arch: FLAT in
    `/usr/lib/at-spi-bus-launcher` and `/usr/lib/at-spi2-registryd` — checked
    against the Arch package's own file list, not guessed. `command -v` cannot
    find either, so the hardcoded path list IS the search, and a wrong path used
    to be reported with the same words as an absent package. The list is now six
    entries wide in both copies and the refusal prints every path it tried.

20. **FOUND 14's CAUSE, MEASURED 2026-09-19: quickshell destroys a
    `QCoreApplication` before it creates its `QGuiApplication`, and that
    destruction CLEARS Qt's accessibility factory list for the rest of the
    process's life.** Not reasoned — reproduced in a 140-line standalone binary,
    `scratch-p2-b/round30/repro.cpp`, five modes, one process each, under
    `env -i` + `QT_QPA_PLATFORM=offscreen`, no AT-SPI bus needed (the symptom is
    visible in-process, before any bridge):

    | mode | shape | `accessibleRoot()` | `queryAccessibleInterface(qApp)->childCount()` |
    |---|---|---|---|
    | A | `new QCoreApplication` → `delete` → `new QGuiApplication` (**quickshell**) | **NULL** | **0** |
    | B | `new QGuiApplication` only (**`qml-qt6`**, the round-29 control) | NON-NULL, name `QTROOT` | 1 |
    | C | A + our own factory installed from a `Q_CONSTRUCTOR_FUNCTION` (**the shape qtdeclarative uses**) | **NULL** | **0** |
    | D | A + our own factory installed from `Q_COREAPP_STARTUP_FUNCTION` | NON-NULL, name `APEX-CUSTOM-ROOT` | 1 |
    | E | A + our own factory installed by hand after `new QGuiApplication` | NON-NULL, name `APEX-CUSTOM-ROOT` | 1 |

    The chain, each link now checked in source AND measured: qtbase
    `QAccessible::installFactory` registers `qAccessibleCleanup` with
    `qAddPostRoutine`, and `qAccessibleCleanup` does
    `qAccessibleFactories()->clear()`; post routines run from
    `~QCoreApplication`; qtdeclarative installs `qQuickAccessibleFactory` from
    `Q_CONSTRUCTOR_FUNCTION(QQuick_initializeModule)`, i.e. **once per library
    load**, so nothing re-installs it; quickshell's `src/launch/main.cpp:127`
    constructs a `QCoreApplication` to parse the command line and
    `src/launch/launch.cpp:282` does `delete coreApplication;` on the line
    before `new QGuiApplication(...)`. A comment at
    `src/core/logging.cpp:426` names that window explicitly ("while the event
    loop is destroyed between QCoreApplication delete and Q(Gui)Application
    launch"), so the destroy-and-recreate is deliberate.
    **Mode C is the load-bearing one**: a factory installed exactly the way
    qtdeclarative installs it is gone after the delete, which reads the cleared
    list directly instead of inferring it. **Mode D is the fix**, measured:
    `Q_COREAPP_STARTUP_FUNCTION` runs from `QCoreApplicationPrivate::init()` on
    EVERY application object — the log shows it firing twice, once per app — and
    its list is not cleared, so the factory comes back. So the one-line upstream
    change is in **qtdeclarative**, `Q_CONSTRUCTOR_FUNCTION` →
    `Q_COREAPP_STARTUP_FUNCTION` for the accessibility factory install;
    quickshell's `delete coreApplication` is the trigger, not the defect, and
    Qt Widgets is immune for the same reason mode D is (it installs its factory
    from `QApplicationPrivate::initialize()`, per instance, not per load).
    **Mode E is the shim** the `## NEXT` builds. `cleanupAdded` is a file-static
    bool that stays true after the first install, so the post routine is not
    even re-registered: the clear happens exactly once and is permanent.

21. **FOUND 20 CONFIRMED INSIDE THE REAL QUICKSHELL PROCESS, and the whole
    accessibility tree comes back the instant the factory is re-installed.**
    `scratch-p2-b/round30/shim.cpp` is an `LD_PRELOAD` that interposes
    `QGuiApplication::exec()` (`_ZN15QGuiApplication4execEv` is `U` in
    `nm -D /usr/bin/quickshell`, so it is interposable; the interposer
    tail-calls the real one through `dlsym(RTLD_NEXT, …)` because `exec()` is
    what calls `QAccessible::setRootObject(qApp)`), arms a `singleShot` timer so
    the measurement happens on the main thread AFTER the shell's windows exist,
    and then does measure → `installFactory` → measure. The factory it installs
    is a faithful copy of qtdeclarative's `qQuickAccessibleFactory`, including
    the `QQuickItemPrivate::get(item)->isAccessible` filter, covering
    `QQuickWindow`, `QQuickItem` and `QQuickTextEdit` — window-only would have
    produced a frame with zero children and read as a false negative, because
    `QAccessibleQuickWindow::child()` re-enters `queryAccessibleInterface` for
    each root item. It compiles against Fedora's installed private headers
    (`qt6-qtbase-private-devel` is present and
    `/usr/include/qt6/QtQuick/6.10.3/QtQuick/private/qaccessiblequick*_p.h`
    ship with `qt6-qtdeclarative-devel`), so no symbol aliasing was needed.
    Run by `scratch-p2-b/round30/probe-shim.sh` — the round-29 bring-up, private
    headless labwc, private session bus, private a11y bus, the SHIPPED
    `shell.qml`, preload scoped to the one `quickshell` command so labwc and
    `at-spi-bus-launcher` never see it:

    ```
    APEXSHIM before: topLevelWindows=4
    APEXSHIM before: window[0..3] type=1 class=ProxiedWindow root=NULL
    APEXSHIM before: appChildCount=0
    APEXSHIM: installFactory(apexQuickFactory) called
    APEXSHIM after:  window[0..3] type=1 class=ProxiedWindow root=NON-NULL
    APEXSHIM after:  appChildCount=4
    ```

    and on the BUS, same process, no restart: **1 node before, 5 after** — the
    application node plus four `role=frame` children with
    `states=enabled,sensitive,showing,visible`. So the empty factory list is not
    an inference from a standalone repro; it is read directly out of the live
    quickshell, which is the only way to read it at all (`qAccessibleFactories`
    is a `Q_GLOBAL_STATIC` and not an exported symbol, so gdb cannot reach it).
    Two details worth keeping: quickshell's windows are class `ProxiedWindow`,
    a `QQuickWindow` subclass, so the metaobject walk does reach `"QQuickWindow"`
    and the round-29 "return early above QQuickWindow" branch is **eliminated**;
    and the four frames came back with EMPTY names and ZERO children, which is a
    separate question from this one and is NOT yet explained — the desktop
    shell's bar may simply carry no `Accessible.*`, or there may be a second
    layer. Do not report the shell as accessible on the strength of this.

22. **`LockedHintService` silently drops a lock whose hint chain overlaps a
    FAILING one, and a flaky test is how it was found.** `_pump()` returns
    immediately while `_busy`; `_succeeded()` clears `_busy` and re-pumps;
    `_failed()` clears `_busy` and **deliberately does not** ("Deliberately no
    retry here", `src/services/system/LockedHintService.qml`). So a
    `setLocked(true)` that arrives while a failing chain is in flight only
    updates `_desired`, and nothing ever acts on it — the lock engages, the
    surface comes up, and logind is never told. `Lockscreen.qml`'s
    `Component.onCompleted` starts exactly such a chain at startup, and on a
    private runtime directory it always fails at step 1. Measured: the new
    shim suite's lock-acknowledged assertion went red on **2 runs in 9** while
    the lock had in fact engaged — §6 read the entire lock surface back in
    those same runs — and 5 runs in 5 once the suite waits for the startup
    chain's refusal before asking for a lock. `tests/run-lockscreen-atspi.sh`
    does not see this because its §2 waits for that same warning as an
    assertion of its own and closes the race **by accident**; delete that wait
    and its §3 becomes intermittent. The shipped consequence is real and is not
    a test problem: on a machine where any of the three steps fails
    transiently, a lock that engages during the failure reports nothing to
    logind, and P0-015's lock policy (`apexd/apex-agent-core/src/lock.rs`)
    reads `LockedHint`. One line fixes it — `_failed()` should re-pump, or the
    guard should be a trailing-edge timer. NOT this unit's item; hand it on.

23. **A mutant that SURVIVES is worth more than ten that are caught, and this
    round has one.** `mutate-lockscreen-atspi-shim.sh` S4 puts a private-use
    codepoint into an accessible string and requires the tree-wide scan to find
    it. It SURVIVED, and the survival was correct **twice over**, for two
    independent defects that a green run would have hidden for good:
    (a) the private-use character written into the mutant did not survive being
    written to the file — what landed was `U+F033` followed by the letter `E`,
    a different codepoint in a different plane — and nothing in the source made
    that visible; (b) the SUITE's own character class had lost its BMP bounds
    the same way, leaving `[-` plus the two supplementary planes, so a BMP
    private-use glyph could not have been caught even if it had arrived. Both
    now build their characters from NUMBERS: the class is written entirely in
    `\uXXXX`/`\UXXXXXXXX` escapes and covers all three planes (unit-checked
    against `U+0041`/`E000`/`F033`/`F8FF`/`F033E`/`10FFFC`, and against the old
    broken class for contrast), and the mutant builds its character with
    `printf '\U000f033e'` (`od` says `f3 b0 8c be`). **An unprintable character
    in a source file is not reviewable.** A third thing came out of the same
    mutant: it had been aimed at the field's `Accessible.name`, which
    `passwordEdit` suppresses, so the glyph would never have reached the bus at
    all — a mutant pointed at a string the bus never delivers proves nothing
    about the assertion it was aimed at, and a harness that scored it CAUGHT
    would have certified a check it never exercised.

24. **A signal trap whose handler only RETURNS does not stop the script, and a
    restore that works anyway is not a working restore.** `trap put_back EXIT
    INT TERM` in a mutation harness looks right and restores the files, but
    bash defers the signal until the running command substitution finishes,
    runs the handler — which restores and then deletes the snapshot — and then
    **resumes at the next line**: it scores a verdict for a mutant that was
    killed, and only then dies inside `restore()` on the snapshot the handler
    has just removed. The first kill test here reported "restored" and was
    believed; its log has exactly one `cp: cannot stat` and **no totals line**,
    which is what stopping-by-choking looks like. Fixed in `20a1613` with
    separate exiting handlers (`trap 'put_back; exit 130' INT`,
    `trap 'put_back; exit 143' TERM`, `trap put_back EXIT`) and a kill test
    that asserts all three properties. **The general lesson is the one this
    program keeps meeting from a different direction: a check with one
    assertion cannot tell "it worked" from "it happened to end up that way".**
    `tests/mutate-lockscreen-atspi.sh` (round 29) has the same `trap '…' EXIT
    INT TERM` shape and is deliberately NOT changed on this branch — changing
    it means re-running eleven full bring-ups to prove a change nobody has
    measured a failure from. Whoever touches it next should fix it then.

25. **The RECOVERY page has ZERO `Accessible.*` in 892 lines, and the
    consequence measured over real AT-SPI is worse than "unlabelled": the
    entire factory-reset evidence surface is INVISIBLE and its commit button
    DOES NOT EXIST on the bus.** Measured 2026-09-19 with round 30's shim, a
    private headless labwc, private session and a11y buses, the shipped
    `shell.qml`, and an `apex` stub answering `recover status --json`,
    `doctor --json` and `recover reset --scope desktop --json` from the
    captured fixtures `tests/recovery-test.js` already carries. Control in the
    same run: **1 node before the factory install, 42 after.** The shell log
    confirms the page had real data —
    `RecoveryService: 8 component row(s) - 1 needing attention; doctor: 6 of 7
    pass, 1 to read`. What reached the bus:

    * The shared `Cfg*` controls, correctly: `Re-check`, `Check`, `Open`, the
      two scope radio buttons, `Show what would be lost`, and the four `CfgRow`
      labels, each with its description and a `Press`/`SetFocus` action.
    * **Nothing page-specific at all.** Not the headline status line
      ("1 component needs attention"), not one of the 8 component rows, not one
      of the 6 recovery routes, not one of the 7 doctor checks, not one section
      title.
    * **`Accessible.onPressAction` really drives it**: `atspi-walk.py
      --do-action Open` returned True and the button's name flipped `Open` →
      `Close` on the next dump, so the disclosure genuinely opened over the
      bus. Then `--do-action "Show what would be lost"` also returned True —
      and the tree was **byte-identical apart from that one Open/Close word**.
      The dry run ran, the loss list rendered, and a screen reader learned
      NOTHING: no row, no count, no "NOT backed up" flag, and no
      `Erase N item(s) now` button, because that button is a bare
      `Rectangle` + `MouseArea` with no role, no name, no action and no key
      handling. So the most destructive verb in the product is, for a reader,
      a path that can be walked three steps and then dead-ends with no
      information — and for a keyboard user it cannot be pressed at all, while
      every SAFE control on the page can be.
    * Structural, and not specific to this page: the Nexus frame's children are
      **FLAT at one depth and mix several pages' controls** — `bg`, `accent`,
      `Rescan`, `tonal-spot`, `Night light`, `Temperature`, `Corner radius` are
      Colour/Display page controls sitting beside Recovery's. There is no page
      boundary and no heading, so a reader cannot tell which page is open.
      `showing,visible` does not separate them either: only `Re-check` and a
      scroll bar carry it, because everything else is below the fold in a
      1280x720 window — the same state that would mean "on another page".
    * Three of quickshell's five top-level windows publish a frame with ZERO
      children (TopBar and the borders), which is the other half of FOUND 21's
      unexplained empty frames: they have no `Accessible.*` either.

26. **P2-004's RTL discriminator is SOLVED, and the answer is neither
    candidate on the ledger. What supplies APEX's right-to-left layout
    direction is `libKF6I18n.so.6`, which Fedora's qt6ct build links against —
    not qt6ct, not the Qt version, not `/etc/xdg/qt6ct/qt6ct.conf`.** Four
    measurements here on Qt 6.10.3, one process each, `qmltestrunner-qt6
    -platform offscreen`, `LANG=LC_ALL=ar_EG.UTF-8`, everything else scrubbed,
    with `strace -e openat` naming every `qt_*.qm` the process opened:

    | case | `Qt.application.layoutDirection` | catalogues opened |
    |---|---|---|
    | `QT_QPA_PLATFORMTHEME=qt6ct` | **1 (RTL)** | `qt_ar.qm`, `qt_en.qm` |
    | a BOGUS theme name | 0 | none |
    | no platform theme | 0 | none |
    | no platform theme **+ `LD_PRELOAD=/lib64/libKF6I18n.so.6`** | **1 (RTL)** | `qt_ar.qm`, `qt_en.qm` |

    The last row is the one that settles it: with no platform theme at all,
    merely loading KF6's i18n library into the process loads the Qt catalogue
    and flips the direction. `ldd` on
    `/usr/lib64/qt6/plugins/platformthemes/libqt6ct.so` shows why it is there —
    Fedora's build is a post-release git snapshot
    (`qt6ct-0.11-13.20250907git23a985f.fc43`) linked against **eight** KF6
    libraries including `libKF6I18n.so.6`; the plugin's own binary contains no
    QTranslator code at all (`nm -DC`, `strings`). The Arch runner has upstream
    release **qt6ct 0.11-8**, read out of the CI log, which is the plain build.

    **CONFIRMED FROM THE RUNNER, 2026-09-18 run 35388802498.** The iff added to
    `run-rtl-test.sh` this round passed on the Arch runner in the OPPOSITE
    branch to the one it passes in here, which is exactly what it was built
    for: *"it did NOT flip, and
    /usr/lib/qt6/plugins/platformthemes/libqt6ct.so links no libKF6I18n, so
    this machine's red above is that build of qt6ct and not APEX."* Its
    can-answer-NO control passed there too, and the LD_PRELOAD pin correctly
    reported COULD-NOT-RUN because the runner has no libKF6I18n at all. So the
    one line of this finding that was inferred rather than measured is now
    measured, on the machine it was about.

    **Both ledger candidates are eliminated, by measurement rather than by
    argument.** The conf: repeating the ar_EG run with `XDG_CONFIG_DIRS` and
    `XDG_CONFIG_HOME` both pointed at an empty directory — so qt6ct cannot find
    `qt6ct.conf` anywhere — still gives RightToLeft. The Qt version: no longer
    needed as an explanation, and the runner's own log shows it has 64
    `qt_*.qm` including `qt_ar.qm`, and that `QLocale calls ar_EG.UTF-8
    right-to-left` PASSES there. Nothing is missing on the runner except a
    process that loads the catalogue. The runner's three reds are exactly the
    three that need a right-to-left APPLICATION direction, and they are
    truthful about that machine.

    **The shipped consequence is bigger than the CI red and is the same family
    as FOUND 2.** APEX's right-to-left layout does not depend on anything APEX
    declares. It depends on a Fedora packaging choice to link qt6ct against
    KF6I18n. If that build ever goes back to upstream's, or a slimming pass
    drops KF6, every mirrored surface silently stops mirroring and nothing goes
    red — the second time this exact silent-loss shape has been found under
    this row.

27. **A private-use character built by `printf '\U000f033e'` is TEN ASCII
    BYTES under a C/POSIX locale — which is what `env -i` gives and what the CI
    container gives — and that is the THIRD way this unit has lost one of these
    characters in transit.** Measured with `od`, not reasoned:
    `printf '\U000f033e'` gives `f3 b0 8c be` on a UTF-8 desk and
    `5c 55 30 30 30 46 30 33 33 45` under `env -i`. FOUND 23 has the other two,
    both from round 30, and both were fixed by building the character from a
    NUMBER. This one WAS built from a number and lost it anyway, at the shell.

    It reached a landed file: `tests/mutate-lockscreen-atspi-shim.sh`'s S4.
    Round 30 scored it CAUGHT, and that verdict was only true because this
    laptop's locale is UTF-8 — on the Arch runner S4 would have inserted the
    plain letters `\U000F033E`, which the suite's private-use class correctly
    does not match, and the harness would have reported a verdict about a
    mutant that was never the mutant it claimed to be. Invisible forever,
    because that harness skips on the runner for want of quickshell.

    Fixed in `73566cd` and re-measured in full rather than assumed: eight
    mutants, each a compile plus a complete bring-up, **8 applied, 6 CAUGHT /
    0 SURVIVED / 0 MISSCORED / 0 UNSCORABLE, 2 HELD / 0 FALSE-RED**, baseline
    23/0/1 — identical to round 30, with S4's CAUGHT now earned against a
    character that is really four bytes. Both harnesses now build it with
    `python3 -c` and **ABORT** rather than scoring anything if the result is
    not four bytes. An abort and not a skip, because a harness that quietly
    downgraded here would be the thing it exists to prevent.

    The general rule for this tree: **do not build a non-ASCII character in
    bash.** `check-recovery-a11y.sh` writes its canned QML through python3 with
    an `@PUA@` placeholder for the same reason, and carries a self-test
    requiring the substituted glyph to be ONE character rather than the five of
    `@PUA@`.

28. **THE FACTORY RESET CANNOT BE COMMITTED — BY ANYONE. `RecoveryService`
    assigns `plan` before `resetPhase`, the loss list acknowledges itself on
    the first of those, and `acknowledgeLossList()` refuses while the phase is
    still `planning`. Nothing ever acknowledges again, so `commitReady` is
    permanently false, so the Erase button is permanently invisible.** This is
    not an accessibility defect. It is a product defect that an accessibility
    read-back found, because the bus is the only place the button was visible
    at all.

    Measured, not reasoned. `src/services/RecoveryService.qml` `_onPlan()`:

        root.plan = p                  // fires lossList.onPlanChanged -> _ack()
        root.resetPhase = "planned"    // one line too late

    QML property notifications are synchronous, so `_ack()` runs on line 1 with
    the phase still `"planning"`, `acknowledgeLossList()` returns at its first
    guard, and `_ackToken`/`_ackCount` stay `""`/`-1`. `_ack()` is wired to
    `onPlanChanged`, `onShownChanged` and `Component.onCompleted` — none of
    which fires again after line 2.

    Instrumented in the live headless run (debug removed afterwards, tree
    reverted): `APEXDBG ack: plan=true shown=0`, `APEXDBG ack: plan=true
    shown=4`, then `APEXDBG press: commitReady=false phase=planned plan=true
    shown=4 losses=4`. And `Rec.commitArgv(plan, 4, plan.confirmToken)`
    evaluated against the same fixture in node returns a perfectly good
    `["apex","recover","reset","--scope","desktop","--commit","--confirm",
    "desktop:4:5d7f91ba"]`. So every input is right and the acknowledgement is
    the only thing missing.

    Why nothing caught it: `recovery-test.js` tests `recovery.js`, where
    `commitArgv` is correct; `check-recovery-ui.sh` reads the source and
    asserts the wiring EXISTS, which it does. Neither instantiates the QML, and
    the ordering is the whole bug. It is the same shape as FOUND 22 — an
    acknowledgement or a retry wired to triggers that do not include the one
    that makes it valid.

    Found only because the button, though never visible, IS on the
    accessibility bus (FOUND 16) with a Press action, so it could be pressed
    over AT-SPI and the refusal observed.

    **FIXED in apex-shell `5327bc6`, both ways round**, so the description
    above is of the defect AS FOUND and not of the shipped code: the two
    assignments are the right way round, and the page re-acknowledges on a
    phase change so the order stops being load-bearing. That makes the guard
    two-deep, which is why its regression mutant needs two edits — see FOUND
    30. The gate is `mutate-recovery-atspi-shim.sh` R12, CAUGHT.

29. **A suite that calls one assertion by TWO names — one when it passes and
    another when it fails — breaks every instrument pointed at it, and the
    breakage reads as a wrong expectation rather than as a defect.** Found by
    `mutate-recovery-atspi-shim.sh` on its first full run, 2026-09-19, and it
    is the assertion-level version of the shape this program keeps meeting.

    `run-recovery-atspi-shim.sh`'s pre-plan Erase row printed

        ok   … the Erase button reports NO states at all — a reader is told it is unavailable
        FAIL … the Erase button reports itself unavailable

    Nothing about that is visible in a diff. The mutation harness verifies a
    mutant's target against the `ok` wording — that is FOUND 18's guard, and it
    is right — then looks for the same wording in the `FAIL` line. It is not
    there. So R5 broke `enabled: RecoveryService.commitReady`, the suite
    correctly went red on exactly the row R5 was aimed at, and the harness
    scored **MISSCORED**: a real defect, correctly detected, reported as a
    broken expectation. A human diffing two runs loses the row the same way.

    **FOUR rows had drifted, not one.** Only R5 had a mutant pointed at it, so
    the other three were invisible and would have stayed invisible until
    somebody wrote a mutant for them. Checked by parsing the suite's own source
    rather than by reading it.

    Fixed two ways. The four titles are stable, with the variable detail moved
    into the second argument — `ok()` grew the optional detail argument that
    `bad()` and `nope()` already had, so a stable title does not have to
    swallow its own explanation. And the CLASS is gated: `run-recovery-atspi-shim.sh`
    §0 asserts that every FAIL title is a substring of some ok or SKIP title,
    which is the exact property a mutation harness depends on. §0 sits **above
    every skip-out on purpose** — the Arch runner has no quickshell and cannot
    reach a single other assertion in that suite, so anywhere lower this would
    go unchecked for ever. Proven in both directions rather than asserted: a
    probe copy with one FAIL title reworded reports 32/1/1 and names the
    offending title, and the check carries its own negative control (it runs
    its matcher over a pair that HAS drifted and reports CONTROL-FAILED if the
    matcher fails to flag it), so a matcher that silently stopped matching
    cannot read as a pass.

30. **A guard that is genuinely two-deep cannot be mutation-tested with one
    edit, and pretending otherwise is a false claim about the guard.** The
    FOUND 28 fix put `RecoveryService._onPlan()`'s two assignments the right
    way round AND gave the page a phase-change re-ack. Either half alone
    restores a completable reset, which is the point of writing it that way. So
    reverting the assignment order — the actual historical defect — leaves the
    suite green, and a harness that scored that as SURVIVED would be reporting
    a vacuous assertion where there is a two-deep guard, while a harness that
    picked whichever half it could break and called the result a regression
    gate would be claiming the guard is one-deep. `mutate_pair` applies both,
    prints why it is the only mutant that does, and is CAUGHT. The same
    reasoning is written down for the one assertion here that has NO mutant:
    "pressing Erase before the loss list exists commits NOTHING" is three-deep,
    so the harness records it as deliberately unmutated in its own totals
    output rather than quietly omitting it.

31. **Carrying roadmap evidence forward means READING it back correctly, and
    the obvious regex drops the last line.** `set-status.py` REPLACES evidence,
    so every round here has to read the existing block and append to it. Slicing
    the task's block as `s[i:s.find("\n- id:", i)]` leaves the final evidence
    line WITHOUT its trailing newline, so a `(?:    .*\n)+` match stops one line
    short. On 2026-09-19 that made a correct `set-status.py` write look like a
    66-character truncation, and the next step would have been to "fix" a tool
    that had done nothing wrong. Append the newline back before matching, then
    compare the stored text against the intended text character for character —
    both items here round-trip exactly and the file parses at 128 tasks. Checked
    against the raw file, not against the reader, which is the only reason the
    false alarm did not become a bug report.

## BLOCKED ON

Nothing this unit can act on. FOUND 14 is closed and the read-back is written
and green under the shim (FOUND 20–21), so the §5 blocker is gone as a
*measurement* problem. What remains is a genuine upstream change, and it is now
one line rather than an open question: qtdeclarative should install
`qQuickAccessibleFactory` from `Q_COREAPP_STARTUP_FUNCTION` instead of
`Q_CONSTRUCTOR_FUNCTION`, measured working as repro mode D. Failing that,
quickshell should stop destroying its `QCoreApplication`
(`src/launch/launch.cpp:282`). **Neither is APEX's code and neither is this
unit's to write.** Somebody should file it; nothing in this repository routes
around it, and until it lands a real screen reader still gets one node from the
shell on a real machine.

**HANDED ON, not mine:** FOUND 22, the `LockedHintService._failed()` path that
never re-pumps. It is a live-machine defect in the lock-state chain, it belongs
with whoever owns **P0-015**, and it is deliberately NOT fixed on
`task/p2-b-round30` — this round only worked around it in the new suite and
wrote down why.

Queue items 1–2 still need hardware.

## Ledger (per sub-feature; nothing green without a named suite + mutation pair)

**P2-003.** screen reader: the LOGIN screen is MEASURED over real AT-SPI on 3
machines — `test-apex-greet-atspi.sh` (31, 8/8 mutants),
`test-apex-greet-session-bus.sh` (37, 13/13) — and still works after round 29's
`atspi.sh` fix. **The DESKTOP and the LOCK screen are not: quickshell publishes
no windows to the accessibility bus at all (FOUND 14), so every `Accessible.*`
in apex-shell is source-correct and runtime-unreachable.** orca as the `greetd`
user: still unproven, needs hardware. magnifier / sticky-slow-mouse keys /
on-screen keyboard: NOT PRESENT. high contrast + colour filters: Hyprland-only
shaders, not an a11y feature. reduced motion: MEASURED and ratcheted —
`check-reduce-motion.sh` (12), 39 of 450 durations (8.7%). large text: only
global `Metrics.scale`; 7px floor guarded by `check-scale-tokens.sh`.
keyboard-only installer: MEASURED — `installer/test-installer-a11y.sh`, rings
walked on FIVE pages on a private Xvfb with real `xdotool` keys read back over
AT-SPI. keyboard-only desktop + screen-reader markup: source-level DONE for the
shared `Cfg*` controls (`run-a11y-controls-test.sh`, 21) — QML-side only, see
FOUND 14. login: DONE three ways. **lock: SOURCE-LEVEL DONE round 28
(`check-lockscreen-a11y.sh` 33/0/0 + `mutate-lockscreen-a11y.sh` 20 CAUGHT/4
HELD); RUNTIME READ-BACK ATTEMPTED AND BLOCKED round 29 —
`run-lockscreen-atspi.sh` (17/0/6) engages a real ext-session-lock on a nested
labwc, reads the tree back, and finds it empty; `mutate-lockscreen-atspi.sh`
(11 mutants, 8 CAUGHT / 0 SURVIVED, 3 HELD) proves that result is not a broken
harness. RUNTIME READ-BACK DONE round 30 UNDER THE SHIM —
`run-lockscreen-atspi-shim.sh` 23/0/1 + `mutate-lockscreen-atspi-shim.sh`
6 CAUGHT / 0 SURVIVED / 2 HELD: with Qt's factory restored in-process the
shipped markup arrives on the bus intact, so the markup is PROVEN and the only
thing left is the upstream fix. Note what that does and does not mean — on a
real machine, with no shim, a screen reader still gets one node.** the CAUSE of
that is named and pinned: `check-quickshell-a11y-cause.sh` 14/0/0 +
`mutate-quickshell-a11y-cause.sh` 11 CAUGHT / 0 SURVIVED / 2 HELD. **recovery:
DONE round 31, both halves.** Source-level: `check-recovery-a11y.sh` 34/0/0 +
`mutate-recovery-a11y.sh` 17 applied / 14 CAUGHT / 0 SURVIVED / 3 HELD, the same
on this laptop and on the Arch runner. Runtime
read-back AND drive: `run-recovery-atspi-shim.sh` **33/0/1** +
`mutate-recovery-atspi-shim.sh` **15 applied / 12 CAUGHT / 0 SURVIVED /
0 MISSCORED / 0 UNSCORABLE / 3 HELD / 0 FALSE-RED** — the whole destructive
flow walked with nothing but DoAction, against an `apex` stub whose argv log
makes both directions checkable. 1 node before the factory install, 90 after.
Same caveat as the lock screen: the factory is put back by a test-only
LD_PRELOAD and on a real machine a screen reader still gets one node.

**P2-004.** layout before password: DONE, greeter and installer
(`test-apex-greet-layout.sh` 25, `test-installer-keymap.sh` 43,
`test-installer-locale.sh` 26). timezone: DONE (installer). multiple layouts:
greeter DONE; nothing in the shell switches layouts. IME/fcitx5: present in
image, NO suite asserts any of it. CJK: MEASURED (`run-i18n-test.sh` §5). RTL:
shared settings surface mirrors and is mutation-proved (`run-rtl-test.sh`
30/0/0 here, `mutate-rtl.sh` 13/13 — **CLOSED on this laptop, do not re-run**);
**0 of 14 window roots mirror** — the honest remaining half. Mechanism guarded
from apex-os by `test-apex-platform-theme.sh`. **`run-rtl-test.sh` is red on the GitHub
Arch runner and has been since it landed — now 19 passed / 3 failed / 3 skipped
(was 17/3/2; round 31 added two passing assertions and one COULD-NOT-RUN, and
the same three rows are still red).** Not any branch's doing — control run on
`roadmap/v2.2` (35351740747), and red again on `roadmap/v2.2`-derived branches
in 35352450127, 35353034100, 35382910849 and 35415236747. **THE CAUSE IS NO
LONGER UNKNOWN: see FOUND 26.** It is `libKF6I18n.so.6`, which Fedora's qt6ct
build links against and Arch's upstream `qt6ct 0.11-8` does not; that library
is what loads `qt_ar.qm` and flips the layout direction. CONFIRMED FROM THE
RUNNER in 35388802498 and again in 35415236747, where the iff PASSED in the
opposite branch to the one it passes in here and said so in words. **Both
candidates this row used to list are ELIMINATED BY MEASUREMENT**, not by
argument: the conf, because the ar_EG run with `XDG_CONFIG_DIRS` and
`XDG_CONFIG_HOME` both pointed at an empty directory still gives RightToLeft;
and the Qt version, because it is no longer needed as an explanation and the
runner has 64 `qt_*.qm` including `qt_ar.qm`. So the runner's three reds are
TRUTHFUL ABOUT THAT MACHINE. What is open is a DECISION — leave it red, or make
the runner match what APEX ships (install `ki18n`, preload it for that step
only, a route already measured to work) — and it is written up as NEXT item 1.
**Do not close it by making section 1 SKIP** — that section IS the
discriminator. locale: deliberately NOT
offered — image ships `glibc-langpack-en` only. translated shell: pipeline
proven on two machines, blocker is the host (FOUND 3). translated installer: not
present, route is gettext. per-user language: not present. recovery flow:
untouched.

## Standing queue (ordered; 1–2 need hardware, not mine)

1. Orca at the login screen on real hardware; 2. greeter audio;
3. ~~the quickshell accessibility defect (FOUND 14)~~ **DIAGNOSED AND CLOSED
   round 30 (FOUND 20–21). What is left is not investigation: it is filing one
   upstream change and waiting for it. See BLOCKED ON;**
4. the QTranslator host change (FOUND 3) — costs a compiled artefact;
5. the ~200 prose strings in `AgentHelpContent.qml`, PARKED until 4 (and
   `check-agent-help.sh` greps the exact shape `{ k: "kv", t: "$m"`, so it must
   move with them);
6. ~~**`run-rtl-test.sh` section 1 is RED on the GitHub Arch runner and the
   cause is unknown**~~ **CAUSE IDENTIFIED round 31 (FOUND 26) and confirmed
   from the runner itself: Arch's upstream qt6ct 0.11-8 links no libKF6I18n,
   which is the library that loads the Qt catalogue and flips the direction.
   The step is 19/3/3 there and the three reds are truthful about that machine.
   What remains is a DECISION, not an investigation — see NEXT item 1. Still
   older than any current branch, and still must NOT be closed by making
   section 1 SKIP;**
7. **RTL layout gap: 0 `LayoutMirroring` / 0 `layoutDirection` among the 14
   window roots**;
8. ~~the RECOVERY screen, the last unaudited a11y surface named in the P2-003
   row~~ **DONE round 31, source-level and over the bus. It also turned up
   FOUND 28, a product defect that made the factory reset uncompletable by
   anybody; fixed in apex-shell `5327bc6`;**
9. installer locale picker, needs langpacks in `Containerfile.core` first;
10. the installer's gettext route;
11. CI does NOT cover `run-i18n-test.sh` §4 or §5's non-CJK rows (they SKIP on
    the Arch runner);
12. **`src/popups/` (18 files) and `src/nexus/NavPane.qml` are bespoke
    Rectangle+MouseArea, mouse-only and unnamed** — though note FOUND 14 makes
    this moot at runtime until item 3 is settled;
13. a SKIP in an installer suite means UNMEASURED, not met.

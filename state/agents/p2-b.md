# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

items: P2-003, P2-004
repo: both
worktree: /var/tmp/apex-work/wt-p2-b4
branch: task/p2-b-round29
second_worktree: /var/tmp/apex-work/wt-p2-b4-sh
second_branch: task/p2-b-round29
scratch: /var/tmp/apex-work/scratch-p2-b/round29/

The branch name moved from `task/p2-b-round23` to `task/p2-b-round29` this
round, and it must exist and be pushed in BOTH repos even when a repo gets no
commits, because apex-shell's CI looks for a matching branch name. Full
pre-prune cards: `scratch-p2-b/p2-b.card.pre-round27-prune.md` and
`scratch-p2-b/p2-b.card.pre-round28.md`.

## NEXT

**Find out why `QQuickWindow::accessibleRoot()` is null inside quickshell.**
FOUND 14 is the largest thing this unit has measured and it currently ends in
"upstream". It should not: one experiment closes it.

Everything is ruled out but one branch. The windows are top-level, are
`Qt::Window`, and `QAccessible::isActive()` is 1 — all measured in gdb, both
processes. Qt's `queryAccessibleInterface` walks the metaobject chain calling
every installed factory, and `qQuickAccessibleFactory` (qtdeclarative
`src/quick/util/qquickglobal.cpp`, installed by the `Q_CONSTRUCTOR_FUNCTION`
`QQuick_initializeModule`) is the one that answers for `"QQuickWindow"`. If it
answered, `accessibleRoot()` could not be null. So either that factory is not
in `qAccessibleFactories()` in this process, or the loop returns early on a
class name above `QQuickWindow` — `queryAccessibleInterface` has a
`return result;` inside the plugin branch that returns even when `create()`
gave null.

The experiment: `qQuickAccessibleFactory` is NOT exported (checked with
`nm -D /usr/lib64/libQt6Quick.so.6`) so it cannot be called from gdb, but
`_ZN22QAccessibleQuickWindowC1EP12QQuickWindow` IS. Write a tiny LD_PRELOAD
whose constructor calls `QAccessible::installFactory` with a factory returning
`new QAccessibleQuickWindow(...)` for `"QQuickWindow"`, run quickshell under it,
and walk the tree. A tree that appears proves the factory was missing and hands
upstream a one-line diagnosis; a tree that does not means the plugin branch is
swallowing it, which is also a one-line diagnosis. Neither result changes any
shipped file — this is diagnosis, not a fix.

After that, and only after it, the §5 read-back assertions in
`tests/run-lockscreen-atspi.sh` become writable. They are listed there already.

## IN PROGRESS

Nothing. Both worktrees clean, both branches pushed.

## DONE

Round 29 (2026-09-18).

apex-shell `task/p2-b-round29`, four commits on `roadmap/v2.2`'s `379eef8`,
pushed, NOT yet merged: `d710bfb` the AT-SPI read-back suite and the ported
harness, `f66db73` the mutation pair and the CI wiring, `c81c421` the
provenance self-check for the two copied files, `e65d378` the harness guard the
Arch runner made necessary (FOUND 18) plus the at-spi path list (FOUND 19).

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
  **8 red CAUGHT / 0 SURVIVED / 0 MISSCORED, 3 green HELD / 0 FALSE-RED.**
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
harness, not an environment problem: see FOUND 18. Both are fixed in `e65d378`
and re-dispatched as 35362877551. Note what the runner still cannot do: it has
no `quickshell` (AUR), so §2–§5 of the read-back suite cannot run there and the
mutation harness now says NOTHING WAS MEASURED and exits 0. The §1 control
should run there once at-spi resolves; that is the thing to read off the
re-dispatch.

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

## BLOCKED ON

The §5 read-back assertions are blocked on FOUND 14, which is upstream of this
repository. The NEXT above is the diagnosis that would unblock it, and it costs
one LD_PRELOAD and no shipped file. Queue items 1–2 need hardware.

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
harness.** recovery: NOT done.

**P2-004.** layout before password: DONE, greeter and installer
(`test-apex-greet-layout.sh` 25, `test-installer-keymap.sh` 43,
`test-installer-locale.sh` 26). timezone: DONE (installer). multiple layouts:
greeter DONE; nothing in the shell switches layouts. IME/fcitx5: present in
image, NO suite asserts any of it. CJK: MEASURED (`run-i18n-test.sh` §5). RTL:
shared settings surface mirrors and is mutation-proved (`run-rtl-test.sh`
30/0/0 here, `mutate-rtl.sh` 13/13 — **CLOSED on this laptop, do not re-run**);
**0 of 14 window roots mirror** — the honest remaining half. Mechanism guarded
from apex-os by `test-apex-platform-theme.sh`. **`run-rtl-test.sh` is 17/3/2 on
the GitHub Arch runner and has been since it landed**, proven not to be any
branch's doing by a control run on `roadmap/v2.2` (35351740747). `qt6ct` was
genuinely absent and the step now installs it; `qt6-translations` was ALREADY
present at 6.11.2 — still 17/3/2 (35352450127, 35353034100), so those two were
necessary and not sufficient and the cause is NOT identified. Unruled-out
candidates: the runner is Qt 6.11.2 against this laptop's 6.10.3, and its qt6ct
has no `/etc/xdg/qt6ct/qt6ct.conf`, which APEX ships. **Do not close it by making
section 1 SKIP** — that section IS the discriminator. locale: deliberately NOT
offered — image ships `glibc-langpack-en` only. translated shell: pipeline
proven on two machines, blocker is the host (FOUND 3). translated installer: not
present, route is gettext. per-user language: not present. recovery flow:
untouched.

## Standing queue (ordered; 1–2 need hardware, not mine)

1. Orca at the login screen on real hardware; 2. greeter audio;
3. **the quickshell accessibility defect (FOUND 14) — the `## NEXT` above. It
   gates every a11y item in the product and nothing in this repo can route
   around it;**
4. the QTranslator host change (FOUND 3) — costs a compiled artefact;
5. the ~200 prose strings in `AgentHelpContent.qml`, PARKED until 4 (and
   `check-agent-help.sh` greps the exact shape `{ k: "kv", t: "$m"`, so it must
   move with them);
6. **`run-rtl-test.sh` section 1 is RED on the GitHub Arch runner and the cause
   is unknown** — see the P2-004 ledger row; it is older than any current branch;
7. **RTL layout gap: 0 `LayoutMirroring` / 0 `layoutDirection` among the 14
   window roots**;
8. the RECOVERY screen, the last unaudited a11y surface named in the P2-003 row;
9. installer locale picker, needs langpacks in `Containerfile.core` first;
10. the installer's gettext route;
11. CI does NOT cover `run-i18n-test.sh` §4 or §5's non-CJK rows (they SKIP on
    the Arch runner);
12. **`src/popups/` (18 files) and `src/nexus/NavPane.qml` are bespoke
    Rectangle+MouseArea, mouse-only and unnamed** — though note FOUND 14 makes
    this moot at runtime until item 3 is settled;
13. a SKIP in an installer suite means UNMEASURED, not met.

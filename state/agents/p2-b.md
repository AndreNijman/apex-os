# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

items: P2-003, P2-004
repo: both
worktree: /var/tmp/apex-work/wt-p2-b4
branch: task/p2-b-round23
second_worktree: /var/tmp/apex-work/wt-p2-b4-sh
second_branch: task/p2-b-round23
scratch: /var/tmp/apex-work/scratch-p2-b/

Branch name is from round 23 and is CONTINUED, not renamed: it must exist and be
pushed in BOTH repos even when a repo gets no commits, because apex-shell's
CI looks for a matching branch name. Full pre-prune cards:
`scratch-p2-b/p2-b.card.pre-round27-prune.md` (28 KB, round-26 narrative and
mutant tables) and `scratch-p2-b/p2-b.card.pre-round28.md`.

## NEXT

Read the LOCK SURFACE back over real AT-SPI, which round 28 measured to be
possible for the first time. `tests/run-lockscreen-test.sh`: bring up a private
headless labwc through `tests/lib/headless.sh` (labwc 0.9.6 on this box
advertises `ext_session_lock_manager_v1` — checked with `strings`, so it CAN
host a `WlSessionLockSurface`), start `quickshell -p shell.qml` inside it the
way `tests/run-popup-smoke.sh` already does, exec `at-spi-bus-launcher` into a
private session bus (never D-Bus activation — FOUND 7), flip the lock through
the IPC entry point at `src/state/IpcManager.qml:542`, and read the surface's
tree back: the field's name, role and `passwordEdit`, the status line's live
name, and the two icons' absence from it.

**Name and clear the guard BEFORE flipping the lock.** `LockedHintService`
calls `loginctl`, and `loginctl set-locked-hint` on the SYSTEM bus can reach
Andre's live session — apex-agentd polls that property and holds Remote Control
sessions and revokes root grants off it. The headless run on 2026-09-18 logged
`LockedHintService: loginctl show-user failed`, which SUGGESTS the private
XDG_RUNTIME_DIR cuts it off, but that is one log line and not a measurement:
read what `src/services/system/LockedHintService.qml` execs, and assert the
refusal, before locking anything.

## IN PROGRESS

Nothing. Both worktrees clean, both branches pushed, both repos merged up to
`roadmap/v2.2` (apex-os `266dcc57`, apex-shell `b7953e4`).

## DONE

Round 28 (2026-09-18), apex-shell `task/p2-b-round23` `b7953e4 → a869a7d`,
pushed, NOT yet merged. apex-os got the merge only — no commits this round.

`src/windows/Lockscreen.qml` had zero `Accessible.` anything across 440 lines
and now carries the markup, its suite (`tests/check-lockscreen-a11y.sh`, 33/0/0)
and its mutation pair (`tests/mutate-lockscreen-a11y.sh`, 24 mutants: 20 red
CAUGHT / 0 SURVIVED / 0 MISSCORED, 4 green HELD / 0 FALSE-RED), both wired into
`.github/workflows/ci.yml` and all three files added to the structure-check
REQUIRED list. Six commits: `0b6845f` the suite landed red first, `3abfb32` the
markup plus the two extractor defects running it exposed, `e38eb9b` the
comments-blanked value copy, `2239484` the mutation pair and the CI wiring,
`170890c` the git-free restore, `a869a7d` the two things only the runner said.

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
6. **A mutation harness can misscore.** Verdicts are FOUR-way as of round 28 —
   CAUGHT / MISSCORED / **CRASHED** / SURVIVED — and the harness must be shown
   capable of all four. CRASHED exists because a mutant that makes the suite
   exit FATAL prints no totals line, and a harness that only counts failures
   reads that as "nothing failed": a working suite reported as a broken one.
7. **Never rely on D-Bus activation for the a11y bus on APEX.** `org.a11y.Registry`
   activation fails `Permission denied` — SELinux (`gnome_atspi_exec_t`), no AVC
   logged (dontaudit). Exec `at-spi-bus-launcher` directly; `tests/lib/atspi.sh` does.
8. **No `lupdate`/`lrelease` on this box** (`qt6-qttools` absent). `qmltestrunner-qt6`,
   `quickshell`, `labwc`, `sway`, `cage`, `Xvfb`, `xdotool` are present.
   `qmllint-qt6` is too, and it RESOLVES Quickshell's types — it warned about
   unqualified access INSIDE the `WlSessionLockSurface` body — so it is a real
   type check on shell QML and not merely a parse.
9. **A new suite is not a gate until it is wired into a workflow.** In apex-os
   that is `.github/workflows/pr-validation.yml` and
   `tests/check-suites-run-in-ci.sh` enforces it — but that checker globs
   `tests/test-*.sh` **in apex-os only**, so it can neither see nor protect an
   apex-shell suite. apex-shell's workflow is `.github/workflows/ci.yml`, one
   big `arch-validate` job on `archlinux:latest`, plus a `structure-check` job
   with a REQUIRED file list new suites belong in. Every suite exits 0 on a
   SKIP: **read the totals line, never the tick.**
10. **Restore mutants authoritatively, and prove the restore before using it.**
    `git checkout --` is what the two older harnesses use and it CANNOT work in
    apex-shell CI: `arch-validate` runs Checkout BEFORE it installs git, so
    `actions/checkout` falls back to the tarball API and the workspace has no
    `.git` at all. `mutate-lockscreen-a11y.sh` restores from a pristine copy in
    a per-run `mktemp -d` (never a shared scratch path — that once put back
    another agent's file) and VERIFIES sha256 against a baseline taken before
    anything was touched.
11. **Design fork, settled round 2, option (b):** an installer-picked keyboard
    layout reaches the running session only by re-execing cage with
    `XKB_DEFAULT_LAYOUT`. There is no xkb layout in the APEX input model.
12. **A description is not speech.** A screen reader does not generally speak a
    `description-changed` event on the object that already holds focus, so
    `Accessible.description` puts state in the tree without anyone hearing it.
    `Accessible.announce(message, politeness)` is the fix; it is Qt **6.8+**
    (`revision: 1544` in `plugins.qmltypes`) and the image ships **6.10.3**,
    measured with a runtime probe, not assumed. It is a no-op while
    accessibility is inactive.
13. **labwc 0.9.6 advertises `ext_session_lock_manager_v1`**, so the nested
    headless harness CAN host the lock surface. This is what makes the NEXT
    above possible; every earlier round recorded the runtime half as needing a
    compositor we did not have.

## BLOCKED ON

Nothing workable. Queue items 1–2 need hardware and are not mine to fake.

## Ledger (per sub-feature; nothing green without a named suite + mutation pair)

**P2-003.** screen reader: MEASURED over real AT-SPI on 3 machines —
`test-apex-greet-atspi.sh` (31, 8/8), `test-apex-greet-session-bus.sh` (37,
13/13); still unproven: orca as the `greetd` user, needs hardware. magnifier /
sticky-slow-mouse keys / on-screen keyboard: NOT PRESENT. high contrast +
colour filters: present as Hyprland-only shaders, not an a11y feature. reduced
motion: MEASURED and ratcheted — `check-reduce-motion.sh` (12), reaches 39 of
450 durations (8.7%), 402 are bare int literals. large text: only global
`Metrics.scale`; 7px floor guarded by `check-scale-tokens.sh`. keyboard-only
installer: MEASURED — `installer/test-installer-a11y.sh`, rings walked on FIVE
pages on a private Xvfb with real `xdotool` keys read back over AT-SPI;
`disk`/`mode`/`part`/`run`/`done` unaudited by design. keyboard-only desktop +
screen-reader markup: DONE for the shared `Cfg*` controls only
(`run-a11y-controls-test.sh`, 21). login: DONE three ways. **lock: SOURCE-LEVEL
DONE round 28** — `check-lockscreen-a11y.sh` (33/0/0) + `mutate-lockscreen-a11y.sh`
(20 red CAUGHT, 4 green HELD), both in CI; the runtime read-back of the lock
surface over AT-SPI is **NOT done and is now feasible** (FOUND 13). **recovery:
NOT done.**

**P2-004.** layout before password: DONE, greeter and installer
(`test-apex-greet-layout.sh` 25, `test-installer-keymap.sh` 43,
`test-installer-locale.sh` 26). timezone: DONE (installer) — picker from
`zone1970.tab` (312 zones), pick survives the compositor restart. multiple
layouts: greeter DONE; nothing in the shell switches layouts. IME/fcitx5:
present in image, NO suite asserts any of it. CJK: MEASURED (`run-i18n-test.sh`
§5). RTL: shared settings surface mirrors and is mutation-proved
(`run-rtl-test.sh` 30/0/0 here, `mutate-rtl.sh` 13/13 — **CLOSED on this
laptop, do not re-run**); **0 of 14 window roots mirror** — the honest remaining
half. Mechanism guarded from apex-os by `test-apex-platform-theme.sh`.
**NEW round 28: `run-rtl-test.sh` is 17/3/2 on the GitHub Arch runner and has
been since it landed**, proven not to be any branch's doing by a control run on
`roadmap/v2.2` (35351740747) with identical totals. Section 1 cannot produce an
RTL application direction there. `qt6ct` was genuinely absent and the step now
installs it; `qt6-translations` was ALREADY present at 6.11.2 — and it is STILL
17/3/2 (35352450127), so those two were necessary and not sufficient and the
cause is NOT yet identified. Unruled-out candidates: the runner is Qt 6.11.2
against this laptop's 6.10.3, and its qt6ct has no `/etc/xdg/qt6ct/qt6ct.conf`,
which APEX ships. **Do not close it by making section 1 SKIP** — that section
IS the discriminator. locale: deliberately NOT offered — image ships
`glibc-langpack-en` only. translated shell: pipeline proven on two machines,
blocker is the host (FOUND 3). translated installer: not present, route is
gettext. per-user language: not present. recovery flow: untouched.

## Standing queue (ordered; 1–2 need hardware, not mine)

1. Orca at the login screen on real hardware; 2. greeter audio; 3. the
QTranslator host change (FOUND 3) — costs a compiled artefact; 4. the ~200 prose
strings in `AgentHelpContent.qml`, PARKED until 3 (and `check-agent-help.sh`
greps the exact shape `{ k: "kv", t: "$m"`, so it must move with them);
5. **`run-rtl-test.sh` section 1 is RED on the GitHub Arch runner and the cause
is unknown** — see the P2-004 ledger row; it is the one thing making
`task/p2-b-round23`'s CI red and it is older than this branch;
6. **RTL layout gap: 0 `LayoutMirroring` / 0 `layoutDirection` among the 14
window roots**; 7. **the lock surface read back over AT-SPI — the `## NEXT`
above**; 7. the RECOVERY screen, the last unaudited a11y surface named in the
P2-003 row; 8. installer locale picker, needs langpacks in `Containerfile.core`
first; 9. the installer's gettext route; 10. CI does NOT cover
`run-i18n-test.sh` §4 or §5's non-CJK rows (they SKIP on the Arch runner);
11. **`src/popups/` (18 files) and `src/nexus/NavPane.qml` are bespoke
Rectangle+MouseArea, mouse-only and unnamed — only 8 of ~50 MouseArea files
carry any `Accessible.*`, and all 8 are `components/config/`**; 12. a SKIP in an
installer suite means UNMEASURED, not met.

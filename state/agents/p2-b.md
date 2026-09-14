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
`pr-validation.yml` "Input page and generator agree" step looks for a matching
branch name. Full pre-prune card (28 KB, round-26 narrative and mutant tables):
`/var/tmp/apex-work/scratch-p2-b/p2-b.card.pre-round27-prune.md`.

## NEXT

In `/var/tmp/apex-work/wt-p2-b4-sh`, give `src/windows/Lockscreen.qml` the
`Accessible.role`/`.name`/`.description` markup its password `TextInput` (line
~315) and its buttons have none of, then add `tests/check-lockscreen-a11y.sh`
plus a both-direction mutant file, and wire the suite into `pr-validation.yml`.

## IN PROGRESS

Nothing. Both worktrees clean, both branches pushed.

## DONE

Round 26 landed as apex-os merge `8968135b` (578914fd, b1db5eb0, c9db7fcf,
77bbd712): `qt6-qttranslations` named explicitly in `Containerfile.core`,
`tests/test-apex-platform-theme.sh` (18/0/0 under `env -i`) + its CI step, a
prefix-match hole the harness found, and `tests/mutate-platform-theme.sh`
(13 mutants: 10/10 red CAUGHT, 0 SURVIVED, 0 MISSCORED; 3/3 green HELD).

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
6. **A mutation harness can misscore.** Verdicts are three-way (CAUGHT /
   SURVIVED / MISSCORED) and the harness must be shown capable of all three.
7. **Never rely on D-Bus activation for the a11y bus on APEX.** `org.a11y.Registry`
   activation fails `Permission denied` — SELinux (`gnome_atspi_exec_t`), no AVC
   logged (dontaudit). Exec `at-spi-bus-launcher` directly; `tests/lib/atspi.sh` does.
8. **No `lupdate`/`lrelease` on this box** (`qt6-qttools` absent). `qmltestrunner-qt6`,
   `quickshell`, `labwc`, `sway`, `cage`, `Xvfb`, `xdotool` are present.
9. **A new suite is not a gate until wired into `.github/workflows/pr-validation.yml`**
   (`tests/check-suites-run-in-ci.sh` enforces). Run `check-shellcheck-coverage.sh`
   and `check-containerfile-assertions.sh` too. Every suite exits 0 on a SKIP:
   **read the totals line, never the tick.**
10. **Restore mutants with `git checkout`, never `cp` from a backup dir.**
11. **Design fork, settled round 2, option (b):** an installer-picked keyboard
    layout reaches the running session only by re-execing cage with
    `XKB_DEFAULT_LAYOUT`. There is no xkb layout in the APEX input model.

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
pages (keyboard, **wifi**, secureboot, confirm, account) on a private Xvfb with
real `xdotool` keys read back over AT-SPI; `disk`/`mode`/`part`/`run`/`done`
unaudited by design. keyboard-only desktop + screen-reader markup: DONE for the
shared `Cfg*` controls only (`run-a11y-controls-test.sh`, 21). login: DONE three
ways. **lock and recovery: NOT done.**

**P2-004.** layout before password: DONE, greeter and installer
(`test-apex-greet-layout.sh` 25, `test-installer-keymap.sh` 43,
`test-installer-locale.sh` 26). timezone: DONE (installer) — picker from
`zone1970.tab` (312 zones), pick survives the compositor restart. multiple
layouts: greeter DONE; nothing in the shell switches layouts. IME/fcitx5:
present in image, NO suite asserts any of it. CJK: MEASURED (`run-i18n-test.sh`
§5). RTL: shared settings surface mirrors and is mutation-proved
(`run-rtl-test.sh` 30/0/0, `mutate-rtl.sh` 13/13 — **CLOSED, do not re-run**);
**0 of 14 window roots mirror** — the honest remaining half. Mechanism guarded
from apex-os by `test-apex-platform-theme.sh`. locale: deliberately NOT offered
— image ships `glibc-langpack-en` only, so a picker would silently degrade to
`C.UTF-8`. translated shell: pipeline proven on two machines, blocker is the
host (FOUND 3). translated installer: not present, and its route is gettext.
per-user language: not present. recovery flow: untouched.

## Standing queue (ordered; 1–2 need hardware, not mine)

1. Orca at the login screen on real hardware; 2. greeter audio; 3. the
QTranslator host change (FOUND 3) — costs a compiled artefact; 4. the ~200 prose
strings in `AgentHelpContent.qml`, PARKED until 3 (and `check-agent-help.sh`
greps the exact shape `{ k: "kv", t: "$m"`, so it must move with them);
5. **RTL layout gap: 0 `LayoutMirroring` / 0 `layoutDirection` among the 14
window roots**; 6. ~~wifi ring~~ **DONE, landed 2026-09-13** (`2eff50b8`,
`719513fa`, `5d070093` — every Wi-Fi network was an unnamed Tab stop);
7. installer locale picker, needs langpacks in `Containerfile.core` first;
8. the installer's gettext route; 9. CI does NOT cover `run-i18n-test.sh` §4 or
§5's non-CJK rows (they SKIP on the Arch runner); 10. **`src/popups/` (18 files)
and `src/nexus/NavPane.qml` are bespoke Rectangle+MouseArea, mouse-only and
unnamed — only 8 of ~50 MouseArea files carry any `Accessible.*`, and all 8 are
`components/config/`**; 11. a SKIP in an installer suite means UNMEASURED, not met.

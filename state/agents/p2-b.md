# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

**ROUND 26 is live. Branch `task/p2-b-round23` in BOTH repos** (the name is from
round 23 and has been continued, not renamed — both must exist and be pushed even
if a repo gets no commits, because apex-shell's `pr-validation.yml` "Input page
and generator agree" step looks for a matching branch name).

- apex-os:    worktree `/var/tmp/apex-work/wt-p2-b4`
- apex-shell: worktree `/var/tmp/apex-work/wt-p2-b4-sh`
- scratchpad: `/var/tmp/apex-work/scratch-p2-b/` (mine alone; never the shared one)

**Round 26 state.** apex-shell's round-25 work LANDED (`roadmap/v2.2` a0b3deb ->
`acf4879`); that worktree is clean and has nothing outstanding. apex-os was
fast-forwarded onto `origin/roadmap/v2.2` = `2219ef75` at the start of this round
and carries this round's commits on top.

**This card was 112 KB on 2026-09-14 and was pruned to conclusions.** The
pre-prune copy is `/var/tmp/apex-work/scratch-p2-b/p2-b.card.pre-round26-prune.md`
— nothing below is invented, it is the same substance at a tenth the size. Two
standing cautions from the old card, kept because both cost real time:
**anchor a splice on a heading, never on a phrase that also appears in prose**
(a `str.index` on section text truncated this file once), and **line numbers in
this card go stale** — re-locate before trusting any `file:NNN`.

## The items

- **P2-003** — "Screen reader/magnifier/high-contrast/reduced-motion/keyboard-only
  installer validated". ROADMAP.md §29 expands it to twelve sub-features.
- **P2-004** — "Keyboard layout before password, IME/CJK/RTL/locales/timezones
  supported". ROADMAP.md §30 expands it to ten.

Twenty-two sub-features across two repos. The unit closes on named suite
assertions that run and count, so the deliverable is the **ledger** below: per
sub-feature, measured-green / measured-red / not-measurable-headless /
not-present. Rows that cannot be honestly measured here are named, not faked.

## Harness facts established before writing anything

- `qmltestrunner-qt6`, `quickshell`, `labwc`, `sway`, `cage` are present.
- **No `lupdate`/`lrelease` on this box** (`qt6-qttools` is not in the image).
  `/usr/lib64/qt6/bin/{qml,qmldom,qmllint}` are present.
- `at-spi2-core` 2.58.8 is installed; `pyatspi` is not — an AT-SPI walk goes over
  D-Bus with `gdbus`, not Python bindings.
- **Never rely on D-Bus activation to start the a11y bus on APEX.** Activation of
  `org.a11y.Registry` fails with `Permission denied`, and it is **SELinux**, not
  at-spi: a byte-identical copy of `at-spi-bus-launcher` in `/tmp` (`user_tmp_t`)
  activates, the original (`gnome_atspi_exec_t`) does not, and no AVC is logged
  (`dontaudit`). Exec the launcher directly. `tests/lib/atspi.sh` does.
- Two runnable shapes in apex-shell: `run-settings-controls-test.sh` stages real
  components beside a `Theme` stub for `qmltestrunner -platform offscreen`;
  `tests/lib/headless.sh` gives a private labwc for anything needing a real
  `quickshell` process (`tests/check-headless-runners.sh` enforces the second).
- apex-os CI is `.github/workflows/pr-validation.yml` (there is no `ci.yml` any
  more). `tests/check-suites-run-in-ci.sh` fails a suite that is in no workflow
  and not in `tests/suites-not-in-ci.txt` — **a new suite is not a gate until it
  is wired in.** `tests/check-shellcheck-coverage.sh` and
  `tests/check-containerfile-assertions.sh` are the other two gates to run
  before believing a change.
- `xorg-x11-server-Xvfb` + `xdotool` are what make the installer criterion
  measurable; both are on this laptop and in CI's installer job.
- `files/system/libexec/apex-input-apply` models libinput only. **There is no xkb
  layout in the APEX input model** — the layout route is `XKB_DEFAULT_*`.

## Findings that must not be re-derived

1. **The RTL switch is driven by a TRANSLATION CATALOGUE, not by the locale**
   (round 23, the unit's biggest finding). `Qt.application.layoutDirection`
   follows whether a Qt catalogue for that language is loaded — Qt translates the
   string `QT_LAYOUT_DIRECTION` and compares to `RTL`. Measured with a negative
   control under `env -i`: `ar/he/fa` + `QT_QPA_PLATFORMTHEME=qt6ct` give
   APEXDIR=1; **`ur_PK` gives APEXDIR=0 with TEXTDIR=1** because there is no
   `qt_ur.qm`, and `ar` with the theme unset or bogus gives 0 too. So mirroring
   on a shipped APEX desktop works **entirely as a side effect of a theming
   choice made for a dark palette**. Round 26 guards that; see below.
2. **A QTranslator needs a HOST change and no QML can do it** (round 20, measured
   not grepped). QTranslator is C++ and is not a QML type; `/usr/bin/quickshell`
   calls neither it nor `installTranslator` and constructs a bare `QQmlEngine`,
   never the `QQmlApplicationEngine` whose `_q_loadTranslations` loads a `.qm`
   for free. Round 23 refined it: the process DOES get a translator — from the
   platform theme. What quickshell lacks is a route to load OUR catalogue.
3. **The CJK "will tofu" claim was false** and sat in the ledger for 19 rounds on
   an argument nobody ran. ~45 sites hardcode JetBrains Mono, which has no CJK
   coverage — and `QFontEngineMulti` asks fontconfig for a font that owns the
   glyph. Measured on two machines. Same for Arabic/Hebrew/Thai/Devanagari:
   **RTL is a LAYOUT gap, not a font gap.**
4. **The shipped greeter had no bus to publish on** (round 18) — greetd ran sway
   directly, no `dbus-run-session` anywhere, so a perfect a11y tree published
   zero nodes. Closed in round 19 by `/usr/libexec/apex-greet-session`.
5. **An assertion whose truth is supplied by the ambient environment is the same
   defect class as a gate that inspects nothing.** Round 22's RTL suite was
   13/0/0 on this laptop only because it inherited `QT_QPA_PLATFORMTHEME`; the
   mutation harness runs under `env -i HOME PATH USER TMPDIR` and the baseline
   failed 3 of 15. The harness was right and the suite was wrong.
6. **A mutation harness can misscore.** Round 25 found a mutant the suite CAUGHT
   scored SURVIVED for the sixth consecutive round. Every verdict is now
   three-way (CAUGHT / SURVIVED / MISSCORED) and the harness is itself proved
   capable of printing all three.
7. **`shellcheck` without `-x` failed a sourcing suite and switched off the forty
   suites below it**; the `if: ${{ !cancelled() }}` that stops a red step hiding
   its successors is part of the same fix (landed, apex-shell `4e1ec25`).
8. **CI is a second environment and finds things this laptop cannot.** The
   `Australia/Perth` timezone fallback guard `case "$tz" in ""|"UTC")` missed
   `Etc/UTC`, which is what systemd canonicalises to on a runner.
9. **Restore mutants with `git checkout`, not `cp` from a backup dir.** A `cp`
   restore once left `apex-installer-session` holding the launcher's contents
   (369 lines for 171) and it was caught by an editor notice, not the harness.
10. **Design fork, settled round 2, option (b):** a keyboard layout picked in the
    installer reaches the RUNNING session only by re-execing cage with
    `XKB_DEFAULT_LAYOUT`. The other two routes were ruled out by measurement.

## Suites this unit owns, and their standing verdicts

| repo | suite | totals | mutants |
| --- | --- | --- | --- |
| apex-os | `tests/test-apex-greet-atspi.sh` | 31 assertions, green on 3 machines incl. the GitHub runner | 8/8 caught |
| apex-os | `tests/test-apex-greet-session-bus.sh` | 37 assertions | 13/13 caught |
| apex-os | `tests/test-apex-greet-a11y.sh` | 22 assertions | in the round-18 set |
| apex-os | `tests/test-apex-greet-layout.sh` | 25 (23 + 1 skip off-host) | — |
| apex-os | `tests/test-apex-platform-theme.sh` | **round 26 — see NEXT** | — |
| apex-os | `installer/test-installer-a11y.sh` | 68 assertions, 4 pages' Tab rings | round-18b set |
| apex-os | `installer/test-installer-keymap.sh` | 43 assertions | — |
| apex-os | `installer/test-installer-locale.sh` | 26 assertions | — |
| apex-shell | `tests/run-rtl-test.sh` | **30 passed / 0 failed / 0 skipped** under `env -i` | 13/13 caught, 0 survived, 0 misscored — **CLOSED, do not re-run** |
| apex-shell | `tests/run-i18n-test.sh` | 23 here; 15/0/5 on the Arch runner | — |
| apex-shell | `tests/run-a11y-controls-test.sh` | 21 runtime assertions | — |
| apex-shell | `tests/check-reduce-motion.sh` | 12 assertions | 3 self-tests |

Every suite here exits 0 on a tool SKIP: **read the totals line, never the tick.**

## Ledger

Filled in as each row is measured. Nothing is marked green without a named
suite and a mutation pair.

### P2-003 — accessibility

| sub-feature | state | assertion |
| --- | --- | --- |
| screen reader | **MEASURED over real AT-SPI on three machines; the production hole is CLOSED** | `test-apex-greet-atspi.sh` (31, 8/8) reads the shipped surface back over D-Bus exactly as Orca does — the password never crosses the bus, and a reader can OPERATE the session picker and layout pill with `DoAction`. `test-apex-greet-session-bus.sh` (37, 13/13) asserts greetd runs `/usr/libexec/apex-greet-session`, which starts a session bus with an empty service directory, execs `at-spi-bus-launcher` and `at-spi2-registryd`, then EXECS the compositor. SUPER+ALT+S asks for a reader on all three desktop sessions and both greeter hosts. **Still partial:** nothing has run orca AS the `greetd` user (HOME, audio, SELinux exec of `gnome_atspi_exec_t`) — needs hardware. Note: nothing sets `org.a11y.Status.ScreenReaderEnabled` until a reader connects and Qt gates on it, so a fresh greeter publishes an empty tree until the key is pressed. Correct behaviour, not a regression. |
| magnifier | not present | no magnifier in either repo; wlroots has no standard one |
| high contrast | present, untested as a11y | six shaders in apex-shell `src/config/shaders/`, applied via Hyprland `decoration:screen_shader` only — niri and labwc get nothing; shipped as a visual effect, not an a11y feature |
| reduced motion | **MEASURED and ratcheted** | `check-reduce-motion.sh` — 12 assertions. Reaches **39 of 450** animation durations (8.7%); **402 are bare int literals** no switch can touch. Counts pinned in both directions. Runtime half NOT built — `SettingsService` imports Quickshell so qmltestrunner cannot load it. |
| large text | partial by other means | only the global `Metrics.scale` (0.5–3.0); the one real a11y constant is the 7px floor in `fs()`, enforced by `check-scale-tokens.sh` |
| colour filters | **present** | shipped shaders + hyprshade protanopia/deuteranopia/tritanopia, Hyprland only |
| sticky / slow / mouse keys | not present | 0 mentions in either repo |
| on-screen keyboard | not present | no wvkbd/squeekboard/maliit anywhere |
| keyboard-only installer | **MEASURED** | `installer/test-installer-a11y.sh` — **68 assertions**. The Tab ring is walked on FOUR pages (keyboard, secureboot, confirm, account); on `confirm` the walk is two-sided and is the strongest assertion in the suite — the destructive button must NOT be in the ring while the field is empty and MUST join it once ERASE is typed with the keyboard alone. The GTK4 GUI runs on a private Xvfb, `xdotool` delivers real X key events, results are read back over AT-SPI. SIX of eleven pages are name-audited. NOT audited, and named: `disk`/`mode`/`part` enumerate real block devices, `run` starts an install, `done` follows one. |
| keyboard-only DESKTOP | **DONE for the shared controls** | `run-a11y-controls-test.sh` — 21 runtime assertions; every shared `Cfg*` control is a tab stop and operates on Space/Enter, proved with real `QKeyEvent`s. Bespoke widgets in `popups/` are NOT covered. |
| screen-reader markup, desktop | **DONE for the shared controls** | same suite: `CfgRow` hands label, description, disabled-reason and live readback to whatever control it holds. |
| accessible login/lock/recovery | **login DONE three ways; lock and recovery NOT done** | QML side, bus side and config side above. |

### P2-004 — internationalisation

| sub-feature | state | assertion |
| --- | --- | --- |
| keyboard layout before password | **DONE — greeter and installer** | Greeter: `test-apex-greet-a11y.sh` test_030-034 + `test-apex-greet-layout.sh`. Installer: `test-installer-keymap.sh` (43) and `test-installer-locale.sh` (26). Measured, not inferred — cage started with `XKB_DEFAULT_LAYOUT=de` hands a GTK4 client a keymap where physical Y gives `z`, Z gives `y`, `;` gives `ö`. |
| timezone choice | **DONE (installer)** | Engine: an explicit timezone becomes `/etc/localtime`; an explicit UTC is not overruled by the Perth fallback, while an absent choice still keeps it. GUI: a searchable picker fed from tzdata's `zone1970.tab` (312 zones), driven at runtime, and §6b proves the pick SURVIVES the compositor restart. **This row was twice written DONE while a path through it did not work** — first with no GUI widget at all, then with the pick silently discarded by the restart. Both caught in review, both fixed rather than softened. |
| multiple layouts | **greeter DONE** | `test-apex-greet-layout.sh`: both configured layouts survive extraction; a three-layout machine reports all three in order. Nothing in the desktop shell switches layouts yet. |
| IME / fcitx5 | present in image, untested | fcitx5 + chinese-addons/hangul/anthy/m17n installed in `Containerfile.core`, autostarted in 3 places. `QT_IM_MODULE`/`GTK_IM_MODULE` deliberately unset (Wayland text-input-v3). No suite asserts any of it. |
| CJK | **MEASURED, and the old claim was false** | `run-i18n-test.sh` §5, 7 assertions: 漢 advances **32** under JetBrains Mono, matching a family that covers U+6f22 and not JetBrains Mono's own **19.1875** (asserted first, because the discriminator rests on that family being monospaced). Same 32 on the GitHub Arch runner against Noto Sans CJK HK. |
| RTL | **the shared settings surface MIRRORS and is mutation-proved; the WINDOW ROOTS still do not** | apex-shell: **2 components declare mirroring** (`CfgRow`, `CfgScroll`) and **0 of 14 window roots** do — the 0 is the honest remaining half and is pinned in both directions. `run-rtl-test.sh` 30/0/0 under `env -i`; `mutate-rtl.sh` 13/13 caught. Rendering half measured through the engine: Arabic ا falls to DejaVu Sans Mono (19.265625), Hebrew א to DejaVu Sans (21.390625), Thai ก to Droid Sans Thai (19.75), Devanagari अ to Droid Sans Devanagari (24.453125) — all image-owned. The Noto families really are absent, which is a typographic quality question, not a tofu one. |
| locales / timezones | **keyboard + timezone DONE, LOCALE still not offered** | Deliberate: the image installs **`glibc-langpack-en` only**, so a free picker would let a user choose a locale that silently degrades to `C.UTF-8` on the installed system — a worse failure than not asking, because it looks like it worked. Closing the row means adding langpacks first, then a picker restricted to what the target ships. |
| translated installer | **not present** | 0 `qsTr`/gettext in `installer/`. GTK4/Python, so its route is gettext — a separate pipeline. |
| translated shell | **pipeline PROVEN on TWO machines; the blocker is the HOST** | `run-i18n-test.sh` — 23 assertions here, 15/0/5 on the Arch runner. All four links run with real tools on a shipped file: `qsTr()` marks 5 strings in `AgentHelpContent.qml`, `lupdate-qt6` extracts exactly 5 into the right context, `lrelease-qt6` compiles `apex-shell_de.ts`, and a running engine under `qmltestrunner -translation` reads the German back off the live singleton. The load-bearing assertion is the sensitivity one — the SAME fixture with and without `-translation` must DISAGREE, sampled on three strings. The gap is the host; see finding 2. |
| per-user language | not present | nothing per-user anywhere |
| non-US recovery / install flows | **install flow DONE, recovery not** | The engine now prefers an operator's `keymap`/`keyvariant`/`timezone` over every inference. `bib-config.toml` still hardcodes `us`/`en_US.UTF-8`/`Australia/Perth`, but as the FALLBACK. **Recovery is untouched** — nothing in the recovery flow asks for or applies a layout. |

## NEXT

**One line:** in `/var/tmp/apex-work/wt-p2-b4` (apex-os, `task/p2-b-round23`, now
fast-forwarded to `2219ef75`), write `tests/test-apex-platform-theme.sh` — the
three-way agreement guard that the IMAGE sets `QT_QPA_PLATFORMTHEME=qt6ct` —
then `tests/mutate-platform-theme.sh`, then wire the suite into
`pr-validation.yml` after the ai-apps step.

## IN PROGRESS

`tests/test-apex-platform-theme.sh` (apex-os, new file) — not yet written.
Nothing is committed this round yet.

## FOUND (round 26, before writing a line)

**`qt6-qttranslations` reaches the image only as a `Recommends` of
`qt6-qtbase-gui`, and is named in NEITHER Containerfile.** Measured on this
booted host, not assumed: `rpm -q --whatrequires qt6-qttranslations` → *no
package requires it*; `dnf repoquery --installed --whatrecommends
qt6-qttranslations` → `qt6-qtbase-gui`; `rpm -q --recommends qt6-qtbase-gui`
lists it. It owns `/usr/share/qt6/translations/qt_ar.qm`, which finding 1 proves
is the whole mechanism by which anything mirrors. So the RTL chain has **three**
image links and the last one is a weak dependency nobody wrote down: any build
that ever passes `--setopt=install_weak_deps=False` — the standard image-slimming
move, and `apex-pkg` already has a `--no-weak-deps` flag — silently deletes RTL
with nothing red anywhere. **This repo has been bitten by this exact class
before**: `files/system/libexec/apex-pkg:69` records that mksquashfs reaches the
image only as a weak dependency of dracut-squash.

The gap the round was dispatched for, stated honestly — it is narrower than
"nothing asserts it":

- `Containerfile.core` appends the variable to `/etc/environment` (re-locate the
  line; it was `:948`, then `:2030`, and v2.2 moves it).
- `files/desktop/labwc/environment:22` sets it again and is **unguarded outright**.
- `tests/test-apex-ai-apps.sh` greps `printf 'QT_QPA_PLATFORMTHEME` only to
  LOCATE the `/etc/environment` line, then asserts `ELECTRON_OZONE_PLATFORM_HINT`
  and `DISABLE_AUTOUPDATER` on it. **Deleting** the variable therefore does go red
  — with a message about Electron, in a suite nobody would read for a
  right-to-left regression, and only because it happens to be a grep anchor.
  **Changing its VALUE is caught by nothing at all**, and that is the change that
  silently un-mirrors the shell.
- apex-shell `run-rtl-test.sh` checks the live value but SKIPs off a booted APEX
  host, so the one repository that can break this has no guard naming the
  consequence.

**The M7 trap has three decoys, not one.** A bare `grep -q qt6ct` is satisfied by
the comment in `files/system/qt6ct/qt6ct.conf:3`, by the
`COPY files/system/qt6ct/qt6ct.conf` line in `Containerfile.base`, and by the
printf itself. The guard must extract the ASSIGNMENT and compare VALUES.

## BLOCKED ON

Nothing. The two items at the top of the standing queue need hardware and are not
mine; everything else in the queue is workable.

### The standing queue

Ordered. Items 1-2 need hardware and are **not** this agent's to attempt or fake.

1. **The reader at the login screen, end to end, on real hardware.** Everything
   around it is asserted: session bus, a11y bus and registry on both greeter
   hosts, SUPER+ALT+S bound on both, tree readable and operable over AT-SPI on
   three machines. NOT proven and not provable here: orca running as the `greetd`
   system user (writable HOME, speech-dispatcher with no audio session) and
   whether SELinux permits the greetd session context to exec
   `gnome_atspi_exec_t` (`ps -Z -C greetd` gives the domain; `sesearch -A -s
   <domain> -t gnome_atspi_exec_t -c file -p execute` answers it). If the exec is
   refused the wrapper degrades safely — that IS asserted — so the failure mode
   is a silent no-reader, not a broken login. **Needs someone who can boot an ISO
   and listen.**
2. **Greeter audio.** Falls out of item 1; nothing here configures a sound path
   for the greetd user.
3. **A QTranslator needs a HOST change** — see finding 2. Two ways out, both real
   work: upstream quickshell gaining translation support, or a QML extension
   module on the import path whose `initializeEngine()` installs a QTranslator
   (`QQmlEngine::addImportPath` is in quickshell's symbol table, so the engine
   would honour it). That is a compiled artefact in a repository of QML and shell
   scripts and it changes the image build — cost it before starting.
   `tests/run-i18n-test.sh` asserts the host facts, so if quickshell ever gains
   the call the suite says so instead of going quietly green.
4. **The `sections` array in `AgentHelpContent.qml`** (~200 prose strings) stays
   PARKED, and the reason is measured: with item 3 open, 200 more translated
   strings reach exactly as many users as the 5 that exist — none. When it
   happens, `tests/check-agent-help.sh` greps for the exact shape
   `{ k: "kv", t: "$m"`, so wrapping those `t:` values in `qsTr()` breaks it;
   that suite and the conversion must move together.
5. **RTL is a LAYOUT gap.** The half that stands is `0 LayoutMirroring` and
   `0 layoutDirection` among the 14 window roots in `src/`.
6. **`wifi` is the one audited installer page whose Tab ring is still unwalked.**
   It has two shapes and which one is built depends on the machine, so the walk
   needs the same shape detection `audit_page` already does. `disk`, `mode`,
   `part`, `run` and `done` are not audited at all, by design.
7. **Locale is still not offered by the installer, deliberately** — add langpacks
   to `Containerfile.core` first, then a picker restricted to what the target
   ships.
8. **The installer's translation route is gettext**, not the Qt pipeline proven
   here. Separate work.
9. **What CI does NOT cover for this unit**, named so it is not read as covered:
   `run-i18n-test.sh` §4 (the quickshell symbol probe) SKIPs on the Arch runner —
   one laptop only; §5's Arabic/Hebrew/Thai/Devanagari rows SKIP there too (no
   covering font; the suite says so rather than failing, and fails only where
   `/run/ostree-booted` says the font set being measured IS the image's), so only
   the CJK row has a second machine. One latent fragility: `covering()` takes
   `head -4` of the alphabetically-sorted families covering a codepoint and
   requires the fallback's advance to match one of them — that held on both
   machines by luck, not guarantee. Widen the slice or match on the resolved
   family if it ever fires.
10. **Remaining accessibility gaps:** `src/popups/` and `src/nexus/NavPane.qml`
    still use bespoke Rectangle+MouseArea and are mouse-only and unnamed. No
    magnifier, sticky keys, slow keys, mouse keys or on-screen keyboard anywhere.
    Lock and recovery screens are not measured; only login is.
11. **A SKIP in the installer suites means the criterion is unmeasured, not met.**

## DONE (this round)

Nothing yet.

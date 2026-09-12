# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

Worktrees, both on `task/p2-b-accessibility-i18n`, both pushed before any work,
neither rebased:

- apex-shell: `/var/tmp/apex-work/wt-p2-b` (off `origin/roadmap/v2.2` @ `cd324ec`)
- apex-os:    `/var/tmp/apex-work/wt-p2-b-os` (off `origin/roadmap/v2.2` @ `4f80746e`)

## The items

- **P2-003** — "Screen reader/magnifier/high-contrast/reduced-motion/keyboard-only
  installer validated". ROADMAP.md §29 expands it to twelve sub-features.
- **P2-004** — "Keyboard layout before password, IME/CJK/RTL/locales/timezones
  supported". ROADMAP.md §30 expands it to ten.

Twenty-two sub-features across two repos. The unit closes on named suite
assertions that run and count, so the deliverable is a **ledger**: per
sub-feature, measured-green / measured-red / not-measurable-headless /
not-present. Rows that cannot be honestly measured here are named, not faked.

## Harness facts established before writing anything

- `qmltestrunner-qt6` is present; `quickshell`, `labwc` and `sway` are present.
- **No `lupdate`/`lrelease` on this box** (`qt6-qttools` is not in the image;
  the only copies on disk are inside a flatpak runtime and a vendored app).
  `/usr/lib64/qt6/bin/{qml,qmldom,qmllint}` are present.
- `at-spi2-core` 2.58.8 is installed; `pyatspi` is not (so an AT-SPI walk has to
  go over D-Bus with `gdbus`, not Python bindings).
- The two runnable shapes in this tree: `run-settings-controls-test.sh` stages
  real components next to a `Theme` stub and hands them to `qmltestrunner
  -platform offscreen` (real instantiation, real `QMouseEvent`/`QKeyEvent`);
  `tests/lib/headless.sh` gives a private labwc for anything that needs a real
  `quickshell` process. `tests/check-headless-runners.sh` enforces the second.
- `ci.yml` `on.push.branches` is `[main, dev]` — **not** `roadmap/v2.2`. CI here
  will not run a measurement that cannot also be run locally.
- `ci.yml` `structure-check` carries a REQUIRED file manifest; new suites go in
  it or they are not guarded.
- apex-greet is plain QML in apex-os (`files/desktop/apex-greet/GreetSurface.qml`,
  `required property var theme` + `ctx`) — so it stages into qmltestrunner the
  same way the settings controls do. That one fixture is where "accessible
  login" (P2-003) and "keyboard layout before password" (P2-004) both live.
- `files/system/libexec/apex-input-apply` models libinput only (touchpad,
  pointer, keyboard repeat). **There is no xkb layout in the APEX input model.**

## Ledger

Filled in as each row is measured. Nothing is marked green without a named
suite and a mutation pair.

### P2-003 — accessibility

| sub-feature | state | assertion |
| --- | --- | --- |
| screen reader | **partial** | Markup measured at runtime (see login row). The end-to-end AT-SPI walk is NOT done — see "What could not be measured". |
| magnifier | not present | no magnifier in either repo; wlroots has no standard one |
| high contrast | present, untested as a11y | six shaders in apex-shell `src/config/shaders/` incl. `HighContrast.glsl`, applied via Hyprland `decoration:screen_shader` only — niri and labwc get nothing, and it is shipped as a visual effect, not an a11y feature |
| reduced motion | **MEASURED and ratcheted** | `tests/check-reduce-motion.sh` — 12 assertions. Reaches **39 of 450** animation durations (8.7%); **402 are bare int literals** no switch can touch; 9 resolve to neither. Counts pinned exactly in both directions, the chain asserted link by link, 3 self-tests. Runtime half (turn it on in a live shell, read a Behavior's duration back) NOT built — `SettingsService` imports Quickshell so qmltestrunner cannot load it. |
| large text | partial by other means | no text-specific setting; only the global `Metrics.scale` (0.5–3.0). One real a11y constant: the 7px floor in `fs()`, enforced by `check-scale-tokens.sh` |
| colour filters | **present** | shipped shaders + hyprshade's protanopia/deuteranopia/tritanopia, Hyprland only |
| sticky keys | not present | 0 mentions in either repo |
| slow keys | not present | 0 mentions in either repo |
| mouse keys | not present | 0 mentions in either repo |
| on-screen keyboard | not present | no wvkbd/squeekboard/maliit anywhere; none in the image |
| keyboard-only installer | **not present** | installer is GUI-only (GTK4/Adw); the whiptail text UI was deleted deliberately. Not measured — see NEXT. |
| keyboard-only DESKTOP | **DONE for the shared controls** | `tests/run-a11y-controls-test.sh` — 21 runtime assertions. Every shared `Cfg*` control is now a tab stop and operates on Space/Enter, proved by posting real `QKeyEvent`s and counting the signal the pages listen to. Pages that use only these controls are covered; bespoke widgets in `popups/` are NOT. |
| screen-reader markup, desktop | **DONE for the shared controls** | same suite: `CfgRow` hands its label, description, disabled-reason and live readback to whatever control it holds; names and roles read back off the live attached objects. |
| accessible login/lock/recovery | **login DONE** | `tests/test-apex-greet-a11y.sh` — 22 runtime assertions on the shipped `GreetSurface.qml` under qmltestrunner: accessible name/role read off live objects, tab ring walked with real `Qt.Key_Tab`. Lock and recovery NOT done. |

### P2-004 — internationalisation

| sub-feature | state | assertion |
| --- | --- | --- |
| keyboard layout before password | **DONE — greeter and installer** | Greeter: `tests/test-apex-greet-a11y.sh` test_030-034 + `tests/test-apex-greet-layout.sh`. Installer (round 2): `installer/test-installer-keymap.sh` **35 assertions** and `installer/test-installer-locale.sh` **22**. The criterion itself is measured, not inferred — cage started with `XKB_DEFAULT_LAYOUT=de` hands a GTK4 client a keymap where the physical Y key gives `z`, Z gives `y`, `;` gives `ö`. The layout is applied to the RUNNING session by restarting the compositor, which is the only thing that can work (see the design fork). |
| timezone choice | **DONE (installer)** | Engine: `test-installer-locale.sh` — an explicit timezone becomes `/etc/localtime`; an explicitly chosen UTC is **not** overruled by the pre-existing Perth fallback, while an absent choice still keeps it. GUI: a searchable picker on the keyboard page fed from tzdata's own `zone1970.tab` (312 zones), asserted at runtime by `test-installer-keymap.sh` §6 — the real page is driven, Continue is pressed, and the recorded zone must be one that exists on the system. **This row was briefly written as DONE while the GUI had no timezone widget at all** — the engine honoured a key nothing set, so the override path was unreachable from the installer. Caught in review; the fix was to add the picker rather than soften the claim. |
| multiple layouts | **greeter DONE** | `test-apex-greet-layout.sh` "both configured layouts survive extraction" / "a three-layout machine reports all three in order". Nothing in the desktop shell switches layouts yet. |
| IME / fcitx5 | present in image, untested | `Containerfile.core:1182-1189` installs fcitx5 + chinese-addons/hangul/anthy/m17n; autostarted in 3 places. `QT_IM_MODULE`/`GTK_IM_MODULE` deliberately unset (Wayland text-input-v3). No suite asserts any of it. |
| CJK | fonts present, shell will tofu | `Containerfile.core:1319-1329` ships Noto CJK sans+serif. But apex-shell hardcodes `font.family: "JetBrains Mono"` at ~45 sites, which has no CJK coverage. |
| RTL | **absent, and the image cannot render it** | apex-shell: 0 `LayoutMirroring`, 0 `layoutDirection`. apex-os: **no `google-noto-sans-arabic/hebrew/thai/devanagari` package** — only DejaVu's partial coverage. Asymmetry worth noting: `fcitx5-m17n` provides INPUT for Arabic/Hebrew/Thai that the image cannot RENDER. |
| locales / timezones | **keyboard + timezone DONE, LOCALE still not offered** | The installer now collects keyboard and timezone and the engine honours both (above). **Locale is deliberately still not offered**, and this is a decision, not an omission: `Containerfile.core:809` installs **`glibc-langpack-en` only**, so a free locale picker would let a user choose one that silently degrades to `C.UTF-8` on the installed system — a worse failure than not asking, because it looks like it worked. Closing this row means adding langpacks to `Containerfile.core` first, then a picker restricted to what the target actually ships. Named, not faked. |
| translated installer | **not present** | 0 `qsTr`/gettext in `installer/`. The installer is GTK4/Python, so its route is gettext, not Qt — a separate pipeline from the one proven below. |
| translated shell | **pipeline PROVEN, 5 of ~650 strings** | `tests/run-i18n-test.sh` — **12 assertions**. All four links run with the real tools on a shipped file: `qsTr()` marks 5 strings in `AgentHelpContent.qml`; `lupdate-qt6` extracts exactly 5 into the right context; `lrelease-qt6` compiles `translations/apex-shell_de.ts`; and a **running QML engine under `qmltestrunner -translation`** reads the German back off the live singleton. The load-bearing assertion is the sensitivity one — the SAME fixture runs with and without `-translation` and the two must DISAGREE; three independent strings are sampled so one lucky match cannot carry it. **Named gap, asserted so it cannot quietly stop being true: nothing in `src/` installs a `QTranslator`**, so the pipeline works and no user sees a translated word yet. |
| per-user language | not present | nothing per-user anywhere |
| non-US recovery / install flows | **install flow DONE, recovery not** | The "engine only COPIES whatever the live ISO resolved" finding is **no longer true for install**: the engine now prefers an operator's `keymap`/`keyvariant`/`timezone` over every inference, and the installer collects them before the password. `bib-config.toml:70-72` still hardcodes `us`/`en_US.UTF-8`/`Australia/Perth`, but those are now the FALLBACK rather than the only outcome. **Recovery is untouched** — nothing in the recovery flow asks for or applies a layout, so a non-US user recovering a machine still types on `us`. Not measured, and named as the remaining half. |

## What could not be measured here, precisely

**The end-to-end AT-SPI walk** — starting the greeter under a private a11y bus
and reading the tree back the way Orca does. Two blockers, both verified rather
than assumed:

- `at-spi2-registryd` refuses to activate in a `dbus-run-session` here:
  `Activated service 'org.a11y.atspi.Registry' failed: … Permission denied`.
- The only a11y bus that *does* answer is `/run/user/1000/at-spi/bus` — Andre's
  own desktop session's. Reaching it is exactly what this program forbids.

The Qt side is NOT the blocker: `QSpiAccessibleBridge` is compiled into
Fedora's `libQt6Gui.so.6` (31 `org.a11y` symbols), so a Quickshell app can
publish a tree. **The missing assertion, named:** start `apex-greet` under a
private D-Bus + at-spi registry, and assert
`org.a11y.atspi.Accessible.GetChildren` on the app root returns the five
controls with the names `greet-a11y-test.qml` asserts. That belongs on a build
box, not on this laptop.

Separately, the image ships **zero accessibility packages** — no `orca`,
`at-spi2-core`, `speech-dispatcher`, `espeak-ng`, `brltty`, no on-screen
keyboard — so today there is no screen reader on APEX for the markup to reach.

## Mutation pairs

Each mutant changes one arm; a NAMED test must go red. Restores are `cp`.
Full script: `scratchpad/mutate-greet.sh`.

| # | mutant | caught by |
| --- | --- | --- |
| M1 | `Accessible.name: "Username"` → `""` | `test_010_username_has_accessible_name` |
| M2a | `KeyNavigation.tab: nextArrow` → `usernameInput` | `test_020_every_control_is_reachable_by_tab` |
| M2b | `activeFocusOnTab: true` → `false` on the session arrow | `test_023_every_ring_member_is_a_tab_stop` |
| M3 | `layouts.length > 1` → `> 0` | `test_034_single_layout_offers_no_switch` |
| M4 | status line `AlertMessage` → `StaticText` | `test_040_error_text_is_announced` |
| M5 | `Accessible.passwordEdit: true` → `false` | `test_013_password_is_marked_a_password` |
| M6 | layout extraction splits the JSON on commas | `both configured layouts survive extraction` |
| M7 | `if (!ctx.canSwitchLayout) return` → `if (false) return` | `one configured layout: cycleLayout runs nothing at all` |
| N1 | `CfgRow._adoptControls` stops calling `_pushA11y()` | `test_010_row_names_its_control` |
| N2 | `CfgRow` adopts controls that name themselves | `test_012_a_control_that_names_itself_is_left_alone` |
| N3 | `Accessible.checked: root.checked` → `false` | `test_021_checkable_controls_report_their_state` |
| N4 | `CfgTextField` input `activeFocusOnTab` → false | `test_030_every_control_is_a_tab_stop` |
| N5 | `CfgSwitch` drops `Qt.Key_Space` | `test_040_space_toggles_the_switch` |
| N6 | `CfgRow` stops announcing the readback | `test_013_description_carries_the_rows_prose_and_readback` |
| N7 | a segmented pill's `Accessible.name` → `""` | `test_022_each_pill_names_its_option` |
| N8 | `CfgRow` slot `enabled: !unavailable` → `true` | `test_045_a_disabled_control_ignores_the_keyboard` |

### Round 2b — internationalisation (4 pairs, all CAUGHT)

| # | mutant | caught by |
| --- | --- | --- |
| I1 | a string loses its `qsTr()` | `marks exactly 5 strings with qsTr()` + the extraction count |
| I2 | the `.ts` carries a stale source string | `every string … is one lupdate still extracts` (+2 more) |
| I3 | the `.ts` uses the wrong translation context | `the same fixture gives DIFFERENT text with and without -translation` |
| I4 | a "translation" identical to the English source | `translated, entryLabel reads the German` |

I3 and I4 are the two failures that are invisible to any check short of running
the engine: both produce a `.qm` that compiles and loads perfectly, and both
leave every word English. I2 was first recorded as SURVIVED — that was the
mutation harness grepping for a word the failure message does not contain, not a
survival; the three FAIL lines it produced name exactly the right assertions.

### Round 2 — the installer keyboard work (16 pairs, all CAUGHT)

Restores are `git checkout --` (authoritative content, fresh mtime), and the
whole file set is re-compared against HEAD after every mutate and every restore.

| # | mutant | caught by |
| --- | --- | --- |
| K1 | session never exports `XKB_DEFAULT_LAYOUT` | `the second start carries the chosen layout` |
| K2 | restart cap removed | `a GUI that asks forever is capped` |
| K3 | session stops validating the layout against xkb | `a layout xkb does not know is refused` |
| K4 | stale state file no longer cleared | `a restart request with no fresh state is refused` |
| K5 | variant never reaches the compositor | `a chosen variant reaches the compositor` |
| K6 | GUI uses exit 76, session still expects 75 | `both halves use the same restart exit code` |
| K7 | launcher bypasses the session script | `apex-installer-launch runs the session script` |
| K8 | engine ignores an explicit keymap | `an explicit layout reaches the X11 keyboard config` |
| K9 | Perth fallback overrules a deliberate UTC | `an explicitly chosen UTC is not overruled` |
| K10 | engine stops validating the layout before erasing | `a nonsense layout is refused` |
| K11 | keyboard page leaves the forward flow | `the welcome page's primary button leads straight to the keyboard page` |
| K12 | Containerfile stops installing the session script | `the launcher's exec target … is installed` |
| K13 | ISO build stops installing the session script | same |
| K14 | `Continue` stops setting `exit_code` | `…and the process exits 75` |
| K15 | the page stops recording the time zone | `records a real time zone in the answers` |
| K16 | the state file is written with the wrong key | `writes the chosen layout to the session state file` |

K14–K16 exist because §5 read the GUI's SOURCE — a grep for the constant, a
regex for the writer — and the `Continue` button had never been pressed by
anything. All three are invisible to a grep and caught only by §6, which runs
the shipped GUI under cage and clicks it.

**Two survived the first pass, and both were the SUITE's fault, not bad mutants:**

- **K4** survived because the assertion was **vacuous**. It checked that the
  first compositor start carried no layout, using a stub GUI that exited 0
  immediately — but the session only reads the state file after a restart
  REQUEST, so with no request the file was never consulted and the assertion
  held whether or not the clearing happened. Deleting the `rm -f` outright
  changed nothing. Identical in shape to N8 below. Rewritten to seed a stale
  layout, ask for a restart, and write no state — the only situation in which an
  uncleared file actually applies a layout nobody chose.
- **K11** survived because the page-order walk followed **Back buttons as
  forward edges**, making the navigation graph nearly undirected. With `Begin`
  pointed straight at wifi, `keyboard` was still "reachable" — via wifi's own
  Back button — and still sorted before `account`. The one step the entire
  criterion rests on could leave the forward flow without changing a single
  assertion. Reverse edges are now stripped, and the edge that matters is
  asserted on its own.

**And a defect the pairs exposed that no assertion had been looking for:** K12/K13
were added only after noticing that `apex-installer-launch` had been repointed at
`/usr/bin/apex-installer-session` while **nothing installed that file into the
image**. The ISO would have booted with no installer at all — cage never starts,
every boot lands on the diagnostic screen — and every assertion in the suite
passed, because they all read the source tree, where the file plainly exists.
The guard now resolves the exec target out of the launcher rather than
hardcoding a name.

**Earlier rounds' survivors, kept for the record:**

- **M2b** survived the tab-ring walk. Qt honours an explicit `KeyNavigation`
  link regardless of `activeFocusOnTab`, so the walk could not see the flag
  change. `test_023` was added to read the flag off the live object.
- **N8** survived, and the survival was the SUITE's fault. The fixture's
  disabled switch had no `onToggled` handler at all, so its counter could not
  move whatever happened: the assertion was that a number did not change, and
  the number had never been shown capable of changing. It now drives the row
  live and requires the same counter to move once the reason is cleared. This
  is the plainest false-green in the unit and it was invisible without the
  mutant.
- **M7** survived while its assertion was two `grep -q` calls. The stanza still
  CONTAINED both strings they looked for — `canSwitchLayout` in the readonly
  property that defines the guard, and `ctx.layouts.length > 1` in that
  property's own expression — so the grep stayed green with the guard deleted.
  This is exactly the "greps a stanza that also names the thing" trap. Replaced
  by extracting `cycleLayout` and RUNNING it.

## Landed

Both repos on `task/p2-b-accessibility-i18n`, both pushed, neither rebased.

**apex-os** — `a0dd1999`, `e5d049ad`, `69f59cb0`
- `tests/test-apex-greet-a11y.sh` + `greet-a11y-test.qml` — **22 assertions**.
- `tests/test-apex-greet-layout.sh` — **25 assertions** locally (23 + 1 skip on
  the CI runner, which has no qmllint), including a two-mutant inline self-test.
- Baseline before the fix: **15 of 17** accessibility/keyboard assertions failed
  on the shipped greeter.
- CI **confirmed green on the branch** (run 34640150697): the a11y suite prints
  `Totals: 22 passed, 0 failed` and the layout suite `23 passed, 0 failed, 1
  skipped`. The a11y suite runs in a **Fedora container** — ubuntu-24.04 ships
  Qt 6.4 and the greeter imports `QtQuick.Effects` (MultiEffect, Qt 6.5+), so on
  the runner itself it skipped on every run. The first dispatch proved that:
  `E: Unable to locate package qml6-module-qtquick-effects` → `SKIP`.
- Two jobs in that run are red for reasons **predating this branch and untouched
  by it** — this branch changes no Rust and no agent code:
  `tests/test-agent-inject.sh` and `apex-agentd/tests/request_origin.rs` both
  fail because `/proc/<pid>/cgroup` places the hosted runner's connection in
  `system.slice/hosted-compute-agent.service`, which is neither a login session
  nor a user service, so origin detection cannot classify it.

**apex-shell** — `83775dc`, `7ba196e`
- `tests/run-a11y-controls-test.sh` + `a11y-controls-test.qml` — **21
  assertions** over the shipped `src/components/config`.
- `CfgRow` now hands its words to the control it holds; `CfgSwitch`,
  `CfgButton`, `CfgTile`, `CfgSegmented`, `CfgTextField`, `CfgSlider`,
  `CfgSwatch` gained roles, state, tab stops, Space/Enter and focus rings.
- `tests/check-reduce-motion.sh` — **12 assertions** measuring and pinning how
  far Reduce Motion reaches (39/450), with 3 self-tests including an inverse
  prose mutant.
- Wired into `ci.yml` and `structure-check`'s manifest.
- Regression found and fixed: `check-wheel-value.sh`'s third self-test mutant
  anchored on the exact `CfgSwitch` line this change rewrites. It reported *"the
  mutation did not apply, so its verdict is meaningless"* rather than passing —
  the anchor was updated, 6 applied / 0 failed-to-apply.

Qt finding worth keeping: `Accessible.name` and `Accessible.passwordEdit` are
**mutually exclusive**. `qquickaccessibleattached_p.h:91-93` returns an empty
QString from `name()` whenever `passwordEdit` is set; `description()` (:111) has
no such guard. A password field can be one a reader will not echo, or one a
reader can name — not both. The greeter keeps the flag and carries the label in
the description; `test_002_qt_suppresses_name_for_password_edits` pins the Qt
behaviour with its own probe items.

## Installer facts established (for whoever writes the page)

- Page order is NOT derived from one list. It lives in hardcoded `self.go("…")`
  targets on every page, plus FOUR duplicated places: the `for name, build in`
  tuple AND `self.builders` dict (`apex-installer-gui:444-456`), the
  `STEP N OF 6` literals (12 call sites), the welcome-page bullet list, and
  `test-installer.sh:224-225`.
- `test-installer.sh:224-225` extracts page names with `grep -oE '"[a-z]+"'`. A
  page named `keyboard-locale` or `kb_tz` is **silently dropped** from the render
  suite with no failure. Name it `[a-z]+`.
- `measure.py` builds EVERY page; a `self.answers['newkey']` with brackets raises
  `KeyError` and every page then fails as "never measured". Use `.get()`.
- The engine has ONE flag (`--headless ANSWERS`). Values travel as `key=value`
  lines in a 0600 file; unknown key → `die`. A new key must be added in FOUR
  places: the GUI's key tuple (`:1423`), the engine's `case` block (`:698-708`),
  the globals-clear line (`:689`), and validation.
- `spawn_engine()` writes `if a.get(k):` — an empty value never reaches the
  engine, so `set_locale_keymap_in()`'s live-env fallback stays load-bearing.
- Layout enumeration already has an in-repo idiom:
  `apex-shell-firstrun:175-183` parses `/usr/share/X11/xkb/rules/base.lst`, and
  `xkeyboard-config` is explicitly installed in the live ISO
  (`Containerfile.installer:147`).
- **Locale caveat:** the image installs `glibc-langpack-en` ONLY. A free locale
  picker would let a user choose one that silently degrades to `C.UTF-8` on the
  installed system. Either restrict the list to what the target ships or add
  langpacks to `Containerfile.core`.

## DESIGN FORK — SETTLED 2026-09-12 (round 2): option (b), and the reason is measured

The fork below asked whether the chosen layout can reach the RUNNING cage
session. It can, by exactly one route, and the other two are now ruled out by
measurement rather than by argument.

**Chosen: (b) — re-exec cage with `XKB_DEFAULT_LAYOUT` after the pick.**

**(a) `localectl set-x11-keymap` is dead, not merely doubted.** The engine's own
comment worries that `systemd-localed` may not answer over dbus. That worry is
beside the point: even a localed that answers perfectly cannot change a running
wlroots session. Measured on this box —

    ldd /usr/lib64/libxkbcommon.so.0   →  linux-vdso, libc, ld-linux.  Nothing else.
    strings libxkbcommon.so.0 | grep -Ei 'locale1|dbus|xorg\.conf|localectl'  →  empty
    strings libxkbcommon.so.0 | grep '^XKB_DEFAULT'  →  RULES MODEL LAYOUT VARIANT OPTIONS

libxkbcommon links **only libc**. It has no dbus dependency and not one string
naming `org.freedesktop.locale1`, `localectl` or `xorg.conf`. Its entire
configuration surface is those five environment variables. `localectl
set-x11-keymap` writes `/etc/X11/xorg.conf.d/00-keyboard.conf` and
`/etc/vconsole.conf` and broadcasts on dbus — three channels, none of which
libxkbcommon reads. So (a) changes the INSTALLED system's files and leaves the
keyboard under the user's hands exactly as it was. It is (c) wearing a costume.

**(b) works, and the env var is read at keymap-compile time**, which is why a
re-exec and not a poke is required. Measured with `ctypes` against the real
library, making the same call wlroots makes (`xkb_keymap_new_from_names(ctx,
NULL, 0)` — all-NULL RMLVO is what makes libxkbcommon consult the env):

| `XKB_DEFAULT_LAYOUT` | KEY_Y | KEY_Z | KEY_Q | KEY_SEMICOLON |
| --- | --- | --- | --- | --- |
| `us` | y | z | q | ; |
| `de` | z | y | q | ö |
| `fr` | y | w | a | m |

and setting the var *after* a keymap is compiled leaves that keymap unchanged —
only the next compile sees it. That is the whole argument for (b) in one line:
**the layout is fixed when the compositor starts, so the compositor has to start
again.** cage is on the same path — `ldd /usr/bin/cage` shows both
`libwlroots-0.18.so` and `libxkbcommon.so.0`.

**(c) alone was never enough**, and the predecessor's engine work is (c). Kept:
it is necessary — it is what puts the layout on the INSTALLED system. It is just
not what lets the user type the password.

**How (b) is built, given the launcher's "one path, no probes" rule.** No loop
goes into `apex-installer-launch` — its retry-then-diagnose path is the safety
net against a black screen and must not grow a second reason to re-run cage. A
separate `apex-installer-session` owns the cage loop; the launcher's `GUI_CMD`
changes by one line and its outer retry still wraps the whole thing. The restart
is requested by exit code: measured, `cage -- sh -c 'exit 75'` returns **75**, so
cage propagates its child's status and the GUI can ask for a restart without a
state file being the only channel. Restarts are hard-capped, because an
unbounded loop behind a compositor is the exact failure the launcher exists to
prevent. The `keyboard` page goes immediately after `welcome`, so the only state
crossing the restart is the layout itself.

**What is NOT measurable on this laptop, named precisely.** The last link —
*cage, on a seat with a real keyboard, started with `XKB_DEFAULT_LAYOUT=de`,
hands a GTK4 client a keymap in which `Gdk.Display.map_keycode(29)` returns
`z`* — is not assertable here. The wlroots **headless** backend creates no input
device, so the seat has no keyboard capability and GDK falls back to a fixed
keymap: the probe read `us` under `XKB_DEFAULT_LAYOUT=de`, which is a
false-green shape and is therefore not shipped. The obvious workaround does not
work either: a `zwp_virtual_keyboard_v1` client (`wtype`) uploads its **own**
keymap, so it would measure wtype rather than cage. Closing it needs a real
input device or the wlroots X11 backend, and there is no `Xvfb`, `Xephyr`,
`Xwayland` or `weston` on this box. Same shape as the AT-SPI row, left partial
for the same reason. What IS shipped is the layer below it — libxkbcommon, the
single component that decides the answer — asserted against the real library.

## Packages installed on this laptop (round 2), and what each unblocked

Andre lifted the no-install constraint mid-round: `sudo apex install <pkg>`
builds a systemd sysext and merges it live. Two installs, both to close a row
that was otherwise going to be reported unmeasurable:

- **`xorg-x11-server-Xvfb`** — closed the design fork's named partial. The
  wlroots HEADLESS backend creates no input device, so the seat has no keyboard
  capability and a GTK client reads a fixed `us` keymap whatever
  `XKB_DEFAULT_LAYOUT` says. The wlroots **X11** backend does create one, so
  cage runs on a private Xvfb and the criterion becomes assertable. This is the
  difference between "not measurable here" and `test-installer-keymap.sh`
  test 3b.
- **`qt6-qttools-devel`** — `qt6-qttools` alone does NOT ship the binaries; the
  devel subpackage is what provides `/usr/bin/lupdate-qt6` and `lrelease-qt6`.
  This is what makes "prove strings extract and substitute" measurable at all,
  against the card's earlier finding that no lupdate existed on this box.

Caveat recorded, not acted on: `apex install` rebuilds the extension from all
enabled repos, and `/etc/yum.repos.d/chatgpt.repo` is enabled, so the extension
now also carries `chatgpt` (29 → 36 packages, 643 → 651 MB). Left alone — that
repo is Andre's call.

## A harness bug that would have made every mutation verdict worthless

The first mutation harness backed each file up to `$BK/$(basename f).orig` and
restored with `cp`. Partway through the run the working tree's
`apex-installer-session` was found holding **the launcher's contents** (369
lines instead of 171). The committed version was intact; only the working tree
was corrupted, and it was caught by an editor notice rather than by the harness,
which had no integrity check at all.

Worth recording because of what it implies: the run was producing verdicts
against a tree that was wrong in a place no assertion looked at, so every
"CAUGHT"/"SURVIVED" after the corruption meant nothing. The backup/restore logic
was afterwards reproduced in isolation and behaved correctly, and the suite alone
(28/28) leaves the tree clean — so the cause is in the harness, not in the suite
or the files.

Take 2 does not use a hand-rolled backup at all: restores are `git checkout --`,
which is authoritative about content and gives a fresh mtime (the property the
"plain `cp`, never `mv`, never `cp -p`" rule exists to protect), and the whole
file set is compared against HEAD **after every mutate and every restore**. A
step that finds the tree dirty in an unexpected place aborts the run and names
itself instead of reporting a verdict.

## The predecessor's last note, run down: the premise was false, and it is a defect

The note was that a bad-username case failed the same way in the new work as in
an existing suite, and that the existing suite must know how to neutralise it.
**It does not.** `test-installer.sh` has no neutralisation, no stub, no fake
image and no `APEX_*` override anywhere near its engine calls (`check()` at :67
and the bare call at :90 are the only two, both plain `sudo -n "$ENGINE"`). Its
entire engine half — argument handling, account validation, answers-file
handling — dies at `apex-install:354`, `The APEX-OS image (localhost/apex-os:daily)
is not present in the live environment`, before argument parsing is even reached.

That is not a regression: `git log -S` puts the image check in `dddabd6f`
(2026-07-23) and the engine test cases in `33b744d5` five days later. The suite
was written against an engine that already refused it, and only ever passed on a
box whose **root** podman storage held `localhost/apex-os:daily` — the ISO build
box. `pr-validation.yml:1082` runs it on a bare `ubuntu-24.04` with no image, so
those cases are dead in CI too. Corroborating tell that does not need the theory:
its "no arguments" case at :90 asserts `rc = 2` and `not a user interface`, and
preflight's `die()` exits 1 — so that case cannot pass here either.

**The neutralisation, found and verified.** `apex-install:56` is
`IMAGE="${APEX_IMAGE:-localhost/apex-os:${EDITION}}"` with the comment
"override with APEX_IMAGE=... for testing". Verified safe before use, not
assumed: the only `bootc install to-disk --wipe` reachable before the validation
block is inside `if [ "$UNATTENDED" = 1 ]`, and both its gates are shut here
(`apex.unattended` is not on `/proc/cmdline`, `/usr/share/apex-installer/allow-unattended`
does not exist). Everything between preflight and validation is function
definitions. Measured, with the override pointing at an image that does exist:

    keymap=NOT_A_LAYOUT  → APEX-INSTALL-FAILED: Invalid keyboard layout 'NOT_A_LAYOUT'. Nothing has been erased.
    keymap=de            → APEX-INSTALL-FAILED: /dev/zzz-does-not-exist is not a block device.   (past validation)
    username='Bad Name'  → APEX-INSTALL-FAILED: Invalid username 'Bad Name'. … Nothing has been erased.

The third line is `test-installer.sh`'s own dead case, alive. Note `sudo`'s
`env_reset` strips `APEX_*` from the caller's environment, so it must be passed
as `sudo -n APEX_IMAGE=… ./apex-install`, not exported beforehand.

## Landed in round 2

Both repos on `task/p2-b-accessibility-i18n`, pushed, not rebased.

**apex-os** — `fc4dbd2d`, `28f59fac`, `fcd3b904`, `3431376f`, `5670eedf`, and the
render-context fix.

- `installer/apex-installer-session` — owns the cage loop so a chosen layout can
  reach the running session. Bounded at 3 restarts; refuses a layout xkb does not
  know; clears a stale state file before the first launch.
- `installer/apex-installer-gui` — `p_keyboard` at step 2, before the account
  page, with a layout picker, a variant picker, a time-zone picker (312 zones
  from tzdata's `zone1970.tab`) and a field to TYPE IN and check.
- `installer/apex-install` — accepts `keymap`/`keyvariant`/`timezone`, prefers
  them over every inference, validates the layout against xkb's own `base.lst`
  before anything is erased.
- `installer/test-installer-keymap.sh` — **35 assertions**.
- `installer/test-installer-locale.sh` — **22 assertions**.
- `installer/test-installer.sh` — **44**, of which 11 engine cases had been dead
  since July and 2 GUI render rows are the new page.

**apex-shell** — `eed578b`.

- `tests/run-i18n-test.sh` — **12 assertions**; `translations/apex-shell_de.ts`;
  five `qsTr()` call sites in `AgentHelpContent.qml`.

**Three defects found that nothing was looking for**, all pre-existing except the
first, which this branch introduced and caught before it shipped:

1. `apex-installer-launch` was repointed at `/usr/bin/apex-installer-session`
   while **nothing installed that file into the image** — the ISO would have
   booted with no installer at all. Every assertion passed while it was true,
   because they all read the source tree.
2. `test-installer.sh`'s engine half had been dead since 2026-07-28 — on every
   machine that is not the ISO build box, and in CI.
3. The GUI render container's build context was `/var/empty`, which does not
   exist on an ubuntu runner, and the build's error output went to `/dev/null`.

## The defect CI found that this laptop could not

Worth its own section because it is the argument for dispatching CI at all.

`set_locale_keymap_in()` falls back to `Australia/Perth` when the live
environment has no real timezone. The guard was `case "$tz" in ""|"UTC")`.
systemd writes the **canonical** zone name into `/etc/localtime`, and on a
machine with no timezone configured that name is **`Etc/UTC`**, not `UTC` — so
the commonest spelling of the exact case the line exists to catch went past it.
The result was not a wrong timezone but **none**: `tz` stayed `Etc/UTC`, the
deploy root had no such entry, the `[ -e ]` guard declined to link it, and the
installed system got no `/etc/localtime` at all.

**Why the test could not see it here.** The assertion read the REAL
`/etc/localtime`, so its verdict depended on where the tester lives. This laptop
is set to `Australia/Perth` — which is exactly the value the assertion expects —
so it passed for the wrong reason. The runner resolves `Etc/UTC`, and failed.

The extracted function's two HOST reads are now redirected to a file the suite
controls, making the live zone an INPUT rather than a property of the machine.
Three cases now run deterministically: no zone → Perth, `Etc/UTC` → Perth,
`Europe/Berlin` → carried through. The third is what stops the fix degenerating
into "always Perth", which would throw away a zone the ISO had already resolved.

The redirect is asserted in **both** directions. A blanket
`s|/etc/localtime|…|` also rewrites `"$deploy/etc/localtime"`, which is where
the function WRITES its answer; the first attempt did that and turned every
assertion into `want [a zone] got []`. One assertion checks the read moved,
another checks the write did not.

## CI, round 2

apex-os's `pr-validation.yml` accepts `workflow_dispatch`, so this branch was
dispatched rather than guessed at (`gh workflow run pr-validation.yml --ref
task/p2-b-accessibility-i18n`).

**The `Installer safety and UI` job had never run for this program at all** — it
is gated on `changes.outputs.installer`, and no roadmap branch had touched
`installer/` until this one. Turning it on surfaced two latent defects rather
than introducing them (the July-dead engine half, and the `/var/empty` build
context). The engine cases now PASS on the ubuntu runner, which is the first time
they have run anywhere but the ISO build box.

**Red jobs that are not this branch's**, checked rather than assumed: `Rust
validation` and `Package engine`. Against its true branch point
(`git merge-base` = `4f80746e`, not `origin/roadmap/v2.2`, which has moved ahead)
this branch changes **15 files, +3337/−48, and not one `.rs` file, nothing under
`apex-agentd/`, and no package engine**. The Rust failure is the known cgroup
one: `/proc/<pid>/cgroup` places the hosted runner in
`system.slice/hosted-compute-agent.service`, which is neither a login session nor
a user service, so origin detection cannot classify it. Diffing against
`origin/roadmap/v2.2` directly is misleading here and says 45 files — that is the
base moving, not this branch.

**apex-shell CI cannot verify the i18n suite on this branch.** Its `ci.yml` had
only `push`/`pull_request` on `main` and `dev`, so no roadmap task branch has
ever been tested by it — every suite in that repository has been verified on one
laptop. `workflow_dispatch` is added on this branch for the next person, with the
caveat in the comment: GitHub only offers a dispatch once the file is on the
DEFAULT branch, so it does nothing for the branch that adds it. The 12 i18n
assertions are local-only until this lands.

## NEXT

1. **The `sections` array in `AgentHelpContent.qml`** (~200 prose strings) is the
   obvious next i18n increment: the pipeline is proven, so this is now mechanical.
   One caution measured the hard way — `tests/check-agent-help.sh` greps for the
   exact shape `{ k: "kv", t: "$m"`, so wrapping those `t:` values in `qsTr()`
   breaks it; that suite and this conversion have to move together.
2. **Nothing installs a `QTranslator`.** Until something does, the shell's
   translation pipeline reaches no user. `run-i18n-test.sh` asserts the absence,
   so the row flips itself when somebody wires it up.
3. **Locale is still not offered by the installer, deliberately.** The image
   installs `glibc-langpack-en` ONLY (`Containerfile.core:809`), so a free picker
   would let a user choose a locale that silently degrades to `C.UTF-8`. Add
   langpacks first, then a picker restricted to what the target ships.
4. **The installer is GTK4/Python**, so its translation route is gettext, not the
   Qt pipeline proven here. Separate work.
5. Remaining accessibility gaps, unchanged from round 1: the end-to-end AT-SPI
   walk (needs a private a11y bus that would not activate here); `src/popups/`
   and `src/nexus/NavPane.qml` still use bespoke Rectangle+MouseArea and are
   mouse-only and unnamed; the image ships **zero** accessibility packages, so
   the markup reaches no screen reader; and no Arabic/Hebrew/Thai fonts, so RTL
   input from `fcitx5-m17n` cannot be rendered.
6. **`Xvfb` is now the tool that makes the compositor-keymap criterion
   measurable.** If a future runner lacks it, `test-installer-keymap.sh` §3 SKIPs
   rather than lying — but a skip there means the criterion is unmeasured, not
   met.

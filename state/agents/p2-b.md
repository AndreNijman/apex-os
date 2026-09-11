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
| reduced motion | **present, 11% effective, 0 tests** | `SettingsService.reduceMotion` → `effectiveAnim`; reaches 48 of 451 `duration:` sites. 402 are int literals. `grep reduceMotion tests/` = 0 |
| large text | partial by other means | no text-specific setting; only the global `Metrics.scale` (0.5–3.0). One real a11y constant: the 7px floor in `fs()`, enforced by `check-scale-tokens.sh` |
| colour filters | **present** | shipped shaders + hyprshade's protanopia/deuteranopia/tritanopia, Hyprland only |
| sticky keys | not present | 0 mentions in either repo |
| slow keys | not present | 0 mentions in either repo |
| mouse keys | not present | 0 mentions in either repo |
| on-screen keyboard | not present | no wvkbd/squeekboard/maliit anywhere; none in the image |
| keyboard-only installer | **not present** | installer is GUI-only (GTK4/Adw); the whiptail text UI was deleted deliberately. Not yet measured. |
| accessible login/lock/recovery | **login DONE** | `tests/test-apex-greet-a11y.sh` — 22 runtime assertions on the shipped `GreetSurface.qml` under qmltestrunner: accessible name/role read off live objects, tab ring walked with real `Qt.Key_Tab`. Lock and recovery NOT done. |

### P2-004 — internationalisation

| sub-feature | state | assertion |
| --- | --- | --- |
| keyboard layout before password | **greeter DONE, installer NOT** | `tests/test-apex-greet-a11y.sh` test_030-034 (indicator visible, bound to the session, above and before the password field, switchable) + `tests/test-apex-greet-layout.sh` 22 assertions. **The installer still has no keyboard step at all** — see below. |
| multiple layouts | **greeter DONE** | `test-apex-greet-layout.sh` "both configured layouts survive extraction" / "a three-layout machine reports all three in order". Nothing in the desktop shell switches layouts yet. |
| IME / fcitx5 | present in image, untested | `Containerfile.core:1182-1189` installs fcitx5 + chinese-addons/hangul/anthy/m17n; autostarted in 3 places. `QT_IM_MODULE`/`GTK_IM_MODULE` deliberately unset (Wayland text-input-v3). No suite asserts any of it. |
| CJK | fonts present, shell will tofu | `Containerfile.core:1319-1329` ships Noto CJK sans+serif. But apex-shell hardcodes `font.family: "JetBrains Mono"` at ~45 sites, which has no CJK coverage. |
| RTL | **absent, and the image cannot render it** | apex-shell: 0 `LayoutMirroring`, 0 `layoutDirection`. apex-os: **no `google-noto-sans-arabic/hebrew/thai/devanagari` package** — only DejaVu's partial coverage. Asymmetry worth noting: `fcitx5-m17n` provides INPUT for Arabic/Hebrew/Thai that the image cannot RENDER. |
| locales / timezones | defaults only, no choice | `Containerfile.core:2027-2030` writes `LANG=en_US.UTF-8` + `KEYMAP=us`; `:809` installs **`glibc-langpack-en` only**; `:2044` hardcodes `/etc/localtime` → `Australia/Perth`. Installer has no step to change any of it. |
| translated installer | **not present** | 0 `qsTr`/gettext in `installer/` |
| translated shell | **not present** | 0 `qsTr`, 0 `.ts`/`.qm`, no lupdate/lrelease step. ~671 direct + 85 secondary hardcoded user-facing literals; 56% of them in `src/services/config_tab/` |
| per-user language | not present | nothing per-user anywhere |
| non-US recovery / install flows | not present | engine only COPIES whatever the live ISO resolved; `bib-config.toml:70-72` hardcodes `us`/`en_US.UTF-8`/`Australia/Perth` |

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

**Two survived a first pass, and both were real weaknesses, not bad mutants:**

- **M2b** survived the tab-ring walk. Qt honours an explicit `KeyNavigation`
  link regardless of `activeFocusOnTab`, so the walk could not see the flag
  change. `test_023` was added to read the flag off the live object.
- **M7** survived while its assertion was two `grep -q` calls. The stanza still
  CONTAINED both strings they looked for — `canSwitchLayout` in the readonly
  property that defines the guard, and `ctx.layouts.length > 1` in that
  property's own expression — so the grep stayed green with the guard deleted.
  This is exactly the "greps a stanza that also names the thing" trap. Replaced
  by extracting `cycleLayout` and RUNNING it.

## Landed

`apex-os` `42f23e62` on `task/p2-b-accessibility-i18n` (pushed).
`tests/test-apex-greet-a11y.sh` 22 assertions + `tests/test-apex-greet-layout.sh`
22 assertions, both wired into `pr-validation.yml` and its shellcheck list.

Qt finding worth keeping: `Accessible.name` and `Accessible.passwordEdit` are
**mutually exclusive**. `qquickaccessibleattached_p.h:91-93` returns an empty
QString from `name()` whenever `passwordEdit` is set; `description()` (:111) has
no such guard. A password field can be one a reader will not echo, or one a
reader can name — not both. The greeter keeps the flag and carries the label in
the description; `test_002_qt_suppresses_name_for_password_edits` pins the Qt
behaviour with its own probe items.

## NEXT

1. **The installer's missing keyboard/locale/timezone step.** This is P2-004's
   acceptance criterion read literally — "keyboard layout before password
   CREATION" — and the installer creates the password on its `account` page.
   `apex-installer-gui:444-456` lists ten pages, `welcome → wifi → disk → mode →
   part → account → secureboot → confirm → run → done`, and none of them is a
   locale/keyboard/timezone page. The engine's `set_locale_keymap_in()`
   (`installer/apex-install:424-510`) already writes all three into the deploy
   root — it just copies whatever the live ISO resolved, and the ISO hardcodes
   `us`/`en_US.UTF-8`/`Australia/Perth` (`bib-config.toml:70-72`). So the
   plumbing exists and the UI to collect a choice does not. Measurable: page
   order (keyboard before account), the engine honouring explicit overrides, and
   a non-US layout reaching the deploy root.
2. **apex-shell accessibility baseline.** 0 `Accessible.*` across 209 QML files,
   0 `FocusScope`, 0 `KeyNavigation`, one real `activeFocusOnTab` (CfgSlider).
   The shared `src/components/config/Cfg*` controls are the leverage point — a
   name and role there fixes every Settings page at once. Same runtime shape as
   the greeter suite; `run-settings-controls-test.sh` already stages that tree.
3. **Reduce-motion has no test at all** and reaches 11% of animations. Cheapest
   honest measurement in the unit: instantiate a real component, toggle the
   setting, read `duration` back at runtime, and report the covered/total count.
4. Image packages: no `at-spi2-core`/`orca` means the markup reaches nothing;
   no Arabic/Hebrew fonts means RTL cannot render.

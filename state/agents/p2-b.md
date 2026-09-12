# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

**Round 18 is the live one.** Branch `task/p2-b-round18` in both repos, forked
from `origin/roadmap/v2.2` after rounds 1-2 landed:

- apex-shell: `/var/tmp/apex-work/wt-p2-b2`    (off `1417402`)
- apex-os:    `/var/tmp/apex-work/wt-p2-b2-os` (off `cafd3635`)

Rounds 1-2, both landed into `roadmap/v2.2`, worktrees kept for reference:

- apex-shell: `/var/tmp/apex-work/wt-p2-b`    on `task/p2-b-accessibility-i18n`
- apex-os:    `/var/tmp/apex-work/wt-p2-b-os` on `task/p2-b-accessibility-i18n`

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
| screen reader | **MEASURED over real AT-SPI (round 18)** | `tests/test-apex-greet-atspi.sh` — 30 assertions, 8/8 mutants caught. The shipped surface runs in a real window on a private compositor against a private a11y bus, and the tree is read back over D-Bus exactly as Orca reads it. The predecessor's "cannot activate" blocker was misdiagnosed — it is SELinux silently refusing the ACTIVATION path, and nothing needs activating. **Still partial for two product reasons, both named and neither a harness limit:** the shipped greeter has no D-Bus session bus, so in production the bridge has nothing to publish on (measured: zero nodes); and `orca` was not in the image, so there was no reader. **Round 18b closed the second of those**: `Containerfile.core` now installs `orca` (stage 5a-a11y), asserted by `tests/test-apex-a11y-stack.sh` — 13 assertions, 4/4 mutants caught. The first is now ASSERTED rather than only observed: `tests/test-apex-greet-session-bus.sh` — 16 assertions, 6/6 mutants caught — runs greetd's own command string with the compositor and quickshell stubbed and shows the client can reach no session bus on either host. It goes RED when somebody fixes it. |
| magnifier | not present | no magnifier in either repo; wlroots has no standard one |
| high contrast | present, untested as a11y | six shaders in apex-shell `src/config/shaders/` incl. `HighContrast.glsl`, applied via Hyprland `decoration:screen_shader` only — niri and labwc get nothing, and it is shipped as a visual effect, not an a11y feature |
| reduced motion | **MEASURED and ratcheted** | `tests/check-reduce-motion.sh` — 12 assertions. Reaches **39 of 450** animation durations (8.7%); **402 are bare int literals** no switch can touch; 9 resolve to neither. Counts pinned exactly in both directions, the chain asserted link by link, 3 self-tests. Runtime half (turn it on in a live shell, read a Behavior's duration back) NOT built — `SettingsService` imports Quickshell so qmltestrunner cannot load it. |
| large text | partial by other means | no text-specific setting; only the global `Metrics.scale` (0.5–3.0). One real a11y constant: the 7px floor in `fs()`, enforced by `check-scale-tokens.sh` |
| colour filters | **present** | shipped shaders + hyprshade's protanopia/deuteranopia/tritanopia, Hyprland only |
| sticky keys | not present | 0 mentions in either repo |
| slow keys | not present | 0 mentions in either repo |
| mouse keys | not present | 0 mentions in either repo |
| on-screen keyboard | not present | no wvkbd/squeekboard/maliit anywhere; none in the image |
| keyboard-only installer | **MEASURED (rounds 18 / 18b)** | `installer/test-installer-a11y.sh` — **51 assertions** (38 before this round); mutation verdicts in the ROUND 18b table below, never quoted from here. The shipped GTK4 GUI runs on a private Xvfb; `xdotool` delivers real X key events into the real toolkit and the resulting focus, names and text are read back over AT-SPI. SIX of the installer's eleven pages are name-audited (welcome, keyboard, wifi, secureboot, **confirm**, account); the Tab ring is walked on `account` and required to reach every field, name every stop and close; the fields are typed into with the keyboard alone and read back, with the password still masked on the bus; and **the keyboard alone advances the flow** — Tab to `Begin`, then Return, then Space, each in its own process, each required to produce the next page's sentinel AND to leave the old page's primary button gone. NOT audited, and named: `disk`/`mode`/`part` enumerate real block devices, `run` starts an install, `done` follows one. |
| keyboard-only DESKTOP | **DONE for the shared controls** | `tests/run-a11y-controls-test.sh` — 21 runtime assertions. Every shared `Cfg*` control is now a tab stop and operates on Space/Enter, proved by posting real `QKeyEvent`s and counting the signal the pages listen to. Pages that use only these controls are covered; bespoke widgets in `popups/` are NOT. |
| screen-reader markup, desktop | **DONE for the shared controls** | same suite: `CfgRow` hands its label, description, disabled-reason and live readback to whatever control it holds; names and roles read back off the live attached objects. |
| accessible login/lock/recovery | **login DONE, twice over** | QML side: `tests/test-apex-greet-a11y.sh` — 22 assertions on live objects. Bus side (round 18): `tests/test-apex-greet-atspi.sh` — 30 assertions on what the BRIDGE publishes, including that the password never crosses the bus and that a reader can operate the session picker and the layout pill via `DoAction`. Lock and recovery NOT done. |

### P2-004 — internationalisation

| sub-feature | state | assertion |
| --- | --- | --- |
| keyboard layout before password | **DONE — greeter and installer** | Greeter: `tests/test-apex-greet-a11y.sh` test_030-034 + `tests/test-apex-greet-layout.sh`. Installer (round 2): `installer/test-installer-keymap.sh` **43 assertions** and `installer/test-installer-locale.sh` **26**. The criterion itself is measured, not inferred — cage started with `XKB_DEFAULT_LAYOUT=de` hands a GTK4 client a keymap where the physical Y key gives `z`, Z gives `y`, `;` gives `ö`. The layout is applied to the RUNNING session by restarting the compositor, which is the only thing that can work (see the design fork). |
| timezone choice | **DONE (installer)** | Engine: `test-installer-locale.sh` — an explicit timezone becomes `/etc/localtime`; an explicitly chosen UTC is **not** overruled by the pre-existing Perth fallback, while an absent choice still keeps it. GUI: a searchable picker on the keyboard page fed from tzdata's own `zone1970.tab` (312 zones), asserted at runtime by `test-installer-keymap.sh` §6 and §6b — the real page is driven, Continue is pressed, and the recorded zone must be one that exists on the system; §6b proves the pick also SURVIVES the compositor restart. **This row was twice written as DONE while a path through it did not work** — first while the GUI had no timezone widget at all — the engine honoured a key nothing set, so the override path was unreachable from the installer. and then while the pick was silently discarded by the compositor restart (a new process, empty `answers`, dropdown reset to the ISO default, and a resume note that tells the user to check the KEYBOARD so nobody looks again). Both caught in review; both fixed rather than softened. |
| multiple layouts | **greeter DONE** | `test-apex-greet-layout.sh` "both configured layouts survive extraction" / "a three-layout machine reports all three in order". Nothing in the desktop shell switches layouts yet. |
| IME / fcitx5 | present in image, untested | `Containerfile.core:1182-1189` installs fcitx5 + chinese-addons/hangul/anthy/m17n; autostarted in 3 places. `QT_IM_MODULE`/`GTK_IM_MODULE` deliberately unset (Wayland text-input-v3). No suite asserts any of it. |
| CJK | fonts present, shell will tofu | `Containerfile.core:1319-1329` ships Noto CJK sans+serif. But apex-shell hardcodes `font.family: "JetBrains Mono"` at ~45 sites, which has no CJK coverage. |
| RTL | **absent, and the image cannot render it** | apex-shell: 0 `LayoutMirroring`, 0 `layoutDirection`. apex-os: **no `google-noto-sans-arabic/hebrew/thai/devanagari` package** — only DejaVu's partial coverage. Asymmetry worth noting: `fcitx5-m17n` provides INPUT for Arabic/Hebrew/Thai that the image cannot RENDER. |
| locales / timezones | **keyboard + timezone DONE, LOCALE still not offered** | The installer now collects keyboard and timezone and the engine honours both (above). **Locale is deliberately still not offered**, and this is a decision, not an omission: `Containerfile.core:809` installs **`glibc-langpack-en` only**, so a free locale picker would let a user choose one that silently degrades to `C.UTF-8` on the installed system — a worse failure than not asking, because it looks like it worked. Closing this row means adding langpacks to `Containerfile.core` first, then a picker restricted to what the target actually ships. Named, not faked. |
| translated installer | **not present** | 0 `qsTr`/gettext in `installer/`. The installer is GTK4/Python, so its route is gettext, not Qt — a separate pipeline from the one proven below. |
| translated shell | **pipeline PROVEN, 5 of ~650 strings** | `tests/run-i18n-test.sh` — **12 assertions**. All four links run with the real tools on a shipped file: `qsTr()` marks 5 strings in `AgentHelpContent.qml`; `lupdate-qt6` extracts exactly 5 into the right context; `lrelease-qt6` compiles `translations/apex-shell_de.ts`; and a **running QML engine under `qmltestrunner -translation`** reads the German back off the live singleton. The load-bearing assertion is the sensitivity one — the SAME fixture runs with and without `-translation` and the two must DISAGREE; three independent strings are sampled so one lucky match cannot carry it. **Named gap, asserted so it cannot quietly stop being true: nothing in `src/` installs a `QTranslator`**, so the pipeline works and no user sees a translated word yet. |
| per-user language | not present | nothing per-user anywhere |
| non-US recovery / install flows | **install flow DONE, recovery not** | The "engine only COPIES whatever the live ISO resolved" finding is **no longer true for install**: the engine now prefers an operator's `keymap`/`keyvariant`/`timezone` over every inference, and the installer collects them before the password. `bib-config.toml:70-72` still hardcodes `us`/`en_US.UTF-8`/`Australia/Perth`, but those are now the FALLBACK rather than the only outcome. **Recovery is untouched** — nothing in the recovery flow asks for or applies a layout, so a non-US user recovering a machine still types on `us`. Not measured, and named as the remaining half. |

## ROUND 18 — the AT-SPI walk is DONE. The blocker was misdiagnosed.

**Rounds 1 and 2 recorded the end-to-end AT-SPI walk as not measurable on this
laptop, and roadmap.yaml's P2-003 evidence says the same. That is now false and
should not be repeated.** The walk runs, green, 30 assertions, 8 of 8 mutants
caught. What follows is the correction, because the reasoning that produced the
wrong answer is worth not repeating.

**What the predecessor saw:** `Activated service 'org.a11y.atspi.Registry'
failed: … Permission denied`, and concluded at-spi was unavailable in the
harness. The observation was real. The conclusion did not follow.

**What it actually is, measured three ways:**

1. The failure is in D-Bus **activation**, not in at-spi. Nothing needs
   activating: exec `at-spi-bus-launcher` and `at-spi2-registryd` directly and
   the whole stack comes up clean, in a private runtime dir, every time.
2. It is **not** the `SystemdService=at-spi-dbus-bus.service` line in
   `org.a11y.Bus.service` — the obvious culprit. Removing that line and
   activating anyway fails identically. A trivial service of our own in `/tmp`
   activates fine in the same bus, so activation itself works.
3. It is **SELinux**, and it is silent. A **byte-identical copy** of
   `at-spi-bus-launcher` placed in `/tmp` (label `user_tmp_t`) activates
   perfectly; the original (label `gnome_atspi_exec_t`) does not. No AVC is
   logged for it — `journalctl` and the kernel ring have none — which is
   consistent with a `dontaudit` rule. The mount carries no `noexec`
   (`ro,nodev,relatime`), so that is ruled out too.

**Consequence for anyone building on this:** never rely on D-Bus activation to
start the a11y bus on APEX. Exec the launcher. `tests/lib/atspi.sh` does.

## What could not be measured here, precisely (round 18)

The AT-SPI row is measured. Two things remain, and they are product gaps rather
than harness gaps — which is a different and more useful kind of "partial" than
the one the roadmap currently records.

**1. The shipped greeter has no bus to publish on.** This is the finding that
matters most from this round, and nothing was looking for it.

`greetd-config.toml` is not an example: `Containerfile.base:2114` copies it to
`/etc/greetd/config.toml`, and the live machine runs exactly it. Its command is

    command = "sway --unsupported-gpu -c /usr/share/apex-greet/sway-greet.conf"

There is no `dbus-run-session` anywhere in the chain, and `sway-greet.conf`
execs only quickshell. So the greeter session has **no D-Bus session bus at
all**. Measured consequence, not inference: an application that cannot resolve
`org.a11y.Bus` publishes **zero** nodes — that is mutant A7, and it is the same
condition the greeter ships in. The markup is right, the bridge works, the
password is safe, and a screen reader still receives nothing, because there is
nothing for the bridge to connect to.

The fix is NOT a one-line `dbus-run-session` wrapper: a session bus alone leaves
activation to fail for the SELinux reason above, so the a11y bus has to be
execed explicitly too. **Not attempted this round, deliberately.** The login
screen is boot-critical, greetd cannot be exercised headlessly here, and the one
load-bearing step is measured to fail under conditions close to production. A
change there should be made by someone who can boot an ISO and watch it.

**2. There is no screen reader in the image to connect to a bus.**
And the card's earlier claim that the image ships "ZERO accessibility packages"
is **wrong** — it was derived from grepping Containerfiles, which only finds
what is named explicitly. Asked of the image's own rpmdb
(`rpm --dbpath <deploy>/usr/share/rpm -q`) rather than the merged `/usr`:

| package | in the image? |
| --- | --- |
| `at-spi2-core` | **yes** (2.58.8) |
| `at-spi2-atk` | **yes** |
| `speech-dispatcher` | **yes** |
| `espeak-ng` | **yes** |
| `orca` | **no** |
| `brltty` | **no** |

So the plumbing and the speech engine are already there as transitive
dependencies of gtk4/Qt; what is missing is the reader itself. That makes the
gap a much smaller delta than "zero packages" suggested.

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

### Round 2c — the CI finding and the resume path (5 pairs)

| # | mutant | caught by |
| --- | --- | --- |
| K17 | engine stops recognising `Etc/UTC` | `a live environment on Etc/UTC is treated as unset` |
| K18 | the Perth fallback becomes unconditional | `a live environment on a real zone is carried into the install` |
| K19 | the suite's host redirect goes back to a blanket `sed` | `…while still WRITING its result into the deploy root` |
| K20 | the GUI stops writing `timezone=` to the state file | `the GUI's state file carries the time zone` + the §6b round trip |
| K21 | the restart loop guard is deleted | `pressing Continue on a resumed session moves ON to wifi` |

**K18 is the one that matters most.** Recognising `Etc/UTC` is only half a fix;
the other half is that a live environment which HAS already resolved a correct
zone must keep it. Without K18's assertion the fix could have degenerated into
"always Australia/Perth" and still passed every other check.

**K20 first survived, and that was a real gap.** §6b resumed from a hand-seeded
state file, so it measured the READ half only; the WRITE half lives in §5. Both
passed separately and nothing joined them, so a feature that wrote one spelling
and read another would have satisfied the suite. §6b now also resumes from the
file the real GUI produced in §6 and requires the zone it reads back to be the
zone it wrote.

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

**Round 1's pairs (M1-M7, N1-N8) were NOT re-taken.** The coordinator's
mid-round correction — that `cp -p` preserves the mtime and is as unsafe as `mv`
— matters where a compile step can be skipped. Those pairs are QML and shell,
which have no build artefact to go stale, so their verdicts stand. Round 2's
restores are `git checkout --` regardless.

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

## Housekeeping

The `claude-memory` MCP server returned 502 for this entire session, so the
session-close journal CLAUDE.md asks for could not be written. **This card is the
durable record of round 2.** Nothing else was lost; both branches are pushed.

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
- `installer/test-installer-keymap.sh` — **43 assertions** (CI run 34661483871 shows 42; it predates the round-trip assertion added in `d85c584f`).
- `installer/test-installer-locale.sh` — **26 assertions**.
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

**Final run 34661483871 — all four installer steps green on the runner**, and the
criterion is measured there rather than skipped:

    have: Xvfb / have: cage / have: python3-gi with Gtk4 + Adw
    PASS  the X11 backend really does put a keyboard on the seat
    PASS  cage started de: the same physical keys now produce the GERMAN letters
    installer-locale: 26 passed, 0 failed, 0 skipped
    installer-keymap: 42 passed, 0 failed, 0 skipped

That last-but-one line is the acceptance criterion running on a **second
machine**, not this laptop. Three earlier dispatches were needed to get there
and each failed for a different real reason — the `/var/empty` build context,
the `Etc/UTC` engine gap, and an apt line that would have skipped the
measurement while reporting success.

**The `Installer safety and UI` job had never run for this program at all** — it
is gated on `changes.outputs.installer`, and no roadmap branch had touched
`installer/` until this one. Turning it on surfaced two latent defects rather
than introducing them (the July-dead engine half, and the `/var/empty` build
context). The engine cases now PASS on the ubuntu runner, which is the first time
they have run anywhere but the ISO build box.

**Red jobs that are not this branch's**, proven by evidence rather than by
construction: run **34640150697**, at the predecessor's tip `69f59cb0` and
before this branch touched `installer/` at all, already shows
`Rust validation: failure`, `Package engine: failure` and
`Installer safety and UI: **skipped**`. So both were red before, and the
installer job genuinely had never run. Against its true branch point
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

## Round 18 — landed so far

apex-os, branch `task/p2-b-round18`, worktree `/var/tmp/apex-work/wt-p2-b2-os`,
pushed. apex-shell worktree `/var/tmp/apex-work/wt-p2-b2`, same branch name.
Both forked from `origin/roadmap/v2.2` AFTER round 2 landed (shell merge
`1417402`, os merge `f6eefd0d`), so round 2's work is underneath this.

- `tests/lib/atspi.sh` — a private accessibility bus: private session bus with
  an **empty service directory** (so nothing can be activated onto it — that is
  what stopped a desktop portal registering itself into the tree and
  fuse-mounting into the scratch dir), private a11y bus, real registry. Refuses
  to continue unless the bus is inside the directory it made.
- `tests/atspi-walk.py` — reads the tree back over D-Bus through Gio. pyatspi is
  absent here and from the image; "no binding" is not a reason to call a
  criterion unmeasurable.
- `tests/test-apex-greet-atspi.sh` — **30 assertions**, green.
- `tests/greet-atspi-app.qml` — the shipped surface in a real window.
- `tests/mutate-greet-atspi.sh` — **8 mutants, 8 caught**, tree verified against
  HEAD after every mutate and every restore.

### Three findings the QML-side suite could not have produced

1. **Qt 6.10.3 never publishes the AT-SPI `password text` role.** Measured four
   ways — explicit `EditableText` + `passwordEdit` (what the greeter does),
   `passwordEdit` alone, `echoMode` alone, and an explicit
   `Accessible.PasswordText` role — all arrive as plain `text`. A bridge
   limitation, not a greeter defect. Pinned as an equality so a future Qt that
   fixes it makes the suite say so.
2. **The password never crosses the bus.** Anything on an a11y bus can call
   `Text.GetText` on any node, so this was measured: the field is filled with a
   known secret and the bus returns the mask. The username field is asked the
   same question and DOES return its text, so the assertion can tell them apart.
   Mutant A2 (`echoMode` → `Normal`) turns it red — with masking gone the bus
   really does hand out the typed password.
3. **A reader can OPERATE the login screen, not just read it.** `DoAction`
   returns true even for a handler that does nothing, so the reply is not
   evidence; the effect is read back off the bus. Pressing "Next session" moves
   the session that would launch; pressing the layout pill changes the announced
   layout — P2-004's "keyboard layout before password", reached with no mouse
   and no sight.

### Two tooling defects found while building it

Both would have made assertions quietly meaningless, and neither is visible
without running the thing:

- `NActions` is a **property** on `org.a11y.atspi.Action`, not a method. Calling
  `GetNActions()` returned nothing, so every node reported an empty action list
  while `DoAction` on the same node worked. A suite written against that output
  would have asserted that a button offers no way to press it — and passed.
- A hand-built table of AT-SPI role numbers was wrong enough to call an editable
  text field `table-column` and a push button `panel`. Roles are now asked of the
  bus with `GetRoleName`. The enum has been renumbered across at-spi2 releases;
  a test that hardcodes one release's numbering asserts against the wrong
  vocabulary on every other.

### A harness claim that was wrong, and the mutant that exposed it

`atspi.sh` first said `org.a11y.Status.IsEnabled` was the production gate and
that setting it honestly was what made the suite meaningful. Mutant A7 flipped
it to false and **SURVIVED**. Measured rather than argued:

- the property **cannot be pushed false** — set `<false>`, read back `<true>`,
  because `at-spi-bus-launcher` reports the bus enabled once anything uses it;
- **Qt 6.10.3 does not consult it.** All four combinations of `IsEnabled`
  true/false and `AT_SPI_BUS_ADDRESS` exported/not still register.

The real gate is **reachability**. A7 now takes the session bus away from the
application, which is a real lever and the exact condition the shipped greeter
runs in. A second correction fell out of it: the suite had been exporting
`AT_SPI_BUS_ADDRESS` into the application under test — a shortcut no real
application gets, and the reason A7 survived even after being rewritten. The
surface now has to resolve `org.a11y.Bus` on the session bus the way every
desktop application does; the walker keeps the variable, because the walker is
the assistive technology and a reader IS told where the bus is.

## ROUND 18b — what landed, and the verdicts

Branch `task/p2-b-round18b`, worktree `/var/tmp/apex-work/wt-p2-b3-os`, pushed.
Forked from `origin/roadmap/v2.2` and **merged `origin/task/p2-b-round18` into
it** (`2d4d0d26`), because round 18 had NOT reached `roadmap/v2.2` — the
orchestrator should merge `round18b` and needs no separate merge of `round18`.

**Correcting the dispatch note:** the agent before this one did not "push
nothing". `39f20442` — the whole installer accessibility audit, 38 assertions —
was on `origin/task/p2-b-round18` already. What it lost was
`installer/mutate-installer-a11y.sh`, sitting uncommitted in its worktree. That
harness was recovered and committed first, before anything else was attempted.

| commit | what |
| --- | --- |
| `eec6507f` | the recovered installer mutation harness, 6 mutants |
| `cb9254f7` | `tests/test-apex-greet-session-bus.sh` (16) + its 6 mutants |
| `c688de74` | `orca` into `Containerfile.core` + `tests/test-apex-a11y-stack.sh` (13) + 4 mutants |
| `db16b410` | the installer audit's page-advance section and the `confirm` page (38 → 50) |
| `6805174a` | all three suites into `pr-validation.yml`, and the job-selector defect below |

### Mutation verdicts

| set | result |
| --- | --- |
| `installer/mutate-installer-a11y.sh` | B1-B8 at `db16b410`: **8 applied, 8 CAUGHT, 0 survived**, tree matches HEAD. (B2's first run at `eec6507f` reported SURVIVED; that was the harness's expectation, not the mutant — see below.) The suite has changed since, twice, for defects CI found; the B set has NOT been re-run against `73f158d2`. Nothing in those changes touches a mutated arm, but that is an argument, not a measurement. |
| `tests/mutate-greet-session-bus.sh` | C1-C6, 6 applied, **6 CAUGHT**, 0 survived |
| `tests/mutate-a11y-stack.sh` | D1-D4, 4 applied, **4 CAUGHT**, 0 survived |

**B2 was first reported SURVIVED, and that was the harness's fault, not a
survival.** Unnaming the Wi-Fi password removes the very node the wifi page's
sentinel waits for, so the page is never recognised as built and the per-page
audit is never reached — the suite failed earlier and louder, on three named
assertions. The harness was grepping for a sentence the failure does not
contain. Identical in shape to I2 in the i18n round. The expected assertion is
corrected, not the mutant.

**C5 and B5 are the two that matter.** Both are vacuity floors: B5 empties the
set of roles the audit considers interactive, so "every focusable control
announces itself" becomes trivially true of nothing; C5 breaks the session-bus
probe so it answers "no bus" whatever it is given, which would leave four
absence assertions green and meaningless. Both caught.

**C6 proves the session-bus suite flips.** It execs `at-spi-bus-launcher` from
the sway host config the way `tests/lib/atspi.sh` does, and "a session bus alone
is not enough: the greeter chain starts no accessibility bus" goes red. So the
suite really is a statement of current state and not a constant.

### CI, round 18b

Dispatched rather than guessed at: `gh workflow run pr-validation.yml --ref
task/p2-b-round18b`, run **34673525673**.

- **`Package engine` — both new suites GREEN on the runner.** `Run greeter
  session-bus assertions` and `Run accessibility-stack assertions` both pass on
  ubuntu-24.04. That job's red is `Run file-injection assertions`
  (`tests/test-agent-inject.sh`), the KNOWN cgroup failure this card already
  records: the hosted runner's connection sits in
  `system.slice/hosted-compute-agent.service`, which is neither a login session
  nor a user service, so origin detection cannot classify it. Red on
  `roadmap/v2.2` itself (run 34672131214) before this branch existed.
- **The installer audit ran on the runner and went RED, and both reds were real
  defects in the SUITE that this laptop could not show.** `44 passed, 4 failed`.
  This is the second time in this unit that CI has earned its keep.
  1. **The `wifi` page has TWO shapes and the audit knew one.** `wifi_available()`
     asks `nmcli -t -f TYPE device` for a line reading exactly `wifi`; with no
     adapter — every CI runner — the installer builds a different page: an
     explanation and two buttons, no password field, no hidden-network field.
     The audit waited the full 40 s for a sentinel that could never appear and
     then reported `page 'wifi' builds and reaches the accessibility bus` as
     FAILED, which reads as a defect in the installer on a page that was working
     perfectly. Wrong in the most expensive direction. Both shapes are now
     audited, the shape that was measured is CONFIRMED against the tree rather
     than assumed, and the two field-name checks SKIP with the reason where the
     fields are not built — a field that does not exist is not a field that is
     unnamed.
  2. **`xdotool type` dropped every character on the runner, and the password
     assertion passed anyway.** The username field read back EMPTY after a type
     that reported success. The masking assertion — "the password the user typed
     never crosses the accessibility bus" — then passed about an EMPTY FIELD.
     That is a false green of exactly the N8 shape, and only a second machine
     could produce it. Fixed in both halves: `--delay 40` and a bounded
     read-back retry, and the masking assertion is now GATED on the read-back
     having proved the typing landed, skipping with the reason if it did not.
- **The page-advance section passed on the runner** — 8 of 8, both Return and
  space — so the newest work is measured on two machines, not one.
- **`tests/test-apex-greet-atspi.sh` is STILL local-only** — this unit has
  produced four accessibility suites and three of them now run in CI. The AT-SPI
  walk needs a wlroots compositor (`labwc`/`sway`) as well as a qml runtime and
  the at-spi stack, so wiring it into the Fedora-container step is a real piece
  of work rather than one more line, and it was not attempted blind. Its 30
  assertions have been verified on ONE laptop. Do not read "CI green" as
  covering it.
- **`Static validation` failed for a reason that is purely the branch NAME**,
  and it is worth knowing about because it will bite every future task branch.
  `Input page and generator agree` clones **the apex-shell branch with the same
  name**, and falls back to apex-shell's DEFAULT branch when there is none — and
  the step's own comment records that the two `main`s have known touchpad drift
  (`drag_lock`, `click_method`). So any apex-os task branch without a
  same-named apex-shell branch fails that step on drift that is not its own. The
  fix taken here: create `task/p2-b-round18b` in apex-shell too, off
  `origin/roadmap/v2.2`, carrying no commits. **That branch exists only to make
  the parity check compare like with like; merging it is a no-op.** Proved
  rather than argued: the next dispatch, run **34673943974**, has `Static
  validation` GREEN with no change to any file that step reads.

### Two things found that nothing was looking for

1. **The page-advance hole.** The installer audit's section titled "the keyboard
   alone can fill the page in **and move on**" did not move on. A primary button
   can be reachable, named, announced perfectly and completely INERT to the
   keyboard; the user is stuck on step 1 of 7 with a mouse the criterion says
   they do not have, and every assertion stays green. Mutant B7 is exactly that
   defect and it is now caught. This is K11 one layer lower: there the forward
   edge existed in the page graph and could still have left the flow, here the
   edge exists and the key press may not travel it.

2. **`Containerfile*` selected no CI job at all** — the SIXTH instance of a bug
   `pr-validation.yml` already documents five times. A PR touching only
   `Containerfile.core` set `rust=false, installer=false, engine=false`, ran
   nothing and passed, because a skipped job counts as success. Deleting `orca`
   from that file and touching nothing else was precisely the change that would
   have sailed through the assertion written to prevent it.

### The negative control that could not fail, caught in review

`test-apex-a11y-stack.sh` first used `ibus` — a word appearing only in
`Containerfile.core`'s prose — as proof that the package extractor ignores
comments. That control **cannot fail**: no comment in that file carries a `dnf5
install` line for a broken parser to read a package out of, so the assertion
held however broken the parser was. It is now a FIXTURE with a known right
answer (a live install line, a commented-out one, and a trailing-`#` comment on
a live line), and mutant D4 turns it red.

### Cost of the orca change, measured not estimated

`dnf5 install --assumeno orca` inside `ghcr.io/andrenijman/apex-os:daily`
(6 weeks old; say so rather than imply a fresh number): **5 packages** — orca,
brlapi, python3-brlapi, python3-louis, **python3-pyatspi** — **4 MiB
downloaded, 23 MiB installed**. Nothing pulls `speech-dispatcher` or
`espeak-ng`, which is the rpmdb confirming they were already there. Deliberately
**not autostarted**: a reader that starts unbidden talks over a sighted user's
first boot, and at the greeter it would have nothing to talk to until the
session-bus hole is closed.

## NEXT

Ordered. 1-4 are accessibility (P2-003) and are what this unit should do next;
5 onward are internationalisation (P2-004) and are inherited unchanged — **round
18b did no P2-004 work at all**, because its three plan items were all P2-003.
Rounds 18 and 18b closed the previous items 1-3.

1. **Give the greeter a session bus, and exec the a11y launcher beside it.**
   This is now the single largest thing standing between APEX and "screen reader
   validated", and it is fully characterised: `tests/test-apex-greet-session-bus.sh`
   states the current hole on BOTH hosts and goes red the moment it is closed;
   mutant C1 shows what closing it looks like on the sway host, C4 on the labwc
   one, C6 shows the accessibility-bus half. A one-line `dbus-run-session` is
   NOT the fix — on an SELinux system `org.a11y.Bus` cannot be D-Bus activated
   at all, measured again this round (EACCES, no AVC), so the launcher must be
   execed explicitly the way `tests/lib/atspi.sh` does. **Still deliberately not
   attempted here:** the login screen is boot-critical, greetd cannot be
   exercised headlessly on this laptop, and this should be done by someone who
   can boot an ISO and watch it come up.

2. **A way to START orca.** The image now ships it and nothing launches it,
   which is correct (a reader that starts unbidden talks over a sighted user's
   first boot) but incomplete: there is no keybinding and no settings toggle, so
   a blind user has no way to turn it on without sighted help. GNOME's
   convention is Super+Alt+S. The keybindings live in
   `/usr/share/apex/hypr/apex/keybindings.lua`; `tests/test-apex-a11y-stack.sh`
   is where the assertion belongs, beside the "nothing autostarts it" one.

3. **`tests/test-apex-greet-atspi.sh` into CI.** The only accessibility suite in
   this unit still verified on one machine. It needs `labwc` or `sway` plus a
   qml runtime and the at-spi stack in the Fedora container the greeter a11y
   step already uses; follow that step's `exit 97` pattern so a container that
   cannot resolve the packages reports "did not run" rather than reddening the
   job. Worth doing before anything else in this list: the round-18b CI dispatch
   found two false greens in the installer audit that a laptop could not, and
   this suite has never had that treatment.

4. **The Tab ring is walked on ONE page.** `installer/test-installer-a11y.sh`
   walks `account` (most fields, both passwords) and advances welcome →
   keyboard. Every other audited page is name-audited only, so a focus trap on
   `wifi`, `secureboot` or `confirm` would not be seen. `advance_with` and the
   ring walk are both parameterised enough to extend; the cost is runtime, about
   a minute per page.

5. **The `sections` array in `AgentHelpContent.qml`** (~200 prose strings) is the
   obvious next i18n increment; the pipeline is proven. One caution measured the
   hard way — `tests/check-agent-help.sh` greps for the exact shape
   `{ k: "kv", t: "$m"`, so wrapping those `t:` values in `qsTr()` breaks it;
   that suite and this conversion must move together.
6. **Nothing installs a `QTranslator`**, so the shell's translation pipeline
   reaches no user. `run-i18n-test.sh` asserts the absence, so the row flips
   itself when somebody wires it up. Worth doing BEFORE item 5: 200 translated
   strings nobody can see is the weaker increment.
7. **Locale is still not offered by the installer, deliberately.** The image
   installs `glibc-langpack-en` ONLY (`Containerfile.core:809`), so a free picker
   would let a user choose a locale that silently degrades to `C.UTF-8`. Add
   langpacks first, then a picker restricted to what the target ships.
8. **The installer is GTK4/Python**, so its translation route is gettext, not the
   Qt pipeline proven here. Separate work.
9. **CJK: measure before fixing.** The card claims apex-shell "will tofu"
   because ~45 sites hardcode `font.family: "JetBrains Mono"`. That was never
   run. Qt does fontconfig fallback at the QFont level, so the claim may be
   false; a `TextMetrics`/`FontMetrics` probe under the headless harness settles
   it in minutes.
10. Remaining accessibility gaps, unchanged: `src/popups/` and
   `src/nexus/NavPane.qml` still use bespoke Rectangle+MouseArea and are
   mouse-only and unnamed; no Arabic/Hebrew/Thai fonts, so RTL input from
   `fcitx5-m17n` cannot be rendered. The `disk`/`mode`/`part` installer pages
   are not audited either, because they enumerate real block devices.
11. **`Xvfb` and `xdotool` are the tools that make the installer criterion
    measurable.** Both are now installed in CI's installer job as well as on this
    laptop. If a future runner lacks them the suite SKIPs rather than lying — but
    a skip there means the criterion is unmeasured, not met.

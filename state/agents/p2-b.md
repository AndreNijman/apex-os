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
| screen reader | | |
| magnifier | | |
| high contrast | | |
| reduced motion | | |
| large text | | |
| colour filters | | |
| sticky keys | | |
| slow keys | | |
| mouse keys | | |
| on-screen keyboard | | |
| keyboard-only installer | | |
| accessible login/lock/recovery | | |

### P2-004 — internationalisation

| sub-feature | state | assertion |
| --- | --- | --- |
| keyboard layout before password | | |
| multiple layouts | | |
| IME / fcitx5 | | |
| CJK | | |
| RTL | | |
| locales / timezones | | |
| translated installer | | |
| translated shell | | |
| per-user language | | |
| non-US recovery / install flows | | |

## Mutation pairs

(each new assertion, the arm that was changed, and the named test that went red)

## NEXT

1. Two read-only surveys are running over both trees (a11y inventory in
   apex-shell; i18n inventory in both). Fill the ledger's "already present"
   column from them rather than from assumption.
2. Build the greeter fixture in apex-os: stage `GreetSurface.qml` next to theme
   and ctx stubs, run under `qmltestrunner -platform offscreen`, measure
   accessible names, Tab focus order, and whether anything precedes the password
   field that names the keyboard layout.
3. Mutation-prove, commit, push, update this card.

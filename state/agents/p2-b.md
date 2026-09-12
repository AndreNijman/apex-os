# p2-b — accessibility baseline (P2-003) and internationalisation baseline (P2-004)

**ROUND 20 IS THE LIVE ONE, on the SAME branch.** `task/p2-b-round19` in BOTH
repos, pushed — round 20 continued on it rather than renaming, and merged
`origin/roadmap/v2.2` into it first (both fast-forwards; round 19 had landed as
apex-os `4b1e797f` and apex-shell `111ed75`). apex-os tip `d12e9ef4`, apex-shell
tip `eb97b40`. Worktrees unchanged: `/var/tmp/apex-work/wt-p2-b4` and `wt-p2-b4-sh`.

- apex-os: worktree `/var/tmp/apex-work/wt-p2-b4`, forked from `origin/roadmap/v2.2`
  and carrying a merge of it (`3541603d`) taken mid-round because v2.2 moved under
  the branch — `d85f7edb` added a RUN immediately after the greetd-config COPY,
  where this round adds its own. One conflict, resolved by keeping both.
- apex-shell: worktree `/var/tmp/apex-work/wt-p2-b4-sh`, and unlike round 18b's
  it carries a REAL commit (the screen-reader keybind default), not just a name
  for the parity check.

**Round 19 closed the previous `## ROUND 20 — the translator gap is in the HOST, the CJK claim was false, and the
## installer mutation set is re-taken

Branch `task/p2-b-round19` in BOTH repos again — continued, not renamed — each
merged with `origin/roadmap/v2.2` before anything else (both were
fast-forwards: round 19 had already landed as apex-os `4b1e797f` and apex-shell
`111ed75`). Worktrees unchanged: `/var/tmp/apex-work/wt-p2-b4` and
`wt-p2-b4-sh`.

### FIVE THINGS NOTHING WAS LOOKING FOR

**1. The QTranslator gap is in the HOST, and `src/` was the wrong place to
look.** Every previous round recorded it as "no QTranslator anywhere in
`src/`" — a grep over QML. The observation was right and the spelling invited
the wrong repair, because it reads like an omission somebody could fix by
writing a line of QML. **They cannot: QTranslator is a C++ class and is not a
QML type**, so no file under `src/` can install one however it is written.

Measured over dynamic symbol tables, each claim with its positive control in
the same run:

| binary | QTranslator / installTranslator | QQmlApplicationEngine | QQmlEngine ctor |
| --- | --- | --- | --- |
| `libQt6QuickTest.so.6` (the host section 3 watches translate) | **4** | — | — |
| `/usr/bin/quickshell` (the host the shell runs in) | **0** | **0** | 1 |
| `libQt6Qml.so.6` | defines `_q_loadTranslations`, 44 `QQmlApplicationEngine` symbols | — | — |

So Qt's automatic route is real — a `.qm` beside the root QML file is loaded
with no code at all — and it belongs to `QQmlApplicationEngine`, which
quickshell never constructs. **Closing the row needs a translator installed
INTO the engine: upstream in quickshell, or by a QML extension module on the
import path whose `initializeEngine()` installs one.** Not attempted: that is a
compiled artefact in a repository of QML and shell scripts, and it changes the
image build.

Two traps met while measuring, both left as comments because both produce a
number that looks like a measurement:

- **Undefined symbols only.** `libQt6Qml` both defines QTranslator's caller and
  is linked by every QML host on the machine, so a scan of the LINK CLOSURE
  reports "found" for a host that never calls it. Checked rather than assumed:
  the closures of quickshell and qmltestrunner both contain those references.
- `/libQt6Qml/` also matches `libQt6QmlMeta`, `libQt6QmlModels` and
  `libQt6QmlWorkerScript`, and `nm` handed three paths at once answers 0 to
  everything while still looking like a measurement.

**2. "The shell will tofu on CJK" was never run, and it is false.** The ledger
had carried it since round 1: ~45 sites write `font.family: "JetBrains Mono"`,
that family has no CJK coverage, therefore tofu. Both premises are true and the
conclusion does not follow — Qt does not stop at the family it was handed, and
`QFontEngineMulti` asks fontconfig for a font that owns the glyph. Measured
through the real engine at `font.pixelSize: 32`:

| script | advance under JetBrains Mono | matched |
| --- | --- | --- |
| CJK 漢 | 32 | a font covering U+6f22 (**and 32 on the GitHub runner too**, Noto Sans CJK HK) |
| Arabic ا | 19.265625 | DejaVu Sans Mono |
| Hebrew א | 21.390625 | DejaVu Sans |
| Thai ก | 19.75 | Droid Sans Thai |
| Devanagari अ | 24.453125 | Droid Sans Devanagari |

JetBrains Mono's own advance is 19.1875 for every glyph it draws, asserted
first, because the whole argument rests on it. **So the RTL row's "the image
cannot render it" is wrong as it stands too.** What is still absent is
LAYOUT — 0 `LayoutMirroring` and 0 `layoutDirection` in `src/`, checked. Fonts
were never the blocker there.

**3. apex-shell CI was red on `roadmap/v2.2`, and the red hid the i18n step.**
`Lint test harness scripts` ran `shellcheck -S warning tests/*.sh` — without
`-x`. Without it shellcheck cannot follow `. tests/lib/headless.sh`, so every
variable a suite sets FOR the library to read is dead code to it.
`tests/test-headless-lib.sh` landed from `task/followups-3` with four such
assignments and the step has been red ever since. The house gate is
`shellcheck -S warning -x`; CI was running something else and failing on the
difference — measured on one tree, rc=1 with 9 findings its way, rc=0 with
`-x`. A job stops at its first red step and this one sits above
`Internationalisation baseline`, so **an SC2034 about a test fixture switched
off the measurement below it**. Same shape as round 19's finding 4 in apex-os.
Fixed on this branch and proved in both directions: a real SC2034 injected into
another harness still exits 1.

**4. The confirmation page's erase button is not a tab stop, and that is
correct.** The new per-page ring walk went RED on `confirm`: two stops, `Back`
and the ERASE field, where the audit counts three named controls. That is not a
keyboard trap — `apex-installer-gui` builds `go = self.btn("Erase and install",
…, False)` and only `e_erase`'s `changed` handler turns it on, when the text is
exactly `ERASE`. An insensitive GTK button is not a tab stop. **The floor was
the wrong assertion**, and the right one is stronger in both directions: the
destructive button must NOT be reachable before the word is typed — a keyboard
user must not be able to land on an irreversible action without confirming
it — and it MUST be reachable after.

**5. `shellcheck` does not catch an undefined function, and the failure it
produced looked like a product defect.** Moving `reach`/`type_text`/`type_into`
up the file so the confirm walk could use them deleted them and did not re-add
them. `shellcheck -S warning -x` passed, `bash -n` passed, the suite ran to
completion and reported `the account fields can all be reached with the
keyboard alone — only 0 of 3 were reachable`, which reads exactly like a
regression in the installer. Caught by reading the totals line and the diff
rather than the exit code.

## NEXT

Ordered, and rewritten after round 20. Items 1-2 are unchanged and still need
hardware. **The old item 6 — "nothing installs a QTranslator" — has been
measured and is now item 3, with a different answer than the one it implied.**

1. **The reader at the login screen, end to end, on real hardware.** Everything
   around it is asserted: the greeter has a session bus, an accessibility bus
   and a registry on both hosts; SUPER+ALT+S is bound on both; the tree is
   readable and operable over AT-SPI on three machines. What is NOT proven, and
   cannot be here: orca running as the `greetd` system user — a writable HOME,
   and speech-dispatcher with no audio session — and whether SELinux permits the
   greetd session context to exec `gnome_atspi_exec_t` (`ps -Z -C greetd` gives
   the domain; `sesearch -A -s <domain> -t gnome_atspi_exec_t -c file -p execute`
   answers it where setools is installed). If the exec is refused the wrapper
   degrades safely — that IS asserted — so the failure mode is a silent
   no-reader, not a broken login. **Needs someone who can boot an ISO and
   listen.**
2. **Greeter audio.** Falls out of item 1: even a perfectly started orca says
   nothing without a sound path for the greetd user. Nothing here configures
   one.
3. **A QTranslator needs a HOST change, and no amount of QML will do it.**
   Round 20 measured this rather than grepping for it, so the next round does
   not spend a morning in `src/`: QTranslator is a C++ class and is not a QML
   type; `/usr/bin/quickshell` calls neither it nor `installTranslator`; and it
   constructs a bare `QQmlEngine`, never the `QQmlApplicationEngine` whose
   `_q_loadTranslations` is the one route that loads a `.qm` with no code at
   all. Two ways out, both real work:
   - **upstream** — quickshell gaining translation support, which also fixes it
     for every other quickshell config on earth;
   - **a QML extension module** on the import path whose `initializeEngine()`
     installs a QTranslator. `QQmlEngine::addImportPath` is in quickshell's
     symbol table so the engine would honour it. This is a compiled artefact in
     a repository of QML and shell scripts, and it changes the image build —
     cost it before starting.
   `tests/run-i18n-test.sh` asserts the host facts, so if quickshell ever gains
   the call the suite says so instead of going quietly green.
4. **The `sections` array in `AgentHelpContent.qml`** (~200 prose strings) stays
   PARKED and the reason is now measured rather than a preference: with item 3
   open, 200 more translated strings reach exactly as many users as the 5 that
   exist — none. Do item 3 first. When it happens, the caution from round 2
   still holds: `tests/check-agent-help.sh` greps for the exact shape
   `{ k: "kv", t: "$m"`, so wrapping those `t:` values in `qsTr()` breaks it,
   and that suite and this conversion must move together.
5. **RTL is a LAYOUT gap, not a font gap.** Round 20 killed the font half:
   Arabic, Hebrew, Thai and Devanagari all fall out of the hardcoded JetBrains
   Mono into image-owned fonts that cover them, measured through the engine. The
   half that stands is `0 LayoutMirroring` and `0 layoutDirection` in `src/` —
   nothing mirrors. That is a real piece of work and it is now the whole of this
   row. (The Noto families are still absent, which is a typographic quality
   question and not a tofu one.)
6. **The Tab ring now runs on four installer pages; `wifi` is the one audited
   page still unwalked.** It has two shapes and which one is built depends on
   the machine, so the walk needs the same shape detection `audit_page` already
   does. `disk`, `mode`, `part`, `run` and `done` are not audited at all,
   because they enumerate real block devices and start real installs.
7. **Locale is still not offered by the installer, deliberately.** The image
   installs `glibc-langpack-en` ONLY (`Containerfile.core:809`), so a free picker
   would let a user choose a locale that silently degrades to `C.UTF-8`. Add
   langpacks first, then a picker restricted to what the target ships.
8. **The installer is GTK4/Python**, so its translation route is gettext, not the
   Qt pipeline proven here. Separate work.
9. **What CI does NOT cover for this unit, named so it is not read as covered.**
   - `tests/run-i18n-test.sh` section 4 — the quickshell symbol probe — SKIPS on
     the Arch runner, which has no quickshell. **One laptop only.**
   - section 5's Arabic/Hebrew/Thai/Devanagari rows SKIP there too: that
     container has no font covering them, and the suite says so rather than
     failing (it fails only where `/run/ostree-booted` says the font set being
     measured IS the image's). Only the CJK row has a second machine.
   - `tests/test-apex-greet-atspi.sh` runs in CI as of round 19; read the run's
     totals line, never its tick — every suite here exits 0 on a tool SKIP.
   - **apex-shell `roadmap/v2.2` is red until this branch merges.** Its
     `Lint test harness scripts` step ran shellcheck without `-x` and failed
     `tests/test-headless-lib.sh`; the fix is on this branch. Until it lands,
     that step hides the i18n and accessibility steps under it.
10. Remaining accessibility gaps, unchanged: `src/popups/` and
    `src/nexus/NavPane.qml` still use bespoke Rectangle+MouseArea and are
    mouse-only and unnamed. No magnifier, sticky keys, slow keys, mouse keys or
    on-screen keyboard anywhere in either repo. Lock and recovery screens are
    not measured; only login is.
11. **`Xvfb` and `xdotool` are what make the installer criterion measurable.**
    Both are in CI's installer job as well as on this laptop. If a future runner
    lacks them the suite SKIPs rather than lying — but a skip there means the
    criterion is unmeasured, not met.

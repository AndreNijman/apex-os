# P1 — progress tracker

**This file is the resume point.** If a session dies, read this first: it says
what is done, what is half-done, and where the next commit starts. It is
updated *in the same commit* as the work it describes, so it can never drift
more than one commit from reality.

P1 is rows 5–8 of the roadmap's own implementation order
(`APEX-OS_Full_Roadmap.pdf` §23):

| Phase | Roadmap rows | Branch |
|-------|--------------|--------|
| 5 | §16 plugin platform, §17 compositor-independent desktop | `p1/compositor-and-plugins` |
| 6 | §8 capsules, §9 universal package resolver | `p1/capsules-and-packages` |
| 7 | §10 declarative blueprint + sync | `p1/blueprint-and-sync` |
| 8 | §11 modes, §12 gaming mode, §13 workload manager | `p1/modes-and-workloads` |

## apex-shell is landing on `main` — 2026-09-03

Merging apex-shell triggers no image build (`release-shell` is dispatch-only,
`build-image` lives in apex-os), so the shell half of P1 lands as it is
finished. `Containerfile.base` vendors the shell from remote `main`, so this is
the documented merge order rather than a shortcut.

| PR | What | State |
|----|------|-------|
| #8  | §17 compositor adapter | merged `30d1801` |
| #9  | §16 plugin platform | merged `f2908f9` |
| #10 | §10 blueprint GUI editor | merged `3434c66` |
| #12 | two more extension points | merged `099c44c` |
| #11 | 5.2 finished + review leftovers | green, **held** |
| #13 | niri application keybinds | green, **held** |

#11 and #13 are held only because `p1/idle-and-stats` is still being built on
top of #11. Merging a base mid-flight orphans the branch above it — that already
cost one rebase today when #9 landed under #12.

**Merging #9 immediately unblocked apex-os PR #34**, whose `Package engine` job
failed on a single labelled assertion: the CLI takes its plugin verdicts from
apex-shell's `manifest.js`, and CI clones the shell from `main`, where it did not
yet exist. The agent predicted that precisely, and the job went green on re-run
with no change on either side. The vendoring coupling working as designed.

## Standing instruction from Andre — 2026-09-03

> "for the rest never ask questions just finish autonomously"

Combined with an explicit "add whatever you want" for the idle-inhibit work and
"put system stats wherever you think it should be". So: **make the judgement
calls, do not stop to ask.** Report decisions and their reasoning afterwards
rather than seeking approval beforehand.

The two things that remain non-negotiable regardless, because both have already
cost him something real: never mutate his live session or anything under his
`~/.config`, and never let a suite pass without being able to fail.

## Working rules for this stack

- **Every branch is stacked on the previous one**, and the bottom of the stack
  is `feat/input-display-parity` (P0, PR #24, unmerged on purpose). Nothing is
  merged to `main` until Andre asks for the single final image build — a push
  to `main` touching `files/**`, `apexd/**` or `config/**` triggers
  `build-image`, and he asked for exactly one of those at the end.
- **Each phase opens a PR against `main`** even though it will not be merged
  there yet. `pr-validation.yml` only fires on `pull_request: branches: [main]`,
  so a PR aimed at the parent branch would get no CI at all. The diff carries
  the parent's commits too; that is noise, not error.
- Because the branches are stacked, merging **only the tip** at the end lands
  every phase and produces **one** build.
- **Commit and push every logical step.** An unpushed commit is not durable.
- apex-shell is a separate repo with no image build on push, so shell phases
  merge to `apex-shell/main` as they go. `Containerfile.base` vendors the shell
  from remote `main`, so shell must land before or with the OS side.

## The three deferred P1 items — DONE

Branch `p1/deferred-items`, off `origin/p1/integration` (which has all four P1
phases merged, built and tested together). Three items that were each deferred
for the same reason — they depended on work on a *parallel* branch — and are
therefore closed together now that the branches are merged.

| Item | Was deferred because | Closed as |
|------|----------------------|-----------|
| §8 GUI export from capsules | "§8 says 'when useful'; needs a real desktop to verify" | `apex env export / unexport / exports` |
| §10 `[development] languages` convergence | "a language→package table here would conflict at integration" | `Step::ProvisionLanguage` → `apex env provision` |
| §16 `apex plugin` CLI | apex-shell PR #9 shipped the platform and left the OS side out | `apex plugin list / info / enable / disable` |

### 1. §8's GUI export

`distrobox-export` runs **inside** the container — it refuses to start anywhere
else — and reaches back through `/run/host` to write the `.desktop` file into
the host's `~/.local/share/applications`. So the host side is
`distrobox enter --no-tty <capsule> -- distrobox-export --app <name>` and there
is no host-side program to call instead. Nothing needs root, and
`update-desktop-database` runs against the user's own directory, so this path
cannot raise an authentication prompt.

Three decisions worth keeping:

- **The host-side filename is recorded, never derived.** distrobox names the
  file after the `.desktop` it found *inside* the capsule, so
  `apex env export py gimp` produces `py-org.gimp.GIMP.desktop`. `export`
  snapshots the launcher directory before and after and records what appeared.
  The test fixture deliberately uses a name the naive derivation does not
  produce — with a matching fixture the claim would be untestable, and a
  derived-name mutant did in fact survive until that was fixed.
- **`rm` deletes those recorded names and never a `<capsule>-*.desktop` glob.**
  Capsule `shell` and capsule `shell-x` are both valid names and the glob for
  the first matches the second's entries. The suite has a capsule pair proving
  it.
- **Only bare application names are accepted.** `--app` also takes an absolute
  path to a desktop file, which would let a caller name any `.desktop` inside
  the capsule and produce a host filename the script cannot predict or check.

**What a desktop session would still be needed for, stated plainly:** that the
exported entry appears in a running launcher, that its icon resolves on the
host, and that clicking it starts the application. Every assertion here is
about the argv, the recorded state and the refusals.

### 2. §10's `[development] languages`

A declared language is satisfied two ways, and the order matters:

1. **The toolchain is already on the host's `PATH`.** The APEX images ship a
   full dev stack (gcc, g++, python3, node, cargo, golang, bash), so this is the
   *common* case. Reading it as drift would provision seven multi-gigabyte
   capsules to duplicate software the image already has — the "reformats a
   machine the first time it runs" failure `Blueprint`'s own doc comment exists
   to prevent.
2. **A capsule records itself as providing it.** This is what `apply` creates,
   and it is where §8 wants a toolchain.

Neither, and there is a step per language — not one for the list, because each
language maps to its own capsule and a partial failure has to leave the others
converged.

The step carries the **language** and never a capsule name: `c`/`cpp` share one
toolchain and `javascript`/`typescript` one runtime, so which capsule provides
what is the engine's decision. That is exactly the second answer phase 7
deferred the section to avoid. The table lives only in `apex-env`; the planner
has the vocabulary, which it needs to validate a blueprint before any engine
exists.

`provision` records a language only after running the language's probe **inside**
the capsule. `dnf -y install` exits 0 for a package set that puts nothing on
`PATH`, and recording on the strength of that exit code is the "exited 0 having
changed nothing" case `apexd/AGENTS.md` forbids. For the same reason `create`
records no language even for the `python` alias, whose `--additional-packages`
are installed by distrobox during container setup and can partly fail while
leaving the container present.

**A behaviour change on a shipped command, called out because it looks
identical to the feature working:** `apex blueprint diff` now exits 1 for a
missing language. Phase 7 shipped the section with `step: None`, so it reported
CANNOT CONVERGE and exited 0 forever.

Two couplings that would otherwise drift silently, both now checked:

- The record directory follows the engine's precedence exactly
  (`APEX_ENV_HOME`, then `XDG_DATA_HOME`, then `$HOME/.local/share`). If writer
  and observer disagreed, `apply` would provision a capsule the next `diff`
  could not see — and it would show up first in the isolated-HOME test, where it
  reads as a broken test rather than a broken path. The suite's engine stub
  writes the record precisely so that assertion is real.
- `files/scripts/check-language-parity` compares `apexd-core`'s `LANGUAGES` with
  `apex-env`'s, and runs `apex-env languages` to assert every row resolves to a
  capsule, packages **and** a probe. It is in the **`static`** job on purpose:
  `rust` fires on `^(apexd/|…)` and `engine` on `^(files/|tests/)`, so a check in
  either specialised job would be skipped by a PR that touched only the other
  side — and a skipped job counts as success. That is the same bug this file
  already records for `test-apex-modes.sh`.

`APEX_ENV_ENGINE` is new and overridable, which is the *opposite* of the rule
`Containerfile.base` asserts for `apex-pkg`'s `readonly ENV_ENGINE`. The
reasoning there ("a caller-controlled variable naming a program a root process
executes is a hole") is right and does not apply here, structurally:
`ProvisionLanguage` is user-domain and `perform()` refuses a step from the other
domain before it builds any path or spawns anything, so the variable can only
name a program the invoking user could already have run. What it buys is a live
`apex apply` that exercises the real convergence path — the engine is reached by
absolute path, so no `PATH` faking intercepts it and a user-domain step has no
domain filtering to fall back on. It is the sibling of `APEX_WINDOW_ADAPTER`.

**Unverified:** a real `apex env provision` against live podman. No capsule was
created; the path is exercised end to end against a recording stub.

### 3. §16's `apex plugin`

**It owns no plugin rules.** Every verdict comes from apex-shell's own
`src/services/plugins/manifest.js`, `require`d by a node shim — the same file
the QML engine imports and the same file apex-shell's tests load. `nodejs` is in
`Containerfile.core` and the shell is vendored to `/usr/share/apex-shell`, so
this works in the image. If manifest.js is absent, `list` and `info` **refuse**;
a fallback would be the second, drifting answer the design exists to prevent,
appearing on exactly the machines where the shell was installed wrong.

The suite proves that differentially rather than trusting it: for each fixture it
asks manifest.js directly and compares the reason code. A shim that invented
`manifest-invalid` instead of passing `manifest-unparseable` through fails ten
assertions.

**The duplication that could not be avoided, and its tripwire.** Four refusals
live in `PluginService.qml`, not manifest.js, because they are facts about the
*directory*: a symlink at any depth, no `.qml`, more than one `.qml`, and an
`entry` that is not the one `.qml` there. That is QML bash cannot execute, so it
is duplicated — declared in both files, and tripwired: the suite asserts
`PluginService.qml` still has exactly **five** literal-reason refusals (those
four plus `load-error`, which needs a live QML engine), still emits four
tab-separated scan fields, still counts symlinks with `find -type l`, and still
delegates to `Manifest.validateManifest` and `Manifest.scanSource`. Adding a
sixth structural refusal upstream fails the count — verified by mutation.

**Why `disable` is a directory move.** The shell has no enabled/disabled
concept: `PluginService.qml` scans exactly one directory and is the only reader
of that path in the whole shell — no allowlist file, no IPC. So `disable` moves
the plugin to a sibling the shell does not scan and `enable` moves it back,
which takes effect against the shipped shell with no shell change. A state file
invented on the OS side would be an `apex plugin disable` the shell ignored,
which is a lie told by a command whose whole job is to be believed. A shell-side
enabled list is the better long-term answer and is a §16 follow-up in
apex-shell.

`list` reports a **VALID** column, not LOADED: a disabled plugin can be
perfectly valid, and nothing here can see a live shell's loaded set. Both verbs
say plainly that a running shell keeps what it has already loaded.

**No file content is ever rewritten**, per the live-config rule added to the
root `AGENTS.md`. `enable`/`disable` are a single `mv` of a directory; the
manifest and `.qml` are carried byte-for-byte and never opened for writing, so
there is no substitution to get wrong and the operation is its own inverse.
Nothing is deleted by any verb. Asserted anyway, because "it is only a rename"
is how the next outage starts: source present and destination absent before the
move, a plugin in **both** trees refused rather than merged, an identical file
count afterwards, and a half-completed move treated as a failure. A `cp -r`
mutant fails nineteen assertions.

`disable` needs no validator at all, deliberately: taking a plugin out of the
shell's reach must work on a machine whose shell install is broken, which is
when a user most needs it. `enable` moves without one too, then reports the
verdict as *unknown* rather than guessing.

The suite **hard-exits** if either plugin directory resolves outside its temp
tree. `disable` moves directories and `~/.config/apex-shell/plugins` is real on
the developer's machine; a skip would be the accident the rule exists for.

**Unverified:** no running APEX Shell was restarted to confirm it stops loading
a disabled plugin. That rests on `PluginService.qml` scanning one directory,
which is read from source and tripwired, not observed live.

### EXPECT ONE JOB RED: the cross-repo merge order, third time

The `Package engine` job fails on **one labelled assertion** until apex-shell
**PR #9** merges. `manifest.js` is on `p1/plugin-platform`, not on apex-shell
`main`, which is what CI clones — confirmed with `git ls-tree`: `main` has no
`src/services/plugins` at all. This is the documented order (shell before or
with the OS) doing its job, exactly as it did for 5.3's keybind suite on PRs #27
and #28. **Check apex-shell `main` before looking anywhere else in this repo.**

It is deliberately one labelled assertion rather than a refusal at the top of
the file: **62 of the 117** assertions do not need manifest.js and run
regardless — measured, not estimated, by running the suite against a checkout
of apex-shell `main`.
A suite that hard-exited would report `passed=0` with a red tick, which is the
same vacuous shape this repository has been bitten by, only inverted.

### Test counts

| Suite | Before | After |
|-------|--------|-------|
| `tests/test-apex-env.sh` | 149 | 253 |
| `tests/test-apex-blueprint.sh` | 105 | 129 |
| `tests/test-apex-plugin.sh` | — | **117** (62 without the shell tree) |

Plus 35 planner unit tests in `apexd-core`, 108 in the `apex` crate, and two new
static CI checks (`check-language-parity`, `node --check` on the shim).
Every load-bearing new assertion was mutation-verified — eight mutants across
the three items, all caught, and one (a derived export filename) survived until
the fixture was changed to make the claim reachable.

## Phase 5 — compositor-neutral shell + plugin platform

Roadmap §17 asks for one `ApexCompositor` surface with compositor adapters
underneath; §16 asks for a plugin platform with permissions. The shell today
branches on `Compositor.isHyprland` / `.isNiri` / `.isLabwc` in 33 files, which
is the technical debt §17 names.

- [x] **5.1** `CompositorService` facade + capability map, with Hyprland, niri,
      labwc and null backends. — apex-shell PR #8, branch
      `p1/compositor-adapter`. 37 live assertions + 23 headless static ones.
      The facade selects its backend by URL so `Quickshell.Hyprland` is never
      parsed off Hyprland. Nothing is rewired yet; consumers move in 5.2.
- [x] **5.2** Migrate consumers onto the facade — **DONE**, apex-shell **PR #11**
      (`p1/finish-5.2`), CI green. No file outside `src/services/compositor/`
      imports `Quickshell.Hyprland`, and none spawns `hyprctl`.

      The last two consumers became capabilities in `HyprlandBackend`:
      `screenShader` (with the `.conf`/lua dialect split and the DPMS damage
      cycle) and `nightLight` (hyprsunset, including adopting an
      already-running one). Both tiles hide on `can.*`. niri and labwc declare
      `false` with a reason — hyprsunset works through
      `hyprland-ctm-control-v1`, so `wlsunset` would be the wlroots equivalent
      and is not shipped; one line to flip later.

      **A real bug fixed on the way:** the shader path was re-`find`ed at apply
      time with the chosen name spliced into a `-name` pattern, so the name
      reached a shell as code and an empty second `find` failed silently. It is
      resolved once at list time and passed as an absolute path.

      `SystemStats` gained per-backend `displayName` + `versionCommand` (argv
      only), which **fixed labwc**: no branch matched there, so the row read
      `WM: labwc:wlroots`. It now reads `WM: labwc 0.9.6`.

      **The boundary is enforced rather than claimed.**
      `check-compositor-backends.sh` scans `src/` *plus* `shell.qml` and fails
      on any `hyprctl` spawn. Three files are allowlisted with reasons, each
      asserted to *still* need it, and the matching is command-shaped rather
      than "contains the word" — the Display page's own error text mentions
      hyprctl in prose.

- [x] **5.3** One settings model → generated compositor config. The gap was
      **labwc, not niri** — the shell already wrote Hyprland `.conf`/`.lua` and
      niri `.kdl`, while labwc's bindings were hand-maintained in `rc.xml`. That
      made the Keybinds page inert on labwc: rebind, UI confirms, nothing
      happens. `apex-labwc-keybinds` now generates and splices them, and
      `check-labwc-keybinds` became an assertion that the seeded config IS the
      generator's output rather than a comparison of two hand-written lists.
      32 assertions; input and display already emitted for all three.
- [x] **5.4** Plugin platform — apex-shell **PR #9**, branch
      `p1/plugin-platform`, CI green. Manifest + permission model, loader,
      crash isolation, the `bar-widget` extension point end to end, and
      `plugins/apex-worldclock` as a real example. 156 headless decision
      assertions + 46 static invariants both run on CI; the 43 behavioural ones
      skip there, which is why the security logic lives in a plain `.js` file
      Node can import.

      That last decision paid off twice. `manifest.js` being plain JavaScript is
      what lets **apex-os's `apex plugin` CLI `require` it directly** instead of
      reimplementing the permission model — see "The three deferred P1 items" at
      the top of this file. The OS side is done on `p1/deferred-items`; this
      shell PR is the dependency it waits on.

      Two decisions worth keeping: `system`, `secrets` and `location` are
      **refused at load** rather than implemented — `system` ("run a command")
      subsumes every other permission, so shipping it would make `network` and
      `files` decorative, and a permission field that grants nothing still reads
      as a reviewed capability. And the guarantee is stated as narrower than
      "confined": QML plugins run in-process, there is no sandbox, and CI fails
      if that paragraph is deleted.

      A late hole: `files` was documented as confined to the plugin's own
      directory but enforced only textually, so a symlinked subdirectory escaped
      it with no `..` and no absolute path. Discovery now refuses any plugin
      containing a symlink at any depth.

      **Extended by apex-shell PR #12** (`p1/plugin-points`): two more points,
      `launcher-provider` and `quick-settings-tile`, each with a host, a working
      example and coverage in both suite halves. API 1.0 → 1.1, additively.
      275 headless / 98 static / 94 behavioural assertions.

      The finding that mattered: `bar-widget` lets a plugin *paint*, but both new
      points are inverted — the plugin returns data and the shell draws it. That
      is a smaller capability and a much larger checking burden, because
      `AppLauncher.activate()` dispatches on fields it finds on a row (`entry`
      runs a DesktopEntry, `exec` reaches `bash -c`). A row carrying either would
      be arbitrary execution granted to a plugin declaring **no** permissions —
      the refused `system` permission through the back door. So results are built
      from an **allowlist** into a fresh object, never a pass-through with bad
      keys deleted, and the suite asserts the surviving key set *exactly* so it
      holds for row fields the launcher does not have yet. A behavioural fixture
      returns hostile rows whose `exec` is `touch /tmp/apex-plugin-breach`; the
      file's absence is the assertion.

      **The notification handler was declined, deliberately.** Reading
      notification summaries and bodies — 2FA codes, message previews, reset
      links — maps to nothing in the closed permission vocabulary. `secrets` is
      nearest and is defined as a broker the plugin never sees through, the
      opposite arrangement. Shipping it would need an invented sixth permission
      or the shell's most sensitive stream with no declaration at all. Recorded
      rather than invented. (*Emitting* a notification is a much smaller
      capability and could be added under its own name.)

      And a check that was green for the wrong reason: renaming the tile host's
      `Loader.Error` handler kept the suite passing, because the file's own
      header sentence satisfied the grep. Every host here documents its
      invariants at length, so any check for a construct these files also
      *discuss* was satisfiable by prose — the crash isolation could have been
      deleted with CI still green. Comment lines are stripped before matching
      now, across 14 assertions, **4 of them inherited from PR #9**.
- [x] **5.5** Tests — covered by each item above rather than as a separate step.

### Phase 5 review — nine defects, seven fixed here

An adversarial review of 5.1–5.3 found real bugs that CI was green over. The
severe ones, all introduced by this branch:

- **A shared `Process` could leave every key captured.** Four commands
  consolidated onto one Process could kill each other (`running = false; true`
  terminates the child). The worst case is a retint killing `submap reset`,
  leaving Hyprland in `ApexShell_clean` until it restarts.
- **Focus mode could permanently lose the user's gaps** — a double-toggle raced
  its own restore and saved 0/6 as the "real" values.
- **The layout indicator never released its ref**, so the polling saving its
  commit message claimed never happened. It gated on `visible`, and an Item in a
  hidden Window still reports visible.
- **Keybind capture was still gated on `isNiri`** — wrong on labwc, where the UI
  offered capture and the keys then fired both the shell and labwc's bindings.
- **The labwc generator dropped every workspace shortcut** — `[A-Za-z-]+`
  excludes digits, so 20 of 68 bindings never entered the model, and the check
  reported "48 defaults" and passed.
- **`splice()` could put the block outside `<keyboard>`** and report success,
  because it anchored on `rfind("</keyboard>")`, which matches a comment.
- One settle timer served both state helpers, so a fast failure could settle a
  slow success as `(false, null)`.

Both of the remaining review items are now closed by PR #11:

**`focusedmon` is kept, deliberately**, and recorded in the event list, the
signal contract and `PopupDismiss`. `CompositorService` defines `focusMoved` as
including "a different monitor", and niri and labwc already honour that half —
dropping it would make Hyprland the one backend narrowing its own contract.

And a correction to what I wrote: **`activemonitor` was never a Hyprland event.**
`focusedmon`/`focusedmonv2` are in the binary; the only `activemonitor` string is
`workspace.activemonitor`, a Lua hook. So the monitor half of `focusMoved` was
simply unimplemented until `focusedmon` landed — the opposite of the "unflagged
behaviour change" I recorded here.

**The facade suite is honest now.** Backends declare `windowsPolled` /
`titlePolled` — deliberately NOT capabilities, because the capability map answers
"what can it do" and these answer "what does it cost". Each flat assertion became
two, chosen by that flag. 48/48 on live Hyprland *and* 48/48 staged into nested
labwc. "At least one toplevel" is an explicit precondition:
`run-nested-labwc.sh` starts a filler window and fails if it dies, so an empty
session goes red instead of passing vacuously.

### Two things PR #11 surfaced that are NOT fixed

- **`SystemStats` is registered in `src/services/qmldir` and instantiated
  nowhere** — verified with a throwaway probe. It is dead code: wire it into an
  About panel or delete it. Andre's call, not an agent's.
- **`ShellState.qml:55` still branches on `Compositor.isLabwc`** for the caffeine
  inhibitor — **now in flight** on `p1/idle-and-stats`, together with placing
  `SystemStats`. Andre's constraint on the first: he must still be able to stop
  idle with something, so a capability that makes the Caffeine tile disappear is
  a regression, not a migration. If every backend can inhibit idle by some
  route, the capability's job is to choose the *mechanism*, not gate the
  feature.
- Also flagged rather than changed, per its brief: the lua dialect interpolates
  the shader path into a lua string as `'$1'`, so an apostrophe in a filename
  would make `hyprctl eval` error. Not a shell injection — it is a bash argument
  — and inherited verbatim.

## `/tmp` was wiped mid-session, and it cost nothing

Four of the coordinator's worktrees and one agent's vanished when `/tmp` was
cleared. **Every branch they held was pushed and matched its remote**, so the
loss was a `git worktree prune` and nothing else.

This is the "commit and push every logical step" rule collecting on its premium.
Worktrees under `/tmp` are convenient and disposable *only* while that holds — a
single unpushed commit in one of them would have been gone with no warning and
no way to tell what had been in it.

## Resumed — and one hard rule added

Restarted after the third pause. Every agent carries a new rule, now in the
repo's root `AGENTS.md` (branch `docs/live-config-edit-rule`), because breaking
it destroyed Andre's live desktop mid-session:

A one-line substitution using Python `re.S` deleted **217 of 256 lines** from his
live `~/.config/hypr/hyprland.conf` — under DOTALL a trailing `.*$` matches to
the end of the FILE, not the line. Every `exec-once` went with it: next reboot,
no shell, no wallpaper daemon, no polkit agent, no clipboard, no input method.

The check I ran afterwards could not have caught it. I grepped for the line I had
just added, found it, and moved on — **grepping for what you added cannot detect
what you deleted.** `Hyprland --verify-config` and `hyprctl reload` both said
`ok`, because a truncated config is still a valid one.

Recovery took two minutes only because a `.pre-browser-fix` backup existed.
`hyprctl reload` re-reads config but does **not** re-run `exec-once`, so each
dead service had to be restarted with `hyprctl dispatch exec`. And `pgrep -f`
reported five of them as running when none were, because it matched this shell's
own command line.

Full write-up in the memory vault at
`errors-and-fixes/re-dotall-truncated-the-live-hyprland-config`.

## PAUSED (third time) — 2026-09-03

All work stopped at Andre's request. **Nothing uncommitted, nothing unpushed**,
verified by sweeping every worktree in both repos.

One worktree was dirty and is now checkpointed: `p1/gaming-profiles` held a
complete 109-assertion `tests/test-apex-gaming.sh` plus its CI wiring, none of
it committed. It passes and is shellcheck-clean, **but no assertion has been
mutation-checked**, so 109/0 is not yet evidence of anything. That is the next
step on that branch and the commit says so.

Branches in flight, all pushed:

| Branch | Repo | State |
|--------|------|-------|
| `p1/finish-5.2` | apex-shell | pushed, unreported |
| `p1/plugin-points` | apex-shell | pushed, unreported |
| `p1/blueprint-editor` | apex-shell | **PR #10 open**, unreported |
| `p1/gaming-profiles` | apex-os | suite checkpointed, mutation pass owed |
| `p1/deferred-items` | apex-os | pushed, was on the `apex plugin` CLI when stopped |
| `p1/blueprint-write` | apex-os | done — `apex blueprint set` |
| `feat/zen-browser` | apex-os | done — **PR #32** |
| `feat/labwc-default-browser` | apex-os | done |
| `fix/core-system-release-branding` | apex-os | done — **PR #31** |
| `p1/integration` | apex-os | done — **PR #30**, all four phases |

**Still not merged to `apex-os/main`, so no image has been built.**

### Fixed: niri had no application keybinds — apex-shell PR #13

**On niri, SUPER+W / SUPER+T / SUPER+E did not exist.** `KeybindService._genKdl`
has `if (e.type) continue`, commented "native compositor actions remain in
niri's own config" — which also skips every `type: "exec"` app-launch bind. So
niri users got the shell popups and no application shortcuts at all.

Fixed on `p1/niri-app-keybinds`. Applications become `spawn` with one argv token
each — niri execs without a shell, so `$browser` had to be resolved or `execvp`
would take it as a literal filename. Window actions map onto niri's column
model; what niri genuinely lacks (pseudo-tiling, toggle-split, scratchpad) is
emitted as a comment naming the dispatcher rather than vanishing. All 13 action
names were verified against the installed `niri msg action --help` before being
written, and the suite re-verifies them — niri rejects a bad include
**wholesale**, so one wrong verb costs the user every binding in the file.

Two of my own test bugs in that work, both the same family as PR #12's: a check
for "the blanket `if (e.type) continue` is gone" **failed** against a file where
it is gone, because the comment explaining the removal quotes it; and
`sed -n '/_niriActions: ({/,/})/p'` stopped at the first `})` — a nested map —
so it checked 7 of 13 names and the other 6 were never verified at all.

## Two gaps the tracker did not show — 2026-09-03

Auditing P1 against the roadmap rather than against this file turned up two
things nothing had recorded:

**§10 had no write verb.** `blueprint show/diff/init`, `apply`, `sync
export/show/import` — read, compare, seed, converge, transfer. Nothing wrote a
blueprint. The agent sent to build §10's GUI editor stopped before implementing
and said so, which was correct: the only write-shaped verb was `sync import`,
which consumes a *bundle*, so the shell would have had to author bundle TOML —
the schema reimplemented with extra steps. `apex blueprint set --json -` now
exists (`p1/blueprint-write`), reusing the same normalise + validate + to_toml +
atomic write a hand-edited file goes through.

**Every P1 verb is missing from the shell completions.** `files/desktop/shell/agent.sh`
completes `agent`, `project`, `request` and `secret` — the P0 verbs — and its
top-level list has none of `env resolve blueprint apply sync mode workload perf`.
The CLI shipped; its shell integration did not follow.

## Finishing the deferred items — 2026-09-03

The four phases are done and integrated (PR #30). Five things were deferred
along the way, and Andre asked for all of P1 finished, so they are now in flight
in parallel worktrees:

| Work | Branch | Repo |
|------|--------|------|
| finish 5.2 (screen shader, night light, SystemStats) + the two open review items | `p1/finish-5.2` | apex-shell |
| §16 more extension points (launcher provider, quick-settings tile) | `p1/plugin-points` | apex-shell |
| §10 GUI blueprint editor (7.4) | `p1/blueprint-editor` | apex-shell |
| §10 `apex blueprint set` — the write verb the editor needs | `p1/blueprint-write` | apex-os |
| shell completions for every P1 verb | `p1/completions` | apex-os |
| §12 controller-first gaming + per-game profiles (8.4) | `p1/gaming-profiles` | apex-os |
| §8 GUI export, §10 `[development] languages` convergence, §16 `apex plugin` CLI | `p1/deferred-items` | apex-os |

The two apex-os branches are off `p1/integration`, so they already contain all
four phases — which is what unblocks two of them: 8.4 needed phase 7's schema
for per-game profile storage, and the languages convergence needed phase 6's
capsules.

## PAUSED (second time) — 2026-09-03, P1 complete

All four P1 phases are done and pushed. Nothing is uncommitted and nothing is
unpushed, in either repo, verified by sweeping every worktree.

| Work | Where | State |
|------|-------|-------|
| Phase 5 §17 compositor adapter | apex-shell — **merged to main** as `30d1801` | done |
| Phase 5 §16 plugin platform | apex-shell **#9** (rebased onto main) | done |
| Phase 5.3 OS side | apex-os **#25** | done |
| Phase 6 capsules + resolver | apex-os **#29** | done |
| Phase 7 blueprint + sync | apex-os **#28** | done |
| Phase 8 modes + workloads | apex-os **#27** | done |
| MT7925 wifi resume fix | apex-os **#26** | done |

**Nothing is merged to `apex-os/main`, so no image has been built.**

**Integration is done: `p1/integration`, PR #30.** That is the branch to merge
when the final build is wanted — it lands all four phases and produces exactly
one build.

Integrating early paid for itself. Git saw two conflicts, both trivial. The one
that mattered was invisible to it: phase 7's blueprint and phase 8's modes each
export a type called `Step`, both re-exported at the crate root, so the merged
workspace did not compile (E0252). Every branch was green alone and the merge
was clean — only building the result finds that.

Verified on the merged tree: builds, 246 Rust tests, shell suites 35 / 154 / 88
/ 105 / 67 / 54, clippy and shellcheck clean.

Two integration lessons worth keeping:

- **`test-labwc-keybinds.sh` failed three assertions for a reason unrelated to
  the code.** Its shell-tree discovery tries `$ROOT/../apex-shell` and otherwise
  falls back to the installed `/usr/share/apex-shell` — and in a git worktree the
  fallback is always taken, where the installed shell is whatever the last image
  shipped. It was silent about that, so staleness read as regression. It now
  prints which tree it picked and warns when that tree is the installed one.
- **I committed conflict markers into `pr-validation.yml`.** The final merge
  conflicted in three places there as well as in `main.rs` and this file; I read
  the merge output through `tail -5`, saw only the last two, fixed those, and
  then "verified" with a grep over exactly the files I had just fixed. `git add
  -A` did the rest. GitHub rejected the workflow outright — a 0-second run with
  "likely a workflow file issue" — which is the only reason it was caught.

  The rule that would have caught it: after resolving a merge, grep the WHOLE
  TREE for markers, not the files you remember touching. `git diff --check` and
  `git status` both say so plainly and neither was consulted.

### The weekly `core` build — FIXED, PR #31

Red every Monday since ~2026-08-24 on `/etc/system-release not branded`.

**Root cause.** `Containerfile.core` de-brands by writing `/usr/lib/fedora-release`
and *inheriting* the `/etc` entries that point at it. In the base image those are
not ordinary symlinks — they are hardlinked ones carried out of the ostree commit
(`/etc/system-release` and `/etc/redhat-release` share an inode, nlink=3). The
image never wrote them; it hoped the builder would carry them through 45 layer
commits intact.

In the failing builds `/etc/system-release` existed and was readable but still
said `Fedora release 43`, while `/usr/lib/fedora-release`, written in the same
layer, said `APEX-OS release 43`. That is a flattened symlink: content snapshotted
at copy-up, no longer tracking its target.

**What moved.** GitHub's `ubuntu-24.04` runner image `20260810.271` (2026-08-11)
replaced distro podman 4.9.3 with a bundled 5.8.4 — exactly the gap between the
last green fresh `core` (08-03, runner `20260720.247`) and the first red one
(08-17, runner `20260810.271.1`). GitHub withdrew it in `20260831.293` citing
"unexpected container storage behavior… corrupted images" and "conflicts between
the bundled and distribution-provided Podman installations".

The corruption could not be reproduced on a clean host, and the report says so
rather than overclaiming. What *is* established: the Containerfile is correct on
a healthy builder, the package graph is clean, and the only thing that changed in
the window is the runner's podman.

**The fix** re-creates the seven `/etc` entries and — the part that matters —
**asserts through `/etc`**. The old block asserted `/usr/lib/os-release` three
ways and asserted nothing through `/etc`, which is why a wrong image built green
and only died 45 minutes later in verify, on a cron nobody watches. Same shape as
the `-ge 30` assertion in the labwc suite: the check pointed at something other
than the thing that could break.

Verified with real layered builds on the Katana under podman 5.8.4, including two
controls: flattening `/etc/system-release` makes the check FAIL (so it can still
catch an unbranded file), and the OLD de-branding code applied to that broken
image also FAILS — which is what shows the fix does real work rather than riding
on GitHub's podman rollback.

### The one unfinished thread: the weekly `core` build

Red every Monday since ~2026-08-24 on
`/etc/system-release not branded (bootupd derives the EFI label from it)`.
Nothing was pushed for it — the work was live probing on the Katana. The one
finding worth keeping, from the agent's last report:

> The branding layer itself is correct on Katana (probe 029 =
> `APEX-OS release 43`). The question is now whether the committed image keeps
> it.

So the debranding step *works*; the suspicion has moved to a later layer
undoing it, or to the assertion running against a different filesystem view than
the one the branding wrote to. That is where to resume.

## Paused, then resumed — 2026-09-03

Andre paused the session and unpaused it shortly after; all seven agents were
restarted from their own transcripts, so they kept their context. The snapshot
below is the state at the moment of the pause and is what a cold restart should
assume, since the agents have moved on from it.

Every agent was stopped and every worktree swept, so
**nothing is uncommitted and nothing is unpushed.** Two branches were caught
mid-edit and checkpointed as `wip(...)` commits rather than losing the work;
both say in their own commit message exactly what state they are in.

Where each branch actually stands:

| Branch | State |
|--------|-------|
| `p1/compositor-adapter` (apex-shell #8) | **MERGED to apex-shell/main** as `30d1801` |
| `p1/compositor-and-plugins` (apex-os #25) | **done** — 5.3 OS side, CI green |
| `fix/mt7925-resume` (apex-os #26) | **done** — wifi fix, applied live and verified |
| `p1/plugin-platform` (apex-shell) | substantial: manifest, permissions, loader, crash isolation, bar-widget point, example plugin. Not finished, no PR yet |
| `p1/modes-and-workloads` | mode catalogue, workload signals, perf readers, then a `wip` commit — `cargo check` clean, verbs not all wired, nothing tested |
| `p1/capsules-and-packages` | capsules + project binding landed, then a `wip` checkpoint. **The claim recorded here that it "does not parse" was WRONG** — see the correction below |
| `p1/blueprint-and-sync` | schema, pure planner, `show`/`diff`/`init`. Clean tree. `apex sync` not started |

### Katana build of `p1/compositor-and-plugins` — PASSED, 96/96

CI never builds the base image, so this is the only place a broken
`Containerfile.base` shows up before the final build. It builds cleanly:
`localhost/apex-os-base:p1-validate`, ~49 min, no failures. Every step this
branch touched passed — 61 (the generator COPYs and the verification block),
72 (the Hyprland verify-config block), and 81, the one that mattered:

    apex-labwc-keybinds: 48 defaults, 4 without a labwc equivalent
    PASS  /usr/share/apex/labwc/rc.xml matches what the shell's defaults generate

In-image: the helper is present and executable, `print` emits 44 `<keybind>`
elements (48 − 4 unmappable, consistent), and the seeded `rc.xml` carries a
134-line marked region matching the generated block exactly.

**The result has a shelf life, and it is worth being precise about.** What is
verified is that the coupling holds *at apex-shell `main` = `44b1fb4`*. Any
commit to apex-shell `main` that touches `_defaults` in `KeybindService.qml`
invalidates it without touching apex-os at all. That is the merge-order rule
doing its job, not a fragile test.

It is trustworthy rather than lucky because the staleness question was actually
checked: 59 of 96 steps came from cache, so the apex-shell clone layer could
have been testing an old shell. It was not — the cached commit equals current
`origin/main` HEAD, confirmed by `git ls-remote` before the build and by
`.apex-shell-commit` inside the finished image.

A 47K build log is left at `/var/home/andre/build/p1-validate.log` on the
Katana. Everything else was cleaned up; `:latest` was never touched.

Not reported before the stop: the adversarial review of 5.1–5.3, and whether the
Katana build completed after step 81. The `core` build fix had nothing pushed.

## In flight — four parallel branches

Andre asked for multiple agents, so phases 5.4, 6, 7 and 8 are being built
concurrently, each in its own git worktree. Worktrees are not optional here: a
previous round of parallel agents shared one working tree and produced
cross-branch commit pollution on three of them.

| Work | Branch | Repo |
|------|--------|------|
| 5.4 plugin platform | `p1/plugin-platform` | apex-shell |
| 6 capsules + resolver | `p1/capsules-and-packages` | apex-os |
| 7 blueprint + sync | `p1/blueprint-and-sync` | apex-os |
| 8 modes + workloads | `p1/modes-and-workloads` | apex-os |

Three more running alongside them, independent of P1 and of each other:

| Work | Branch |
|------|--------|
| the weekly `core` build has been red since ~2026-08-24 (`/etc/system-release not branded`) | `fix/core-system-release-branding` |
| adversarial review of the landed 5.1–5.3 work | read-only, no branch |
| Katana build of `p1/compositor-and-plugins` — CI never builds the base image, so a broken `Containerfile.base` is invisible on a PR | read-only, no branch |

The three apex-os branches all add subcommands to `apexd/apex/src/main.rs`, so
that file is the expected merge conflict at integration. Each branch is
independently complete and independently tested; integration is a separate step
and belongs to whoever picks this up.

## Phase 6 — capsules + universal package resolver — DONE

apex-os **PR #29**, branch `p1/capsules-and-packages`. All CI green.
154 + 88 shell assertions, 8 Rust integration tests, 78 CLI unit tests.

- [x] **6.1** `apex env create|list|info|enter|exec|install|rm|images` —
      rootless podman via distrobox. Each record holds the image, **the digest
      its tag resolved to at create time**, and the device profile.
- [x] **6.2** `Project.capsule` + `apex project env`, surfaced in `project info`
      and `list --json`.
- [x] **6.3** Resolver built *into* `apex-pkg` rather than beside it:
      `apex resolve`, `apex install --source rpm|flatpak|capsule [--env NAME]`,
      `apex search` across repos and Flathub, provenance on every install.
- [x] GUI export via `distrobox-export` — apex-os **PR #34**, now fully green.
      `apex env export / unexport / exports`. `distrobox-export` only runs
      *inside* a container and reaches back through `/run/host`, so there is no
      host-side program to call: the host side is
      `distrobox enter --no-tty <capsule> -- distrobox-export --app <name>`.
      No root, no polkit.

      The host `.desktop` filename is **recorded, not derived**, and `unexport`
      deletes recorded names — never a `<capsule>-*.desktop` glob, because
      capsule `shell` and capsule `shell-x` are both valid names.

      That distinction survived only because of a mutation: the assertion
      "recorded, not derived" **passed against a deriving implementation**, since
      the fixture's name happened to equal what derivation produces. Only
      mutating the code exposed it.

      Two more process findings from that PR worth carrying: the known-red step
      was originally mid-job, where it would have silently skipped four suites
      and the ShellCheck gate below it; and the suite assumed `/usr/bin/node`,
      which would have produced ~50 spurious failures on the runner.
- [x] GUI export via `distrobox-export` — **done** on `p1/deferred-items`.
      `apex env export / unexport / exports`. See "The three deferred P1 items"
      at the top of this file for what a real desktop would still be needed to
      check.
- [x] **6.4** Tests.

`apex install <bare-name>` behaves exactly as before: an exact-name RPM still
wins, and only a one-entry curated table re-routes, saying why and staying
overridable. `cuda`/`rocm` are **device profiles on Ubuntu LTS**, not the 5–20 GB
vendor images.

**Not verified:** in-capsule GPU visibility, a real `distrobox create/enter/rm`,
and the resolver against live dnf5 or Flathub metadata — probes run against
fixtures.

### A correction I owe this file
§10's own "keep generated/system state separate from user-owned blueprint
state" is the requirement the design is built around, so it is worth writing
down what the three kinds of state are:

| | file | who writes it |
|---|---|---|
| **desired** | `~/.config/apex/blueprint.toml` | a person (and, one day, the GUI) |
| **observed** | nothing — probed live on every `diff` | nobody |
| **applied** | `~/.local/state/apex/blueprint-state.toml` | `apex apply`, generated |

Collapsing observed into a cached file is the trap: `diff` would then agree with
`apply` by construction instead of by measurement, and a step that silently did
nothing would report as converged forever.

- [x] **7.1** Blueprint schema and `apex blueprint show/diff/init`.
      `apexd-core/src/blueprint.rs` is the §10 shape with
      `deny_unknown_fields` throughout, closed vocabularies, hostile-input
      checks on app names and bundle project paths, and the pure
      `plan(desired, observed)`. `apex/src/blueprint.rs` is the other half:
      path resolution, the `Host` probes, and the renderers. 24 + 13 tests.
      Two things worth knowing:
      - `Host` takes a fixture `root` **and** a separate `probe_programs`
        switch, for exactly the reason `RealWriter` needs `sys_root` and
        `host_commands` separately — a fixture root redirects a file read and
        cannot redirect a process spawn, so a fixture host that ran
        `flatpak list` would answer from the developer's machine.
      - observed `[apps]` comes from the engine's `requested` list, not
        `state.json`. `state.json` records the resolved transaction including
        every dependency, so diffing against it reports convergence based on
        packages nobody asked for.
- [x] **7.2** `apex apply` convergence, idempotent with a real dry run.
      Idempotency is a property, not a code path: the plan is recomputed from a
      fresh measurement every run, so "twice changes nothing" *is* "observed ==
      desired plans an empty list". The dry run is real for the same reason —
      the plan is computed once and `--dry-run` prints exactly the steps a live
      run executes; the only difference between the two is whether those steps
      reach a converger.
      **Three independent things keep `apply` away from a test machine:**
      1. `RealConverger::for_apply()` is the only constructor with effects,
         after `RealWriter::for_daemon`. CI has a static check with the same
         three parts as the existing host-command one, including "the real
         caller must still use it".
      2. `APEX_BLUEPRINT_NO_APPLY` refuses, after `APEX_DISPLAY_NO_LIVE`. It
         blocks only the *live* path, unlike the display guard — that is
         deliberate, and it is what lets CI export it for a whole job as a
         blanket net while every dry-run assertion still runs.
      3. **`apply` never runs `sudo`.** It converges the privilege domain it is
         already in and reports the other. That is the structural answer to
         "never cause a polkit prompt", and it also removes the silent bug
         where `sudo apex apply` writes user config into `/root` or leaves
         root-owned files in `~/.config`.
      After converging, `apply` **re-measures** and reports residual drift, per
      `apexd/AGENTS.md`: a command that reports success must verify the
      requested state, and a step can exit 0 having changed nothing.
- [x] **7.3** `apex sync export` / `show` / `import`. One bundle file carries
      the blueprint, which projects exist and where they came from — and no
      credentials of any kind, because this is a file people put in a git
      repository. Three deliberate refusals:
      - `import` **converges nothing**. It writes the blueprint and records
        projects; `diff` and `apply` stay separate decisions, so a user who
        has just pulled in someone else's file gets to read it first.
      - `import` never creates a directory or clones anything. A project path
        that is not present is reported with its remote, not acted on.
      - An existing blueprint is only replaced with `--force`, and the old one
        is kept as `blueprint.toml.previous`.
      `export` round-trips its own output through `Bundle::parse` before
      writing, and validates each project entry through the same door `import`
      uses — otherwise a bad bundle only fails on the *other* machine, hours
      later, with no way to tell which end was wrong.
- [ ] **7.4** GUI editing in the shell. **Deferred out of this phase** — §10's
      last bullet, and it is apex-shell work, not apex-os work. The schema
      round-trips through TOML losslessly so the editor has something to write.
- [x] **7.5** Tests. `tests/test-apex-blueprint.sh` — 105 assertions against
      the compiled binary — plus 27 planner unit tests in `apexd-core` and 16
      in the `apex` crate, and two static CI checks.
      The suite runs a **live** `apex apply`, deliberately. "The dry run prints
      the same steps" is only meaningful if it is compared against what a real
      run does; two identical printouts from the same unused code path prove
      nothing. It is safe because `apply` never escalates, so as an ordinary
      user the only reachable steps write files inside a throwaway HOME the
      suite created.
      Four layers keep it off the machine, and the suite asserts each:
      fake `sudo`/`pkexec`/`secret-tool`/`systemctl`/`scxctl` first on PATH
      whose invocation is a failure (with a self-test proving the trap itself
      works, or every isolation assertion would be vacuous); a fully isolated
      HOME/XDG_CONFIG_HOME/XDG_STATE_HOME per invocation; the domain split;
      and the environment guard.
      Also asserted: that the blueprint classifies app names **identically to
      the shipped `apex-pkg`**, by sourcing the engine and calling its own
      `is_flatpak_id` — the planner has to classify independently, because it
      compares against different sources, but classifying *differently* would
      report an app missing forever while the engine kept installing it.
      And that a sync bundle carries no credentials, by planting a sentinel
      token in the runtime's secrets directory and grepping the bundle for it.
      Every assertion was proved able to fail: the guard, the domain split and
      all three parts of the static check were each mutated and the failure
      observed. The suite was run under `bash -e` with a stripped environment,
      which is how GitHub Actions invokes it.

Two scope decisions made up front, both because the alternative was invention:

- **`[gaming] enabled` is observed and reported, never converged.** Gaming
  provisioning is an *image* — the Gaming editions carry the session, drivers
  and tuning. No command turns Daily into Gaming, and installing a gaming
  package set onto Daily is the edition leakage the root `AGENTS.md` forbids.
- **`[development] languages` is validated and diffed, not converged.**
  Toolchains belong to phase 6's `apex env` capsules, which are being built on a
  parallel branch right now; a language→package table here would be a second,
  conflicting answer to the same question. Validating today is still worth it:
  someone who writes `typscript` finds out today.
  — **Superseded.** Both branches are merged, and it converges through a capsule
  on `p1/deferred-items`. The table still lives only in `apex-env`, so the
  reason this was deferred is honoured rather than worked around.

During the pause I recorded that the phase 6 checkpoint "does not parse", quoted
a bash line, and amended a commit message to say so. **That was wrong.** I ran
`python3 -m ast` over `files/system/libexec/apex-pkg`, which is
`#!/usr/bin/env bash`. `bash -n` accepts it. CI picks its syntax checker **by
shebang** — a rule explained in a comment in `pr-validation.yml` that I had
written myself a few hours earlier.

The cost was not zero: the resumed agent was told to fix a line that was never
broken. Recorded rather than quietly deleted, because "I used the wrong checker
and then asserted the result confidently" is the more useful lesson than the
line number.

## Phase 7 — blueprint + sync — DONE

apex-os **PR #28**, branch `p1/blueprint-and-sync`. 105 shell assertions against
the compiled binary, 43 Rust unit tests, two static CI checks.

- [x] **7.1** Schema + a **pure** planner in `apexd-core/src/blueprint.rs`.
      `plan(desired, observed)` does no I/O, so the dry run and the live apply
      are the *same computation* — which is the only way a dry run is real
      rather than a plan the apply then ignores.
- [x] **7.2** `apex apply`, idempotent by construction (replanned from a fresh
      measurement each run) and re-measuring afterwards to report residual drift.
- [x] **7.3** `apex sync export / show / import`. Import converges nothing and
      will not clobber an existing blueprint without `--force`.
- [x] **7.4** GUI editing — apex-shell **PR #10**, branch `p1/blueprint-editor`.
      CI green, and the job log confirms the suites *executed* rather than
      skipping (75 assertions on CI, 80 locally — the 5-assertion gap is a
      vocabulary-parity block needing apex-os checked out beside the shell, and
      it is deliberately **not** a skip: the Node suite asserts the vocabularies
      regardless).

      Config → Blueprint reads with `blueprint show --json`, stages edits in
      memory, writes back through `blueprint set --json -`. **No TOML is
      authored anywhere.** The TOML shown is the CLI rendering the *saved* file;
      there is no preview of unsaved state, because no verb renders a draft and
      writing one would be the schema implemented twice.

      Nine mutation-tested assertions pin the write path: exactly one write
      command, entered from exactly one place (inside the digest re-read handler,
      so the stale guard cannot be bypassed), no plan command may name a writing
      verb, and no `sed -i` / `tee` / `truncate` / shell redirection anywhere. A
      `sed -i` writer and a `printf > "$1"` writer both go red.

      **A bug found in its own review:** `available` gated every section
      including Reload, and the plan path latched it false on any `diff` exit > 1
      — so one transient failure collapsed the page to a false "not available"
      with no escape. Cause worth remembering: DisplayService's flag was copied
      without the property that `refresh()` clears it.

      **Known and accepted:** the first GUI save reformats inline arrays, because
      `to_toml()` is `toml::to_string_pretty`. Semantically lossless — same
      blueprint, same digest, no invented `version` — and the digest surviving it
      is load-bearing, since a moved digest would trip the stale guard on the
      page's own write. Fixing the formatting in QML would be the forbidden
      second TOML writer.

      **Not verified: the QML page has never been rendered.** `qmllint` clean and
      statically checked, but the round trip is proven at the JSON/CLI boundary,
      not through the GUI. Inert until apex-os `p1/blueprint-write` merges.
- [x] **7.5** Tests.

**`apply` never runs `sudo`.** It converges the privilege domain it is already
in and reports the other. That is the structural answer to "never cause a polkit
prompt" — there is no escalation path to prompt from — and it also kills the
silent `sudo apex apply` bug that writes user config into `/root`.

Three sections are deliberately observed-but-not-converged: `[gaming] enabled`
(gaming provisioning is an image; a gaming package set on Daily is the edition
leakage `AGENTS.md` forbids), `[development] languages` (deferred to phase 6's
capsules, to avoid a conflicting language→package table at integration), and
removing applications (`apply` is additive).

**`[development] languages` is no longer one of them** — it converges through a
capsule on `p1/deferred-items`. `[gaming] enabled` and removing applications
still are, and both for reasons that do not expire. Also note that
`Plan::is_converged()` therefore now returns false for a missing language, so
`apex blueprint diff` exits 1 where it used to exit 0.

**Untested:** every run was non-root, so `sudo apex apply` actually driving the
package engine has only been exercised as a refusal.

## Phase 8 — modes + gaming + workload manager — DONE

apex-os **PR #27**, branch `p1/modes-and-workloads`. All CI green. 66 Rust
assertions + 67 in `tests/test-apex-modes.sh` against the built binary.

- [x] **8.1** `apex mode` — eight modes composing levers that already ship
      (tier, apexd's AC/battery auto-switch, game mode). **No new D-Bus
      member.** The active mode is *derived* from what apexd reports rather than
      stored, so `set` needs no root and the answer cannot go stale. `status`
      reports every exactly-matching mode, because development/creator/server
      are genuinely indistinguishable from observable state.
- [x] **8.2** `apex workload` — every signal is measured *with its path* or
      unavailable *with a reason*; there is no third state. Process names never
      decide alone: uncorroborated by PSI or load, the verdict is `unknown`.
      That is the roadmap's "do not market random tuning as AI optimization"
      taken literally.
- [x] **8.3** `apex perf` — clocks, power, temps, VRAM, sched-ext state. Frame
      time is reported unavailable *with the reason* rather than substituted.
- [x] **8.4** Controller-first / per-game profiles — apex-os **PR #33**, all
      checks green.

      **Storage: `~/.config/apex/games.toml`, a separate user-owned file, not a
      blueprint section.** The deciding argument is the blueprint's own stated
      contract — "the only file a person or a future GUI edits… nothing in APEX
      ever rewrites it behind the user's back" — and `apex game profile set` is
      a program that writes. It is still *desired* state on the test that
      matters: only an explicit user command causes a write, never a reconcile
      or a probe, and nothing reads it back as a measurement. `apex sync`
      deliberately does not carry it.

      **A latent bug in 8.1 that 8.4 does not inherit.** Read out of
      `game_enter` rather than assumed: the daemon applies the *sysprofile's*
      `[game] tier` and `fan_mode` **after** `GameMode.SetActive`, so a
      per-title tier must be re-asserted afterwards and the fan step must be
      last. `apex mode set gaming` has the same exposure and is invisible only
      because every shipped sysprofile uses `performance` for both. Left alone
      on purpose — it is 8.1's code — but it is real and it is written down.

      Per-game `scheduler`/`gpu` are **refused, not accepted and ignored**: no
      D-Bus member sets either per title, so the keys exist only to say where
      the setting really lives.

      Controller-first was already built, so it was not rebuilt. What was
      missing is that nothing could answer "will Gaming Mode start here?"
      without rebooting into it; `apex gaming` measures what the session checks
      and separates blockers from warnings using the session's own list.

      **The mutation pass found a real bug in its own suite:** seven hostile-id
      assertions were passing on a *clap usage error*, not the id check —
      `--fan` after the `--` makes clap exit 2 for every id, legal ones
      included. Fixed twice over: the option moved before the `--`, and a
      refusal must now match its message, since 2 is also clap's usage code.
      Two process errors also worth naming: a static-check run reported all four
      mutations "caught" when the check script did not exist (bash exit 127),
      and a final verification failed against a **stale binary**, because
      reverting source does not rebuild.

      **Left undone, with reasons:** no launch wrapper — it needs a real Steam
      install to verify, and an unverified exec path where every game starts is
      worse than none. Nothing restores on exit, and `show` says so. No real
      controller, no real Steam, and `apply` against a live daemon is exercised
      only as a refusal. A gamepad still cannot pick the session at the greeter,
      which is apex-shell work.
- [x] **8.4** Controller-first / per-game profiles — **DONE**, on branch
      `p1/gaming-profiles`, off `p1/integration`. Phase 7 landing is what
      unblocked it: the schema decision it was waiting on is recorded below.
- [x] **8.5** Tests.

**A real bug, found by running it rather than reading it:** `apex perf` printed
`package: 20.47 W` above `battery: 20.47 W` — the same sensor twice, because the
reader took the first hwmon exposing `power1_*` and on this ThinkPad that is
`hwmon4`, owned by `BAT0`. Both numbers real, the label invented. There is now
no single "package power" figure; sensors are named (`amdgpu/PPT: 10.00 W`) and
power-supply hwmons are excluded.

**On the harm this area caused before:** `mode`/`workload`/`perf` construct no
`SysWriter` at all. The suite puts fake `scxctl`/`nvidia-smi`/`systemctl` first
on PATH, fails if any is called, and then calls `scxctl` deliberately to prove
the tripwire was armed — a trap that cannot itself pass vacuously. Verified
independently afterwards: **zero** polkit or keyring prompt events in the whole
session, no scx scheduler running, Hyprland gaps still the user's 5/10.

**Not verified:** a real `apex mode set` against a live daemon. The plan → state
mapping is proven by a property test over all 48 start-state/mode combinations,
not by a live switch. GPU clocks and VRAM on NVIDIA and i915/xe are fixtures
only; the amdgpu path is the one exercised for real.
**8.1, 8.2, 8.3 and 8.5 are done; 8.4 is deliberately not.** `apex mode` was
free as a top-level verb — the existing `Mode` is `apex fan mode`.

- [x] **8.1** `apex mode list/show/set/status`. Eight modes composing tier,
      apexd's AC/battery auto-switch and game mode. No new D-Bus member, no
      daemon change, no state file: the active mode is **derived** from what
      apexd reports, so it cannot go stale and `set` needs no root.
      `apexd-core/src/mode.rs` is pure — no I/O, no writer — and carries the
      ordering rules, 22 unit assertions.
- [x] **8.2** `apex workload`. Signals carry provenance (`Measured` with the
      path, or `Unavailable` with the reason); a process name never decides
      anything without corroboration from PSI or load; the battery row of §13 is
      a constraint layered on top, and it does **not** unwind a running game
      session. 24 assertions.
- [x] **8.3** `apex perf` — CPU/GPU clocks, power, temperatures, VRAM, sched-ext
      state, game cpuset. **Frame time is reported as unavailable with the
      reason** and nothing is substituted for it. 20 assertions.
- [x] **8.4** Controller-first boot-to-game and per-game profiles. **DONE** —
      see "Phase 8.4" below for the storage decision, the two ordering rules,
      and what is left undone on purpose.
- [x] **8.5** `tests/test-apex-modes.sh` — 58 assertions against the built
      binary, wired into the **`rust`** job (the only one with a toolchain).
      `tests/` was added to that job's path selector: a PR touching only the
      suite set `engine=true`/`rust=false`, so it would never have run, and
      `result` would have passed anyway because a skipped job counts as success.

## Phase 8.4 — per-game profiles + boot-to-game readiness — DONE

Branch `p1/gaming-profiles`, off `p1/integration` (the only base with all four
P1 phases built and tested together). 36 + 39 Rust assertions, 113 in
`tests/test-apex-gaming.sh`, and a four-part static CI check — and the check is
verified to sit in the `static` job, which is ungated and therefore always
runs, rather than only to be *correct* (a step in the wrong job, or a workflow
that stops parsing, surfaces on GitHub as a missing check rather than a red
one). Every assertion was mutation-verified — see the end of this section,
because the mutation pass is what found the real bug.

### The storage decision, and why it is not the blueprint

Per-game profiles live in **`~/.config/apex/games.toml`**, beside the blueprint
and not inside it. The deciding argument is the blueprint's own stated
contract, quoted from `apexd-core/src/blueprint.rs`:

> **Desired** — `Blueprint`. Hand-written, user-owned, the only file a person
> or a future GUI edits […] Nothing in APEX ever rewrites it behind the user's
> back.

`apex game profile set` is a program that writes. Putting a program-written
table inside the one file whose contract is that no program writes it breaks
that contract directly — and for every user who runs the convenience verb
rather than hand-editing TOML, which is most of them.

Two supporting reasons: a blueprint describes one *machine* and is meant to
stay short and hand-readable, while a games file grows one entry per title; and
every blueprint section is either converged or carries a `blocked` reason,
whereas a game profile is *selected* when a game runs and would have to become
a third kind of section. §10 already pays for two.

**It is still desired state, not generated state**, on the test that matters —
what causes a write. It is written only in response to an explicit user
command, never by a reconcile, a timer or a probe, and nothing reads it back as
a measurement (applying re-reads the machine over D-Bus, as `apex mode set`
does). So it keeps `deny_unknown_fields` and stays hand-editable.

`apex sync` does **not** carry it, and that is a decision rather than an
oversight: it would need the no-credentials assertion extended to plant a
sentinel in a profile and prove it does not travel. The requirement was that
the storage round-trip losslessly, which it does; sync is a separate question.

### What a profile can set, and what is refused

Executable, all three behind `manage-power`, which ships `allow_active = yes` —
verified against `files/system/polkit-1/actions/org.apexos.apexd.policy` and
the daemon's `authorize(..., ACTION_POWER)` calls **before** any step was
emitted, because a per-game lever behind an `auth_admin` action would be a
password prompt in the area that has burned this repository twice:

- `mode` — a §11 mode, itself tier + auto-switch + game mode;
- `tier` — an override of that mode's tier, for one title;
- `fan` — `Fan.SetMode`, the one lever `apex mode` models and never touched.

**A per-game `scheduler` or `gpu` is refused, not accepted and ignored.** Both
already exist and both are chosen by the *sysprofile*'s `[game]` section and
applied by the daemon when game mode starts; there is no D-Bus member that sets
either per title. So the keys exist in the schema for the sole purpose of
producing a message that says where the setting really lives. That follows
5.4's precedent (refuse a permission rather than ship one that grants nothing)
over §11's (model a service set and report it), because the failure here is a
user believing their game runs under `scx_rusty` when it does not. They are
still composed at mode granularity: `mode = "gaming"` is what turns them on.

### Two ordering rules, from reading the daemon rather than guessing

`apexd/src/game.rs::game_enter` applies the **sysprofile's** `[game] tier` and
`[game] fan_mode` *after* entering game mode. So:

1. **A pinned tier is set again after `GameMode.SetActive`.** Without it, a
   profile asking for `balanced` on a machine whose sysprofile pins
   `performance` reports success and lands on performance. The CLI cannot read
   that value — it is daemon-side — so re-asserting is the only correct move.
2. **The fan step is last of all**, for the same reason one lever along.

**This is a latent bug in 8.1 that 8.4 did not inherit.** `apex mode set
gaming` sets the tier and then enters game mode, so `game_enter` overwrites it
with `cfg.tier`. It is invisible today only because every shipped sysprofile
uses `performance` for both. `mode::plan` was deliberately left alone — changing
phase 8's frozen semantics on this branch is scope creep — but whoever touches
`apex mode` next should fix it there.

### `apex gaming` — what was genuinely missing

§12's Desktop/Gaming split is **already built** and was not rebuilt:
`apex-gaming.desktop`, `apex-gaming-session` (gamescope straight to KMS, Steam
`-gamepadui`, fail-safe bounce to the greeter) and `apex-session-select` with
its NOPASSWD rule. What nothing could do was answer *"will Gaming Mode start
here?"* before rebooting into it — the session's only preflight is a `FATAL` at
start-up that bounces to the greeter and guesses at the cause in a log nobody
sees. `apex gaming` measures every requirement the session checks, with the
path measured or a reason it could not be, and separates blockers from warnings
using the session's own hard-requirement list so it cannot report "ready" about
a session that would then FATAL.

`Probe` takes a root **and** a separate `probe_programs` switch, for the reason
`RealWriter` needs `sys_root` and `host_commands` separately: no fixture root
redirects a PATH lookup. Gamepads are read from
`/sys/class/input/*/capabilities/key`, never `/dev/input/event*` — `/dev` is
not under a fixture root, so a `/dev` probe would collapse the assertion into
"is a controller plugged in right now". Two things about that bitmap that a
fixture would have hidden: the kernel **elides leading zero words**, so words
must be indexed from the right; and the word width belongs to the **reading**
process, because `input_bits_to_string` splits each long under
`in_compat_syscall`. An earlier draft guessed the width from the string's
length, which is wrong for any 64-bit device whose top word is small — caught
by the 32-bit test failing.

### Left undone, deliberately

- **No launch wrapper.** The honest equivalent of SteamOS's per-game
  application is `apex game launch -- %command%`: resolve the AppID from
  Steam's environment, apply, spawn, restore on exit. It needs a real Steam
  install, a real title and real launch options to verify, none of which this
  machine has, and an unverified exec path in the position where *every* game
  starts is worse than none — its failure mode is "the game does not launch"
  and the user cannot tell whether APEX or Proton broke it. What ships is
  `apex game profile launch-command`, which prints
  `apex game profile apply <id> && %command%`, built only from verbs that
  exist. `show` states plainly that applying does not undo itself.
- **Nothing restores on exit**, following from the above. `apex mode set daily`
  or `apex game stop` is the explicit leave step. `apex-gaming-session` has the
  same exposure and accepts it with a trap.
- **Not verified on hardware:** a real controller, a real Steam install, and
  `apply` against a live daemon. Every `apply` in the suite runs against a
  redirected D-Bus address, so the executed path is proven only as a refusal;
  the plan → step mapping is proven by unit tests over all 48
  start-state/mode combinations.
- **A gamepad cannot pick the session at the greeter.** That is apex-shell's
  greeter UI, not apex-os, so "controller-first" is true of the session and not
  yet of the login screen.

### The mutation pass, and the bug it found

Twelve source mutations and four static-check mutations, each verified to
differ from the original, to change the line count by exactly the amount
intended, and to compile, before any red was believed. All sixteen were caught.

**One of them found a vacuous assertion, which is the whole reason to do this.**
A mutant that accepted *every* game id reddened only two assertions and left
green the seven that name specific hostile ids. The cause was argument order:
`game profile set -- "$badid" --fan max` puts `--fan` after the `--`, so clap
rejects it as an unexpected positional and exits 2 — for every id, legal ones
included. Verified directly: a legal `620` took the same path and produced the
same exit code, so the loop asserted nothing about ids at all. Fixed twice
over, because the first fix alone leaves the trap re-armable: the option moved
before the `--`, and a refusal must now be identified by its message as well as
its exit code, since 2 is also clap's usage-error code.

A second, smaller lesson: an early run of the static-check mutations reported
all four "caught" when the check script did not exist — bash exited 127 and a
driver that only looked for "non-zero" believed it. The driver now runs the
control first and refuses to credit a red that arrives without the check's own
`FATAL`.

### The safety record for this branch

Zero polkit or keyring prompt events, `sched_ext` still `disabled`, no `scx_`
process, and the platform profile unchanged — checked after the fact, with
`ps -eo comm=` rather than `pgrep -f`, which would have matched the checking
command's own arguments. The suite's tripwire does not use process matching at
all: forbidden tools are fake executables that append to a log, and three
self-tests prove that log still records.

### Deliberately not shipped

- **No timer, no daemon loop, no background auto-apply.** §13 permits automatic
  policy "where safe" but is emphatic that automatic choices must be visible and
  overrideable. `apex workload` reports and `apex mode set --auto` applies once,
  explicitly. A shipped-but-disabled unit was considered and rejected: the root
  `AGENTS.md` treats aspirational-language-as-implemented as a defect, and this
  is the phase that has already burned the developer twice.
- **Service sets and system extensions are reported, not executed.** They are
  modelled so `apex mode show` can state the full intent. Merging a sysext on a
  mode switch is a heavyweight lever with its own rebuild service, and
  `Containerfile.gaming` already masks `irqbalance` permanently, so a mode
  toggling it would fight the image.
- **`docs/apexd-dbus.md` is untouched** — this phase adds no D-Bus member. Note
  for whoever picks it up: that file is stale on a separate point, still listing
  five tier IDs including `ultra-max`/`ultra`, which `tier.rs` removed.

### The cross-repo merge order, observed working

Worth recording because it is the first time the mechanism actually fired. PR
#27's **`Package engine`** job was red on two assertions in
`tests/test-labwc-keybinds.sh` — "KeybindService invokes the generator" and "it
checks the helper exists before spawning it". Both grep
`src/services/config_tab/KeybindService.qml` in an **apex-shell** tree cloned
from remote `main`, and at that moment `main` had zero occurrences of
`apex-labwc-keybinds`, because the shell half was still on the unmerged
apex-shell PR #8.

Nothing was done about it on this side. The shell change landed while phase 8
was in flight, `main` now has three occurrences, and the job went green on the
next run. That is exactly what 5.3's CI comment predicted — "a shell change …
fails here until it lands, which is the documented merge order doing its job
rather than a broken check". If a future phase sees this suite red, check
apex-shell `main` before looking anywhere else.

### The safety rule this phase is built around

`apexd-core::mode`, `::workload` and `::perf` construct **no `SysWriter` of any
kind**, and the shell suite puts fake `scxctl`/`nvidia-smi`/`systemctl` first on
PATH and fails if any is called — with a negative control proving the fakes were
really there. That is a direct response to the earlier game-mode suite, which
shelled out to `scxctl`, raised polkit prompts on the developer's desktop and
blocked 177 seconds on a password.

## Log

Newest last. One line per pushed commit that changes the state above.

- 2026-09-03 — tracker created; `p1/compositor-and-plugins` branched off
  `feat/input-display-parity`.
- 2026-09-03 — **5.1 done.** apex-shell PR #8 (`p1/compositor-adapter`): the
  `CompositorService` facade, four backends, a shared picker-script library, and
  both halves of its test coverage. niri and labwc gained output-box screenshot
  picking that nobody had wired up, and `NiriService` gained a window list off
  its existing event stream.
- 2026-09-03 — **5.2 in progress**, four pushed commits on
  `apex-shell/p1/compositor-adapter`. The facade grew what the consumers
  actually needed as they moved: `focusedAppName`, a `focusMoved` signal,
  `readGaps`, `workspaceSlots`, per-entry workspace `ref`,
  `specialWorkspaceOpen`, and the tiling-layout surface. 42 live assertions,
  23 static. The whole shell loads under a nested labwc session with zero
  errors, which exercises the labwc backend for real.
- 2026-09-03 — **5.3 done.** `files/system/libexec/apex-labwc-keybinds` +
  32 assertions in `tests/test-labwc-keybinds.sh`; the seeded `rc.xml` now
  carries a generated, marker-delimited region and 44 previously hand-written
  bindings came out of it byte-identical. Two things worth knowing: the reading
  path and the substitution path for `root._shellDir` must be separate inputs
  (`--shell-dir` vs `--shell-path`) or the build bakes a build-host path into
  the shipped config, and 4 bindings genuinely have no labwc equivalent
  (scratchpad ×2, pseudo-tiling, split) so they are reported rather than mapped
  onto something approximate.
- 2026-09-03 — **6.1 done.** `files/system/libexec/apex-env` + `apex env` in
  the CLI + 149 assertions. Three things worth knowing. (1) `cuda` and `rocm`
  are **device profiles on an Ubuntu LTS base**, not the vendor images:
  `nvidia/cuda` and `rocm/dev-ubuntu` are 5–20 GB carrying a toolchain most
  users replace, and §8 asks for the *access profile*, which is the part APEX
  has to get right. `--image` still takes a vendor image. (2) The `python`
  alias is fedora-toolbox plus the Python toolchain, **not**
  `docker.io/library/python` — that image has no user and is not a
  distrobox-compatible base, so the host integration this exists for does not
  work in it. (3) ROCm needs `--group-add keep-groups` as well as `/dev/kfd`:
  rootless podman drops the user's supplementary groups, so without it the
  device node is present and unopenable, which reads as a driver bug. The
  suite pins the exact argv per profile, because `create` can never run for
  real in CI.
- 2026-09-03 — **6.2 done.** The binding is one optional field, and the whole
  difficulty is one line in `remember`: it runs on every `apex agent run` and
  every layout save with a project detected from the filesystem, and the
  filesystem does not know which capsule the user chose. A plain replace would
  have unbound the project the first time an agent started, silently — the
  record would still be there and still valid. `remember` now keeps an
  existing binding when the incoming one is `None`, and `apex project env
  --clear` writes directly instead of going back through it, or the merge
  would undo the clear. Both have their own assertion. A second thing found by
  the suite rather than by reading: `project::list` DELETES the record of any
  project whose checkout has gone, so a test using a root that does not exist
  loses its record the moment another case runs a listing.
- 2026-09-03 — **6.3 and 6.4 done.** The resolver extends `apex-pkg`; there is
  still exactly one package engine. The design question was what
  `apex install <bare-name>` should do now, and the answer is **the same thing
  it did before**: that command ships, and silently re-routing a name which
  installs an RPM today would be a behaviour change on a shipped command. So
  an exact-name repository package still wins — it puts the command on `$PATH`,
  its signature is checked against keys APEX already trusts, and it rolls back
  with the extension — and the resolver's job is to SAY what it chose, print
  the alternative as a runnable command, and take `--source` when the user
  disagrees. The one thing that can move a name is a curated table, currently
  **one entry** (discord: the RPM Fusion package is the vendor tarball
  repackaged and refuses to start once it considers itself stale, which on an
  image-based system it regularly does). Every entry carries its reason in the
  file and prints it at install time. Two notes for later: an earlier draft
  tried to pick GUI-vs-CLI from `repoquery --file '/usr/share/applications/*'`
  to decide Flatpak-vs-RPM automatically — it classifies neovim as graphical,
  costs a multi-MB filelists fetch on the install hot path, and was dropped.
  And mutation testing found a real hole: the `provenance:` line on the RPM
  install path sits behind the root gate, so no runnable assertion could reach
  it; it is covered by a static check over `cmd_install`, labelled as such.
- 2026-09-03 — for the record, because it cost an hour: a mid-session sweep
  committed the half-finished resolver as `wip(pkg)` and reported it as "DOES
  NOT PARSE", having run `python3 -m ast` over `files/system/libexec/apex-pkg`.
  That file is bash. `bash -n` passes, and CI selects the syntax checker BY
  SHEBANG — only `installer/apex-installer-gui` is fed to Python. The cited
  line 124 is `is_flatpak_id`, untouched upstream code. Nothing was broken. The
  commit has been squashed into the finished §9 commit, so the false claim is
  not in the branch history.
- 2026-09-03 — **do not judge these suites by a local run.** They look for an
  apex-shell tree at `../apex-shell` and fall back to `/usr/share/apex-shell`,
  which is whatever the booted image shipped and therefore lags the tree under
  test — the file says so itself, and it is still easy to forget.
  `test-apex-firstrun.sh` fails against that stale copy and passes (24/0) in
  CI. I then predicted, from a worktree of the local `origin/main` ref, that
  `test-labwc-keybinds.sh` would fail on two assertions; **that prediction was
  wrong** — the local ref was behind, the phase-5 shell work is on
  `apex-shell/main`, and CI reports 32/0. PR #29 is green in full. Recorded
  because the mistake is repeatable: a stale `origin/main` in the sibling
  apex-shell clone looks exactly like an unmerged shell branch. `git fetch`
  there before drawing any conclusion about the merge order.
- 2026-09-03 — **Phase 7 done.** PR #28 (`p1/blueprint-and-sync`): the schema
  and pure planner in `apexd-core/src/blueprint.rs`, the probes, converger and
  verbs in `apex/src/blueprint.rs`, `tests/test-apex-blueprint.sh`, and two
  static CI checks. `apex blueprint show/diff/init`, `apex apply`,
  `apex sync export/show/import`. Rust validation and Static validation both
  green on the first CI run.
  The `Package engine` job is red on that PR and it is **not** phase 7's: the
  two failing assertions are phase 5.3's, checking that apex-shell's
  `KeybindService.qml` invokes `apex-labwc-keybinds`. The suite clones
  apex-shell from remote `main`, and that change has not landed there yet —
  `gh api` confirms zero occurrences on `apex-shell/main`. This is the
  documented merge order (shell before or with the OS) doing its job, and it
  will clear when `p1/compositor-adapter` merges to `apex-shell/main`.
- 2026-09-03 — **Phase 8 done bar 8.4** on `p1/modes-and-workloads`: `apex
  mode`, `apex workload`, `apex perf`, 66 new Rust assertions and a 58-assertion
  shell suite. Two things worth carrying forward. First, `apex perf` shipped a
  real bug that only running it exposed — it reported "package: 20.47 W" and
  "battery: 20.47 W", the same sensor twice, because the package reader took the
  first hwmon publishing `power1_*` and on this ThinkPad that is `hwmon4`, owned
  by `BAT0`. There is now no single "package power" figure at all: every sensor
  is reported with its chip and label, and hwmon devices hanging off a
  `power_supply` are skipped. Second, the ordering rules in `mode::plan` are not
  cosmetic — game mode must be left *before* the new tier is set, because
  `apex game stop` restores the pre-session tier and would otherwise overwrite
  it, and auto-switch must go off *before* a tier is pinned, because enabling it
  reconciles immediately. Both are mutation-verified.
- 2026-09-03 — **8.4 done**, closing the last deferred P1 item, on
  `p1/gaming-profiles` off `p1/integration`. Per-game profiles in
  `~/.config/apex/games.toml` (a separate user-owned file — the blueprint's own
  contract is that no program rewrites it, and `profile set` is a program that
  writes), `apex game profile list/show/set/remove/apply/launch-command/path`,
  and `apex gaming` for boot-to-game readiness. Three things worth carrying
  forward. First, `game_enter` applies the SYSPROFILE's `[game] tier` and
  `fan_mode` after `GameMode.SetActive`, so a per-title tier has to be
  re-asserted afterwards and the fan step has to be last — and that means
  `apex mode set gaming` has the same latent bug, invisible only because every
  shipped sysprofile uses `performance` for both. Second, an input device's
  `capabilities/key` bitmap must be indexed from the RIGHT (the kernel elides
  leading zero words) and its word width belongs to the READING process, not
  the kernel; guessing the width from the string's length is wrong for any
  64-bit device whose top word is small. Third, the mutation pass earned its
  keep: it found seven hostile-id assertions passing on a clap usage error
  rather than on the id check, because `--fan` sat after the `--` and clap
  exited 2 for every id including legal ones.
- 2026-09-03 — noted for whoever picks this up: an early draft of the facade
  test called `setGaps(0, 0)` to check that a *capable* action returns true, and
  set the live Hyprland gaps to zero on the developer's desktop. Suites here run
  against the real session. Assert refusal on the actions a backend cannot
  perform; check the capable ones by shape and never invoke them.
- 2026-09-03 — **the three deferred P1 items are done** on
  `p1/deferred-items`, off `origin/p1/integration`: §8's GUI export from
  capsules, §10's `[development] languages` convergence, and §16's `apex plugin`
  CLI. Full write-up at the top of this file. Four things worth carrying
  forward.
  **First**, a mutant survived and that is the useful part: "the exported
  `.desktop` filename is recorded, not derived" passed against an
  implementation that derived it, because the fixture filename
  (`py-gimp.desktop`) happened to equal what the derivation produces. The real
  distrobox output is `py-org.gimp.GIMP.desktop`. A fixture that cannot
  distinguish the right answer from the wrong one makes the assertion
  decorative, and no amount of running it would have shown that — only mutating
  the code did.
  **Second**, the language vocabulary check went in the **`static`** job, not
  `rust` or `engine`. The path selectors are `^(apexd/|config/sysprofiles/|tests/)`
  and `^(files/|tests/)`, so a check in either specialised job is skipped by a
  PR that changes only the other side — and a skipped job counts as success.
  This file already records that exact bug once, for `test-apex-modes.sh`. Any
  future cross-file parity check belongs in `static` unless it needs a
  toolchain.
  **Third**, `apex plugin` calls apex-shell's `manifest.js` through node rather
  than reimplementing it, and the coupling is asserted **differentially** — the
  suite asks manifest.js directly for each fixture and compares the reason code.
  The four structural refusals that live in `PluginService.qml` instead could
  not be shared, so they carry a tripwire on the count of literal-reason
  refusals in that file. "Declare the duplication and make it fail loudly" beat
  both alternatives here.
  **Fourth**, `apex plugin disable` moves a directory and never edits a file,
  which is the strongest available compliance with the live-config rule added to
  `AGENTS.md` the same day. The suite hard-exits if its plugin directory
  resolves outside the temp tree, because `~/.config/apex-shell/plugins` is real
  on the developer's machine.

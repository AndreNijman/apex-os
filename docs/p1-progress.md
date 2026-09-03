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
the file: 61 of the 113 assertions do not need manifest.js and run regardless.
A suite that hard-exited would report `passed=0` with a red tick, which is the
same vacuous shape this repository has been bitten by, only inverted.

### Test counts

| Suite | Before | After |
|-------|--------|-------|
| `tests/test-apex-env.sh` | 149 | 253 |
| `tests/test-apex-blueprint.sh` | 105 | 129 |
| `tests/test-apex-plugin.sh` | — | 113 (61 without the shell tree) |

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
- [~] **5.2** Migrate consumers onto the facade. **Nearly done.** No file
      outside `src/services/compositor/` imports `Quickshell.Hyprland` any
      more. Migrated: CenterContent, IpcManager, ScreenRecService,
      WallpaperService, QuickSettings (focus mode), ShellState (keybind
      interception), PopupDismiss, Workspaces, LayoutDisplayer.
      **Left:** `QuickSettings` screen shader + night light (both genuinely
      Hyprland-only features, needing a `screenShader` capability), and
      `SystemStats`' `hyprctl version` line. Keybind and display config
      generation are 5.3, not 5.2. Compositor *name* checks that remain are
      about appearance — labwc borders, the labwc dock — which §17 permits.
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

Still open, both low severity: `focusedmon` was added to the focus events, so
with `follow_mouse` crossing a monitor boundary now closes popups (unflagged
behaviour change); and the facade suite encodes Hyprland's refcount semantics as
universal, so it would fail on niri or labwc.

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

**Nothing is merged to `apex-os/main`, so no image has been built.** The four
branches are stacked and all touch `apexd/apex/src/main.rs`; integration is a
real step, not a formality. Merging only the tip lands everything and produces
one build.

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
- [ ] **7.4** GUI editing — deferred; it is apex-shell work. The schema
      round-trips losslessly so the editor has something to write.
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
- [ ] **8.4** Controller-first / per-game profiles — deferred, and it was the
      stated lowest priority. `apex-gaming-session` already gives boot-to-game;
      per-game profile *storage* wants a schema decision that belongs with
      phase 7's blueprint.
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
- [ ] **8.4** Controller-first boot-to-game and per-game profiles. **Not done**,
      and it was the explicitly lowest-priority item. The gamescope session
      (`files/system/libexec/apex-gaming-session`) already gives a
      controller-first boot-to-game path; what is missing is per-game profile
      storage and selection, which wants a schema decision that belongs with
      phase 7's blueprint rather than being invented here.
- [x] **8.5** `tests/test-apex-modes.sh` — 58 assertions against the built
      binary, wired into the **`rust`** job (the only one with a toolchain).
      `tests/` was added to that job's path selector: a PR touching only the
      suite set `engine=true`/`rust=false`, so it would never have run, and
      `result` would have passed anyway because a skipped job counts as success.

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

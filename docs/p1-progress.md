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
| `p1/capsules-and-packages` | capsules + project binding landed; then a `wip` commit that **does not parse** — bash spliced into `apex-pkg` at line 124. Fix that first |
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

## Phase 6 — capsules + universal package resolver

Not started. `apex install` / `search` / `repo` / `pkg` already ship on the
sysext engine — phase 6 extends that, it does not replace it.

- [ ] **6.1** `apex env` capsules (create/list/enter/exec/rm), GPU and device
      profiles, GUI export to the host desktop.
- [ ] **6.2** Project ↔ capsule binding, wired into the agent runtime.
- [ ] **6.3** Universal resolver: rank across dnf / flatpak / capsule, present a
      canonical choice, keep explicit source selection.
- [ ] **6.4** Tests.

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

**Untested:** every run was non-root, so `sudo apex apply` actually driving the
package engine has only been exercised as a refusal.

## Phase 8 — modes + gaming + workload manager

Not started. `apex mode` is free as a top-level verb — the existing `Mode` is
`apex fan mode`.

- [ ] **8.1** `apex mode` composing existing primitives (tier, profile, game,
      services, sysext).
- [ ] **8.2** Workload-aware policy: measured signals, visible and overrideable.
- [ ] **8.3** Performance Lab.
- [ ] **8.4** Controller-first gaming mode and per-game profiles.
- [ ] **8.5** Tests.

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
- 2026-09-03 — noted for whoever picks this up: an early draft of the facade
  test called `setGaps(0, 0)` to check that a *capable* action returns true, and
  set the live Hyprland gaps to zero on the developer's desktop. Suites here run
  against the real session. Assert refusal on the actions a backend cannot
  perform; check the capable ones by shape and never invoke them.

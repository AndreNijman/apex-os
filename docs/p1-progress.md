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
- [ ] **5.4** Plugin platform: manifest + permission model, loader, extension
      points, crash isolation, `apex plugin` CLI, one real example plugin.
- [ ] **5.5** Tests: shell CI invariants + an OS-side plugin suite.

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

## Phase 7 — blueprint + sync

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
- [ ] **7.2** `apex apply` convergence, idempotent with a real dry run.
- [ ] **7.3** `apex sync` between machines.
- [ ] **7.4** GUI editing in the shell. **Deferred out of this phase** — §10's
      last bullet, and it is apex-shell work, not apex-os work. The schema
      round-trips through TOML losslessly so the editor has something to write.
- [ ] **7.5** Tests.

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

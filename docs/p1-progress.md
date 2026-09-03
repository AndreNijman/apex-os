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

- [ ] **5.1** `CompositorService` facade + capability map, with Hyprland, niri,
      labwc and null backends. Done when the facade answers `windows`,
      `workspaces`, `focus`, `moveWindow`, `overview`, `screenshot`,
      `outputState` and `inputState` on every backend, reporting unsupported
      rather than failing.
- [ ] **5.2** Migrate consumers onto the facade, a file (or small group) per
      commit. Done when no module outside `src/services/compositor/` spawns
      `hyprctl` or `niri msg`.
- [ ] **5.3** One settings model → generated compositor config, with per-
      compositor emitters (Hyprland and labwc exist; niri is missing). Done
      when a keybind set round-trips to all three.
- [ ] **5.4** Plugin platform: manifest + permission model, loader, extension
      points, crash isolation, `apex plugin` CLI, one real example plugin.
- [ ] **5.5** Tests: shell CI invariants + an OS-side plugin suite.

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

Not started.

- [ ] **7.1** Blueprint schema and `apex blueprint show/diff`.
- [ ] **7.2** `apex apply` convergence, idempotent with a real dry run.
- [ ] **7.3** `apex sync` between machines.
- [ ] **7.4** GUI editing in the shell.
- [ ] **7.5** Tests.

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

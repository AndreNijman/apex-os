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

## Phase 6 — capsules + universal package resolver

Not started. `apex install` / `search` / `repo` / `pkg` already ship on the
sysext engine — phase 6 extends that, it does not replace it.

- [x] **6.1** `apex env` capsules. `files/system/libexec/apex-env` +
      `apex env` in the CLI, 149 assertions in `tests/test-apex-env.sh`.
      Rootless distrobox/podman, records under `${XDG_DATA_HOME}/apex/env`,
      image aliases (fedora, ubuntu, arch, debian, python, cuda, rocm) and
      device profiles (nvidia, amd, hw, none). **GUI export via
      `distrobox-export` is deliberately not done** — §8 says "when useful",
      and it needs a real desktop session to verify, which CI does not have.
- [x] **6.2** Project ↔ capsule binding. `Project.capsule` (serde-default so
      every existing record still parses), `apex project env [NAME|--clear]`,
      shown by `apex project info` and carried in `list --json`. 8 assertions
      in `apexd/apex-agent-core/tests/capsule_binding.rs`.
- [x] **6.3** Universal resolver. `apex resolve <name>`, `apex install
      --source rpm|flatpak|capsule [--env NAME]`, `apex search` across
      repositories and Flathub, provenance printed on every install. Built
      into `apex-pkg`, not beside it.
- [x] **6.4** Tests. 149 (`test-apex-env.sh`) + 88 (`test-apex-resolve.sh`)
      + 8 (`capsule_binding.rs`) + the CLI unit tests, all wired into
      `pr-validation.yml` and all shellcheck-gated. Every assertion was proven
      able to fail by mutating the shipped file and re-running.

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
- 2026-09-03 — **what CI will say about the labwc suite, measured.** Running
  the suites locally is misleading here: they look for an apex-shell tree at
  `../apex-shell` and fall back to `/usr/share/apex-shell`, which is whatever
  the booted image shipped and lags the tree under test. Reproducing what CI
  actually vendors (a worktree of `apex-shell` `origin/main` at
  `../apex-shell`): `test-apex-firstrun.sh` **passes, 51/0** — the local
  failure was purely the stale fallback. `test-labwc-keybinds.sh` **fails on
  two assertions** ("KeybindService invokes the generator", "it checks the
  helper exists before spawning it") and fails identically at the branch point
  4c4fc62 with none of phase 6 present. That is 5.3 waiting on
  `apex-shell/p1/compositor-adapter` to land on `apex-shell/main` — the
  documented merge order (shell before or with the OS) doing its job, not a
  phase 6 regression.
- 2026-09-03 — noted for whoever picks this up: an early draft of the facade
  test called `setGaps(0, 0)` to check that a *capable* action returns true, and
  set the live Hyprland gaps to zero on the developer's desktop. Suites here run
  against the real session. Assert refusal on the actions a backend cannot
  perform; check the capable ones by shape and never invoke them.

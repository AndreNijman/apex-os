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

### Known, unfixed, and outside what was asked

**On niri, SUPER+W / SUPER+T / SUPER+E do not exist.** `KeybindService._genKdl`
has `if (e.type) continue`, commented "native compositor actions remain in
niri's own config" — which also skips every `type: "exec"` app-launch bind. So
niri users get the shell popups and no application shortcuts at all. An app
launch is a `spawn`, which niri supports, so the `continue` is catching more
than it meant to. Pre-existing; raised with Andre, not yet fixed.

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
- [ ] GUI export via `distrobox-export` — §8 says "when useful"; needs a real
      desktop to verify. Deferred.
- [x] **6.4** Tests.

`apex install <bare-name>` behaves exactly as before: an exact-name RPM still
wins, and only a one-entry curated table re-routes, saying why and staying
overridable. `cuda`/`rocm` are **device profiles on Ubuntu LTS**, not the 5–20 GB
vendor images.

**Not verified:** in-capsule GPU visibility, a real `distrobox create/enter/rm`,
and the resolver against live dnf5 or Flathub metadata — probes run against
fixtures.

### A correction I owe this file

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

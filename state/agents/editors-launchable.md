# editors-launchable
items: user-directed defect (not a roadmap item) — "nvim and zed have to work
       since neither work", clarified as "it says provided by apex already when
       i install but the existing one doesn't run"
repo: apex-os AND apex-shell (PAIRED — see LANDING)
worktree: /var/tmp/apex-work/wt-editors2        (apex-os,    branch pushed)
          /var/tmp/apex-work/wt-editors2-shell  (apex-shell, branch pushed)
branch: task/terminal-entries-launchable  (same name in both repos)

## NEXT
Nothing outstanding. The one thing left needs somebody else first: CI cannot yet
be dispatched on a task branch (see CI below). When `workflow_dispatch` reaches
apex-os `main`, run
`gh workflow run pr-validation.yml --ref task/terminal-entries-launchable`.

## CI — tried, blocked, substituted
- `gh workflow run pr-validation.yml --ref task/terminal-entries-launchable`
  → HTTP 422 "Workflow does not have 'workflow_dispatch' trigger". The trigger
  IS on roadmap/v2.2 (0b16268f) but the dispatch API reads the DEFAULT branch,
  and main still has only `pull_request`.
- The `push:` trigger that did land is filtered to `branches: [roadmap/v2.2]`,
  so merging v2.2 forward into this branch would NOT make CI run on it. Not
  done — it would only muddy the integrator's diff for no signal.
- Substitute, which is the same image CI uses (`runs-on: ubuntu-24.04`):
  both suites run in a clean `docker.io/library/ubuntu:24.04` container with
  `--network=none` and the repo mounted read-only.
    tests/test-apex-editors.sh    → 17 passed, 0 failed, 3 skipped
    tests/run-terminal-entry-test.sh → "SKIP: quickshell not installed", exit 0
  That is the structural layer doing exactly what it was written for: green
  where there is no zed.app, no nvim.desktop, no APEX image and no compositor.

## DONE
- apex-shell 3f8a286 — src/services/DesktopExec.qml + both call sites +
  tests/run-terminal-entry-test.sh + ci.yml step. PUSHED.
- apex-os f64446a — merge of task/editors-desktop-entries (the zed half; it has
  since landed on roadmap/v2.2 as ec91e92 anyway, so this merge is redundant and
  conflict-free).
- apex-os bbea274 — install xdg-terminal-exec in Containerfile.core, assert the
  three-part agreement in Containerfile.base, extend tests/test-apex-editors.sh.
  PUSHED.
- apex-os 1aabcf7 — tightened the package assertion that could not fail against
  its own mutant, plus a control. PUSHED.
- apex-shell b36db7c — Path= measured on BOTH arms (it was claimed in a comment
  and the routed code path had never run), plus load-checks of the two edited
  QML files. PUSHED.
- TEN mutation pairs run, every restore byte-identical (sha256 checked):
  5 against apex-shell DesktopExec.qml, 5 against the two Containerfiles.
- Load-checked the two edited QML files rather than reasoning about them:
  run-popup-smoke.sh (loads the whole shell.qml, so AppLauncher) → ERROR count 0;
  run-labwc-matrix-test.sh (instantiates AppDock) → 60 passed, 0 failed.
- END-TO-END chain verified against the SHIPPED helper (rpm unpacked to the
  scratchpad, never installed): upstream xdg-terminal-exec 0.14.1, given a list
  of exactly APEX's shape (one bare `<id>.desktop` line) in $XDG_CONFIG_DIRS,
  selects that entry and invokes it as `<terminal> -e foo bar`. `-e` is what
  Alacritty takes (`-e, --command <COMMAND>...`). So the real chain is:
  click → DesktopExec.launch → xdg-terminal-exec → /etc/xdg/xdg-terminals.list
  → Alacritty.desktop → `alacritty -e nvim`.

## IN PROGRESS
- nothing

## FOUND
- **The measurement the task asked for: Quickshell's DesktopEntry.execute() does
  NOT honour `Terminal=true`.** Measured headlessly on quickshell-0.3.1 under a
  private labwc (tests/run-terminal-entry-test.sh):
      runInTerminal = 1                    Terminal=true IS parsed
      command       = ["…"] with %F gone   field codes ARE stripped
      raw execute() → ran=1 stdout_tty=0 fd1=/dev/null    NO terminal
      via DesktopExec → ran=1 stdout_tty=1 fd1=/dev/pts/1 terminal
  So AppLauncher.qml:325's comment was two-thirds right and wrong about the
  third. A Terminal=true program was started on pipes, drew into /dev/null, died.
- **APEX already shipped the config file for a program it never installed.**
  files/system/xdg/xdg-terminals.list → /etc/xdg/xdg-terminals.list has been in
  Containerfile.base since apex-logs 31, says `Alacritty.desktop`, and is live on
  the L16 — and `xdg-terminal-exec`, the only program that reads it, was absent
  from every APEX machine. Configuration installed, reader never packaged.
- Fedora 43 DOES package xdg-terminal-exec (0.14.1-1.fc43.noarch, repo
  `updates`, upstream Vladimir-csp). An APEX implementation was written and then
  deleted in favour of the package — the "Fedora has none" premise was checked
  and false. The upstream helper does NOT read $TERMINAL; it reads
  xdg-terminals.list, which is why the agreement assertion exists.
- TWO callers of entry.execute(), not one: AppLauncher.qml:399 and
  AppDock.qml:102. Fixing one would have made the defect look intermittent.
- Test-harness traps found and fixed IN MY OWN new tests, both of which would
  have produced false passes:
  1. the probe recorder ran `[ -t 1 ]` INSIDE `{ … } > "$sentinel"`, so fd 1 was
     the file and the answer was always "no terminal" — the suite would have
     passed on a launcher that did nothing.
  2. the package assertion grepped the whole stanza, which contains the FATAL
     message naming the package, so deleting the install left it green.
- DesktopEntries is not populated at Component.onCompleted, and the FIRST byId()
  is what primes the scan. Cost two probe runs to pin down; noted in the QML.
- Quickshell 0.3.1 exports `processContext` as a structured value type, so
  `Quickshell.execDetached({command: […], workingDirectory: …})` works from QML.

## BLOCKED ON
- nothing. (katana was never touched — all measurement was on the L16 and in the
  worktrees, read-only against the live system.)

## COUNTS
- apex-shell tests/run-terminal-entry-test.sh: new suite, 14 passed 0 failed.
- apex-shell tests/run-popup-smoke.sh: ERROR count 0. tests/run-labwc-matrix-
  test.sh: 60 passed 0 failed. (Both pre-existing, both load my edits.)
- apex-shell tests/check-headless-runners.sh: 25/0 before, 25/0 after — the new
  runner is inside its swept set and clean (the suite's assertion count is fixed,
  it does not grow per runner).
- apex-shell shellcheck -S warning tests/*.sh: clean.
- apex-os tests/test-apex-editors.sh on the L16: 14 passed 1 failed BEFORE
  (the 1 = Zed's missing entry, this image predates the zed fix);
  26 passed 2 failed AFTER (+12 assertions; the 2 are both live-layer facts
  about this pre-fix image: Zed's entry, and xdg-terminal-exec not on PATH).
  Both clear on an image build. The structural layer is green everywhere.

## LANDING (for the integrator)
The two halves are ONE change and must land together. The shell routes
Terminal=true entries through `xdg-terminal-exec`; only the apex-os half ships
it. A new shell on an old image breaks Terminal=true entries in a NEW way —
there is deliberately no fallback chain in the QML, because a chain would hide
exactly that. Same branch name in both repos on purpose.
apex-os task/terminal-entries-launchable already contains ec91e92 (the zed half),
so it merges into roadmap/v2.2 without an add/add conflict on
tests/test-apex-editors.sh either way round.

CAVEAT — CORE MUST REBUILD BEFORE BASE. The new assertion lives in
Containerfile.base and checks for a package Containerfile.core installs. Build
base against a cached/stale `:core` image that predates this and it FATALs with
"…ships but xdg-terminal-exec does not". That is the assertion doing its job,
not a bug — but `fix/base-cache` and `fix/restore-base-cache` in the branch list
say base caching has caught this repo out before, so: rebuild core first.

# editors-launchable
items: user-directed defect (not a roadmap item) — "nvim and zed have to work
       since neither work", clarified as "it says provided by apex already when
       i install but the existing one doesn't run"
repo: apex-os AND apex-shell (PAIRED — see LANDING)
worktree: /var/tmp/apex-work/wt-editors2        (apex-os,    branch pushed)
          /var/tmp/apex-work/wt-editors2-shell  (apex-shell, branch pushed)
branch: task/terminal-entries-launchable  (same name in both repos)

## NEXT
Dispatch CI once the orchestrator's workflow_dispatch trigger lands:
`gh workflow run pr-validation.yml --ref task/terminal-entries-launchable`
(apex-os) and the apex-shell equivalent, then read the structural layer's
result with `gh run view --log`. Everything else is done and pushed.

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
- Nine mutation pairs run, every restore byte-identical (sha256 checked):
  4 against apex-shell DesktopExec.qml, 5 against the two Containerfiles.

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
- apex-shell tests/run-terminal-entry-test.sh: new suite, 12 passed 0 failed.
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

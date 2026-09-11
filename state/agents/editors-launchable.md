# editors-launchable
items: user-directed defect (not a roadmap item) — "nvim and zed have to work since
       neither work"; clarified as "it says provided by apex already when i install
       but the existing one doesn't run"
repo: apex-os AND apex-shell (PAIRED — see LANDING below)
worktree: /var/tmp/apex-work/wt-editors2        (apex-os)
          /var/tmp/apex-work/wt-editors2-shell  (apex-shell)
branch: task/terminal-entries-launchable  (same name in both repos)
sibling: task/editors-desktop-entries (ec91e92, apex-os) — fixes the ZED half
         (wrong desktop-entry filename + swallowed install). Do not redo it;
         this branch EXTENDS its tests/test-apex-editors.sh.

## NEXT
Push both worktrees with -u, then build the headless probe that answers whether
Quickshell DesktopEntry.execute() honours Terminal=true.

## DONE
- (nothing pushed yet)

## IN PROGRESS
- Both worktrees created off their repos' roadmap/v2.2 tips (os 1afd807,
  shell 665a3cc). Card written.

## FOUND
- `strings /usr/bin/qs` (quickshell-0.3.1-2.fc43) contains `runInTerminal` and
  `runInTerminalChanged` — the property symbols — and contains NO
  `xdg-terminal-exec`, no bare `TERMINAL`, and no terminal-emulator name
  (alacritty/kitty/foot/xterm/konsole/gnome-terminal). Strong indication that
  execute() parses Terminal= into a property and then ignores it. To be
  confirmed by measurement, not shipped on.
- TWO call sites of entry.execute(), not one:
  src/services/AppLauncher.qml:399 and src/modules/Left/AppDock.qml:102.
  Fixing only the launcher would leave the dock broken identically.
- AppLauncher.qml:325's comment claims execute() "respects Terminal=, Path= and
  Exec field codes". That is a claim with no evidence behind it.
- On L16: alacritty, kitty and foot are all on PATH; ghostty, wezterm, xterm and
  **xdg-terminal-exec** are all ABSENT.

## BLOCKED ON
- nothing

## LANDING (for the integrator)
The two halves are one change and must land together, or be held together.
The shell half routes Terminal=true entries through `xdg-terminal-exec`; only
the apex-os half ships that helper. A new shell on an old image would break
Terminal=true entries in a NEW way. Same branch name in both repos on purpose.
Expect to land task/editors-desktop-entries (zed) as well — this branch's test
changes are edits to the file that branch adds.

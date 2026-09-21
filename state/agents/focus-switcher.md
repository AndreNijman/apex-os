# focus-switcher — DONE (2026-09-20), not landed

Andre: "make it so that super+arrow actually moves the mouse so that the window
you move to actually stays in focus instead of instantly losing focus, also make
it so that you can also just use alt tab to cycle. build a full alt+tab switcher
thing and it goes for all windows including windows in other workspaces."

Branches, both pushed, neither landed:
  apex-os     task/focus-switcher   (from roadmap/v2.2 @ 3855ed4a) — 5 commits
  apex-shell  task/focus-switcher   (from roadmap/v2.2 @ b04c035)  — 3 commits
Worktrees: /var/tmp/apex-work/wt-focus-switcher, /var/tmp/apex-work/wt-focus-shell

## What each session does now

Hyprland  SUPER+arrow moves focus and the POINTER goes with it
          (`cursor { no_warps = false }` pinned in apex/input-defaults.lua).
          ALT+Tab opens the APEX Shell switcher; ALT+SHIFT+Tab steps back;
          releasing ALT commits — Alt_L and Alt_R, each with and without SHIFT
          (after ALT+SHIFT+Tab the fingers do not leave both modifiers at the
          same instant). ALT+Return also commits; ALT+Escape cancels. Every
          bind but ALT+Tab itself is `non_consuming = true`.
labwc     SUPER+arrow still moves the WINDOW (labwc has no directional-focus
          action) and now warps the cursor with it, so a followMouse session
          does not hand focus to whatever the move uncovered. ALT+Tab is
          labwc's own thumbnail switcher, now `workspace="all"`.
niri      Unchanged behaviour, but ALT+Tab is now WRITTEN rather than
          inherited: `recent-windows { binds { Alt+Tab … } }` in the shell's
          generated ApexShellKeybinds.kdl.

## Measured, 2026-09-20 — do not re-derive

* `hyprctl dispatch movefocus l` is a SYNTAX ERROR under a Lua config
  (`return hl.dispatch(movefocus l)`). Use
  `hyprctl dispatch 'hl.dsp.focus({ direction = "left" })'`.
* The cursor dispatcher is `hl.dsp.cursor.move`, not `move_cursor`. The wrong
  name silently moves nothing.
* A nested Hyprland (AQ_BACKENDS=wayland in a headless labwc) has NO output;
  `hyprctl -i $sig output create headless` gives it one. `AQ_BACKENDS=headless`
  core-dumps on this GPU box.
* Its runtime dir also holds `wayland-N-awww-daemon.sock`. Filter for
  `wayland-<digits>` or a client gets a wallpaper daemon as its display.
* **wtype reaches wlroots compositors only.** Hyprland 0.56.2 and niri 26.04
  both accept the virtual keyboard and report `active keymap: error`; no bind
  fires. labwc and sway dispatch normally.
* wtype's `-M alt` sets the modifier state and sends no Alt_L key event; `-P
  Alt_L` sends the key without the modifier. A real keyboard does both, so
  tests send `-M alt -P Alt_L … -p Alt_L -m alt`.
* **labwc's `onRelease="yes"` never fires after a chord.** labwc-config(5):
  "the action should fire when the modifier is used WITHOUT another key".
  Measured: ALT down, Tab, ALT up → an `Alt_L onRelease` binding fires zero
  times. This is why labwc keeps its own switcher. sway's `bindsym --release
  Mod1+Alt_L` does not fire after a chord either.
* **`Toplevel.activate()` (wlr-foreign-toplevel) does not move focus in a
  nested Hyprland or a headless labwc**, with or without a virtual keyboard on
  the seat. That is why `WindowSwitcherService.commit()` asks
  `CompositorService.focusWindow()` first and falls back to `activate()`.
  Whether activate() works on a real desk is UNKNOWN — the AppDock has always
  relied on it, so probably, but nothing here proves it.
* **`transparent` is not the pass-through flag.** Hyprland's `t` means "cannot
  be shadowed by another bind" and still eats the key; `n`,
  `non_consuming = true`, is the one that lets the application see it. And the
  wrong name is SILENT: `nonConsuming`, `consume = false` and `ignore_mods` are
  all accepted by hl.bind and all leave `non_consuming: false`. Asserted per
  bind against a running instance now, because a config grep cannot tell them
  apart.
* **The single tap is a race between two processes.** ALT+Tab and the ALT
  release are each a spawn + `apex` + `qs ipc call`, ~60ms apart at the
  keyboard. The flag file has to be written by the HELPER on `next`, not by the
  shell at the end of that chain, or the release finds no flag and the commit
  is dropped. They can still overtake each other, so the shell remembers a
  commit that arrived with nothing open for 600ms and the `next` that opens
  within that window commits immediately.
* The live L16 binds SUPER+arrow TWICE (`movefocus l` on both `left` and
  `LEFT`): hyprlang's `unbind` is case-sensitive, so the shell's generated
  `unbind = SUPER, LEFT` never removed the seed's `bind = $mainMod, left`. One
  press moves focus two windows. The Lua tip cannot have this — it disables by
  handle through an uppercasing key function — and
  tests/test-apex-hypr-focus.sh now asserts it.

## Suites (all green here)

  apex-os     tests/test-apex-hypr-focus.sh        19 passed, 0 failed
  apex-shell  tests/run-window-switcher-test.sh    13 passed, 0 failed
  apex-shell  tests/run-switcher-activate-test.sh   7 passed, 0 failed
  apex-shell  tests/run-niri-keybinds-test.sh      16 inner assertions

Pre-existing failures NOT caused by this branch, identical on
roadmap/v2.2 @ 602a8376: tests/test-apex-input.sh 101/8, and
tests/test-apex-firstrun.sh 69/1 (it compares rc.xml against the STALE
/usr/share/apex-shell, because `$ROOT/../apex-shell` does not exist in a
worktree layout).

## NEXT

1. **Land both branches** through /var/tmp/apex-work/int-os and int-shell.
   They are independent of each other at the file level but not in behaviour:
   apex-os's rc.xml/Containerfile assertions reference nothing in apex-shell,
   but the shell half is what `apex shell switcher` reaches. Land the shell
   first if you land them separately, so an image built in between has a
   keybind pointing at a verb the vendored shell already answers.
2. **The image vendors apex-shell from a pin.** Until that pin moves past
   `task/focus-switcher`, the ALT+Tab keybind will exist and the shell will
   answer "Target not found". Check the pin before reporting this as shipped.
3. **What only a real session can confirm**, and it is worth asking Andre
   directly rather than assuming:
   - that holding ALT, tapping Tab and letting go actually commits. The
     release binding is verified as REGISTERED, never as fired.
   - that `Toplevel.activate()` works on a real compositor (the fallback path
     when `CompositorService.focusWindow` cannot match a window by app id and
     title — two identical terminals, deliberately refused as ambiguous).
   - that the switcher's tiles are legible at his scale. Nothing has ever
     rendered this overlay on a screen a person looked at.
4. Two things deliberately NOT done, either of which is a reasonable follow-up:
   - ALT+Tab is not in the Keybinds page. The shell's keybind model cannot
     express a key RELEASE, so rebinding there would move `next` and leave the
     commit on ALT. Teaching the model a release is the fix.
   - niri does not warp the pointer on focus (`warp-mouse-to-focus`). It does
     not need to — niri has no focus-follows-mouse by default, so nothing
     steals focus — but "super+arrow moves the mouse" is literally false there.
     One line in apex-input-apply's `gen_niri`, inside the `input {` block.
5. **labwc's ALT+Tab still has Andre's bug**, and it is written down rather
   than guessed at. labwc's switcher spans every desktop now but does not warp
   the pointer, so under `<followMouse>yes</followMouse>` the next mouse
   movement hands focus to whatever is under the cursor.
   `<action name="WarpCursor" to="window"/>` would fix it IF it ran after the
   cycle rather than at key-press time — and nothing here can tell which,
   because labwc has no IPC and the pointer cannot be read back headlessly. An
   unverified warp risks warping to the OLD window before the cycle starts.
   Settling it needs either a labwc source read or a client that reports its
   own pointer position.

---

## CONFIRMED ON ANDRE'S LIVE SESSION — 2026-09-21, by the orchestrator

Andre tested and reported: *"super+arrow not working the cursor isnt moving to
the other app"*. Measured on his running Hyprland 0.56.2 (session pid 2212),
not in a nested harness. **This suite's diagnosis was exactly right**, and the
live numbers are worth keeping because they show the symptom, which the comment
in `tests/test-apex-hypr-focus.sh` describes but does not quantify:

Two tiled windows, workspace 1, single 1920x1200 output. Left window middle
`(482, 617)`, right window middle `(1437, 328)`.

| dispatched | focus ends on | cursor ends at |
|---|---|---|
| `movefocus l` once | left window | `482, 617` — its middle. The warp works. |
| `movefocus l` twice (what SUPER+Left actually did) | **right window** | **`1437, 328`** — where it started |

With only two windows the second dispatch has nowhere further to go and **wraps
around**, so focus and pointer both return to the origin window. That is why it
read as "nothing happens" rather than as "it moved twice".

`hyprctl binds` on his session carried **nine** doubly-bound chords, not four:

    mod64 left/right/up/down   -> movefocus   x2   (literals `left` and `LEFT`)
    mod65 left/right/up/down   -> movewindow  x2   (swap, then swap back)
    mod64 print                -> exec        x2   (grimblast AND screenshot.sh)

So SUPER+SHIFT+arrows and SUPER+Print were broken the same way and had not been
reported — a window that moves and moves back reads as a window that did not
move.

`cursor:no_warps` on his session reads `int: 0 set: false`. The warp was never
the defect; the keybind fired twice. The line this unit landed in
`input-defaults.lua` is therefore correctly described in its own comment as
pinning a value against a future upstream flip, and **the load-bearing fix is
the bind table assertion in this suite**, not that line.

### Live workaround applied (not a repo change)

Andre is on the pre-Lua hyprlang seed and has no image carrying this work, so
the nine duplicate `bind` lines in `~/.config/hypr/hyprland.conf` were commented
out by hand, with a comment block explaining why, and `hyprctl reload` run.
Backup at `~/.config/hypr/hyprland.conf.bak-dupbinds-20260921`. Re-measured
after: **0 duplicated chords**. Andre confirmed: *"fixed"*.

That workaround is his machine only. The shipped fix is this unit plus
`apex-hypr-migrate`.

### The migrate path was checked, not assumed

`files/system/libexec/apex-hypr-migrate` handles this correctly and does not
reintroduce the double-fire for existing users:

- `convert_bind()` builds `combo = (canon_mods(mods), key.upper(), dispatcher, arg)`
  and drops it when it is in `APEX_DEFAULT_BINDS`. `APEX_DEFAULT_BINDS` carries
  `("SUPER", "LEFT", "movefocus", "l")`, so Andre's lower-case
  `bind = $mainMod, left, movefocus, l` is recognised as an APEX default and
  dropped rather than emitted alongside the Lua default.
- A deliberate rebind on a default combo emits `claim(mods, key)`, and
  `apex/keybindings.lua`'s `M.disable()` canonicalises with `:upper()` on the
  key (line 45), so `claim("SUPER", "left")` and `claim("SUPER", "LEFT")` reach
  the same handle.

The case-sensitivity trap that caused this is confined to hyprlang's `unbind`,
which the Lua path does not use.

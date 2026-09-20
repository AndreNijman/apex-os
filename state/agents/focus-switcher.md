# focus-switcher

Andre: "make it so that super+arrow actually moves the mouse so that the window
you move to actually stays in focus instead of instantly losing focus, also make
it so that you can also just use alt tab to cycle. build a full alt+tab switcher
thing and it goes for all windows including windows in other workspaces."

Worktrees: /var/tmp/apex-work/wt-focus-switcher (apex-os, task/focus-switcher)
           /var/tmp/apex-work/wt-focus-shell    (apex-shell, task/focus-switcher)

## MEASURED (2026-09-20, this machine, all read-only or headless)

* The main checkouts are stale. `roadmap/v2.2` ships Hyprland as **Lua**
  (`files/desktop/hypr/hyprland.lua` + `apex/*.lua`), not `hyprland.conf`.
* Andre's LIVE session is Hyprland on the OLD hyprlang seed. `hyprctl getoption`
  says `cursor:no_warps = 0 (set: false)`, `input:follow_mouse = 1 (set)`,
  `input:mouse_refocus = 1 (default)`.
* **The live machine binds SUPER+arrow TWICE.** `hyprctl binds` lists both
  `modmask 64 'left' -> movefocus l` (from hyprland.conf) and
  `modmask 64 'LEFT' -> movefocus l` (from the shell's generated
  ApexShellKeybinds.conf). The generated file emits `unbind = SUPER, LEFT`,
  and hyprlang's `unbind` is CASE-SENSITIVE on the key name, so it removes
  nothing. Reproduced in a nested Hyprland: both entries survive.
  The Lua tip does not have this defect — it disables by handle, and
  `M.key()` uppercases, so "left" and "LEFT" are the same key.
* Hyprland 0.56.2 **does** warp the cursor on directional focus with the
  default `cursor:no_warps = false`: measured, cursor 480,270 -> 240,270 =
  the centre of the newly focused window.
* **`hyprctl dispatch movefocus l` is a syntax error under a Lua config.** It is
  wrapped as `return hl.dispatch(movefocus l)`. The working form is
  `hyprctl dispatch 'hl.dsp.focus({ direction = "left" })'`.
* Nested Hyprland (AQ_BACKENDS=wayland inside a headless labwc) starts with NO
  output; `hyprctl -i $sig output create headless` gives it a 1920x1080 one.
  `AQ_BACKENDS=headless` still core-dumps on this box.
* **wtype (real key events) works on headless labwc, NOT on nested Hyprland.**
  Hyprland accepts the virtual keyboard (`hl-virtual-keyboard-wtype` appears in
  `hyprctl devices`) but reports `active keymap: error` and no bind fires.
* labwc 0.9.6 has `<action name="WarpCursor" to="window"/>`, and
  `NextWindow workspace="all"`. It still has no directional-focus action.
* labwc 0.9.6 has `<keybind ... onRelease="yes">`.
* **niri 26.04 ships a native recent-windows (Alt-Tab) switcher** with
  hold-and-release, MRU, previews and an open delay.

## NEXT

See the commits on both branches. If this card still says NEXT after them,
re-read the "MEASURED" block first — every line of it cost a probe.

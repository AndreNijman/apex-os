# APEX-OS v2.2 - Current UI / UX issues

These issues come from the user's current installed APEX Shell experience and the
2026-09-06 screenshot in `evidence/config-display-2026-09-06.png`.

They are P0 unless stated otherwise.

## UI-001 - Dashboard top navigation overlaps / does not scale cleanly

User report: buttons in the top dashboard/settings navigation overlap.

Relevant current structure: Dashboard has Home, System, Agents, Tasks, Apps and Config.
The public source currently uses a 900px page width for major dashboard pages.

Expected:
- responsive layout;
- no overlapping text/icons;
- no clipped hit targets;
- no tiny-font workaround;
- works across supported display scaling.

Roadmap: P0-017.

## UI-002 - Display Apply confirmation does not appear

User report: after changing monitor configuration and pressing Apply, the 15-second
confirmation dialog does not appear, so the user cannot confirm the change and loses
the intended configuration.

Expected:
- temporary apply succeeds or fails visibly;
- 15-second Keep/Revert dialog appears on a safe active output;
- timeout reverts;
- Keep persists;
- failure keeps staged intent and reports what happened.

Roadmap: P0-018.

## UI-003 - Input settings do nothing

User report: changing Input configuration in APEX Shell has no effect.

Expected:
- every visible control has a real backend;
- effective state is read back;
- unsupported controls are disabled with explanation;
- Hyprland/niri/labwc have tested adapters.

Roadmap: P0-019.

## UI-004 - Scrolling over sliders changes values

The current shared `CfgSlider.qml` explicitly installs a `WheelHandler` for Mouse and
TouchPad and calls the value-change path on wheel events.

User requirement:
- mouse wheel and touchpad scrolling NEVER adjust settings value bars;
- scrolling must scroll the page;
- values require intentional click/drag or keyboard action.

Roadmap: P0-020.

## UI-005 - Agents render only in white

User report: APEX agents display only in white.

Treat this as both:
1. Agent Center theme/state color correctness.
2. Embedded PTY/TUI ANSI color preservation if the white rendering is terminal output.

Roadmap: P0-021.

## UI-006 - Agents/Workspaces are not self-explanatory

User report: even the owner does not know how to use all Agents/Workspaces features.

Expected:
- permanent Help entry at top of Agents;
- first-run onboarding;
- CLI + GUI explanations;
- workspaces/worktrees/checkpoints/remote/security explained from zero knowledge.

Roadmap: P0-022.

## UI-007 - Blueprint experience is confusing

Expected redesign:
- explain Blueprint;
- create/import/sync;
- current vs desired preview;
- simple grouped settings;
- advanced raw config optional;
- explicit destructive diff.

Roadmap: P1-048.

## UI-008 - Gaming settings are confusing / low quality

Expected:
- simple master optimization toggle;
- small set of understandable common toggles;
- current active policy shown;
- advanced expert tuning separated.

Roadmap: P1-049.

## UI-009 - Need persistent Always Unrestricted agent default

Expected:
- Config -> Agents;
- password-gated toggle to make all NEW managed agents default to APEX unrestricted-user
  sandbox mode;
- OFF restores normal protected default;
- does not automatically grant root or expose brokered secrets;
- current session modes remain truthful.

Roadmap: P0-016.

## UI-010 - Settings semantics need consistency

Display currently exposes staged/apply/save/revert concepts. These semantics must be
shared across settings so navigation cannot silently lose intended changes.

Roadmap: P0-023.

## Evidence

`evidence/config-display-2026-09-06.png`


## UI-011 - Hyprland legacy `.conf` compositor configuration is deprecated

Current runtime warning: APEX is still using Hyprland's legacy hyprlang `.conf`
configuration path, which is deprecated and is expected to stop working when legacy
support is removed.

This is a P0 compatibility bug, not merely a warning cleanup.

Expected:
- `hyprland.lua` is the canonical active APEX compositor config;
- APEX-generated monitor/input/keybind/rule modules are Lua-native;
- Settings writes Lua-native configuration;
- existing APEX users receive a backed-up, idempotent migration;
- no user custom keybind/rule/display state is silently dropped;
- `hyprctl configerrors` is clean;
- CI prevents old-format generation from returning.

Roadmap: P0-025.

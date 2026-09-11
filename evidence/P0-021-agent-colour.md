# P0-021 — Agent Center and agent terminal colour: criterion by criterion

Re-derived 2026-09-12 (round 13) by the `p0-finish` unit, read-only, against
apex-shell `task/p0-018-display-finish` @ `7004c2d`. The probe that was
reporting on this when the previous session hit its usage limit did not
survive it, so every figure below was measured again rather than inherited.

**Headline: four of the five criteria are closed or vacuous. One real gap
remains, and the roadmap's current evidence misfiles the reason for it.**

## 1. "Agent cards, icons, state badges, and text use APEX theme tokens rather than hard-coded white" — CLOSED

The roadmap's evidence gives "those 212 sites" as one of the two reasons this
item is still partial. **None of the 212 is in an agent file.**

`check-color-tokens.sh` ratchets 212 `color:` bindings of the form
`Qt.rgba(1, 1, 1, α)` across `src/`. Reproduced independently — 212, matching
the script's `EXPECT_WHITE_FG` exactly — and then split by surface:

| Surface | Count |
|---|---|
| Agent Center and agent-related files | **0** |
| Settings / config pages (`src/services/config_tab/**`) | 20 |
| Everything else (Kanban 24, AudioControl 15, AppLauncher 11, WallpaperPopup 11, UpdatePopup 10, WifiTab 9, …) | 192 |

Because "hard-coded white" is only one spelling, the search was widened to
every hard-coded colour form — `"#rrggbb"`, `"white"`, `"black"`, `Qt.rgba(`,
`Qt.hsla(` — across all 15 files of `src/services/agents/` plus
`AgentService.qml`, `agentstate.js` and `AgentsPage.qml`:

- **30 hits, 28 of them `Qt.rgba(Theme.<token>.r, …, α)`** — a theme token
  with an alpha, which is what the criterion asks for.
- `StateBadge.qml:70` derives from `badge.toneColor`, itself
  `Theme[AgentState.token(state)]` — token-derived through a property, not a
  violation.
- `AgentHelpPanel.qml:57` `Qt.rgba(0, 0, 0, 0.45)` — a scrim behind a panel,
  not a foreground.

**Zero hard-coded white on the surface this criterion names.** The 212 are a
shell-wide *light-mode* remainder and belong to whatever item owns light mode.
They are a real reason light mode is not shippable; they are not a reason
P0-021 is unfinished.

Two holes that a directory-name search would have left, closed explicitly
rather than assumed shut:

- **Consumers outside `src/services/agents/`.** The criterion names badges,
  and the roadmap's standing remainder is about badge weights *on a panel* —
  a surface that would not be matched by path. Every file referencing
  `AgentState.`, `StateBadge` or `AgentService.` from elsewhere
  (`RecoveryService.qml`, `search/AgentsProvider.qml`, `search.js`,
  `config_tab/pages/AgentsPage.qml`) was run through the same widened colour
  grep: **0 non-token hits.**
- **Icons.** The 15 agent files reference no `source:` image asset at all —
  their icons are glyphs, not files — and no SVG under `src/assets/` carries
  a hard-coded `fill="#fff"` / `fill="white"`.

So criterion 1 closes with no qualifier.

## 2. "Working/waiting/blocked/failed/completed visually distinct in both palettes" — CLOSED

`run-agent-state-render-test.sh` drives `agent-state-render-test.qml` through
the real `Theme` in a headless compositor: **17 of 17**, both palettes, worst
light contrast **4.81:1**. The 17 are 7 runtime states × dark/light plus the
palette-recognition and re-resolution checks.

## 3. "If PTY/TUI output is embedded, ANSI 16/256-colour, bold/dim/underline and truecolor are preserved" — VACUOUS BY ITS OWN `If`, and deliberately so

**No PTY is embedded anywhere in the shell.** No `QMLTermWidget`, no
`TerminalDisplay`, no `forkpty`/`openpty`/`ptmx`, no terminal component of any
kind in the tree. This is a stated design decision, not an omission —
`AgentCenter.qml`'s own header:

> A SUPERVISOR AND A NAVIGATOR … not a replacement for the terminal. …
> Clicking a session focuses its real terminal; there is no output pane, no
> prompt box and no way to talk to an agent from the shell, because a second
> and worse terminal is not worth building.

`SessionRow`/`RemoteSessionRow` render only structured fields off a
`SessionInfo` record obtained through the `apex` CLI — agent name, session id,
project, elapsed time, exit code, sandbox chip, worktree chip, state badge.
No raw stdout is ever displayed. `focusTerminal(id)` execs
`/usr/libexec/apex-agent-focus` to focus the *real* terminal window.

The only ANSI handling in the tree is `SystemStats.qml:97`'s `stripAnsi()`,
which strips escapes out of a system-info command's output before parsing it
into stat rows — unrelated to agents, and stripping rather than rendering.

**This criterion should be recorded N/A with that reason**, not left looking
unmet. There is no code path in which the shell flattens agent colour to
white, because there is no code path in which the shell renders agent bytes.

## 4. "Claude, OpenCode, Codex, Gemini and generic PTY sessions render consistently" — STRUCTURALLY TRUE, UNASSERTED

**This is the one real gap.**

Agent identity reaches exactly one thing in the entire shell: `AGENT_NAMES` in
`src/services/agentstate.js`, a display-name map (`claude: "Claude"`,
`opencode: "OpenCode"`, `codex: "Codex"`, `gemini: "Gemini"`,
`kimi: "Kimi"`, `generic: "Agent"`). It affects the label string and nothing
else. `StateBadge.qml` derives tone, weight and fill purely from
`sessionState` —

    readonly property color toneColor: Theme[AgentState.token(badge.sessionState)]

— and **takes no agent parameter at all**. A search across `src/` for
`claude|opencode|codex|gemini|kimi` returns only prose, help text and adapter
id lists; there is no per-agent colour, icon or accent map anywhere.

So the five render identically by construction. But nothing asserts it: all 17
render cases are a *state × palette* matrix, and the fixture's `session()`
factory hardcodes `agent: "claude"` for every one of them. The day someone
adds a per-agent tint, no test notices.

### The missing assertion, specified

In `tests/agent-state-render-test.qml`: five `session()` fixtures differing
only in `agent` — `claude`, `opencode`, `codex`, `gemini`, `generic` — and,
for a fixed state, assert an identical `toneColor` and identical badge
geometry across all five, with the label text the only value that differs.

Mutation that must fail it: give one agent its own tone in `StateBadge`.

Cost: one test case. No new harness — the headless render harness exists and
already drives the real components.

## 5. "Contrast meets accessibility targets" — CLOSED

Same 17/17: every state clears 4.5:1 against the card and hovered backgrounds
in both palettes.

## Blockers that are not code

**The branch is unmerged, and a second branch claims the same item.**

- `task/p0-018-display-finish` @ `7004c2d` — 6 commits ahead of apex-shell
  `roadmap/v2.2`, which has moved **70** commits past their merge base
  (`a85de29`). Contained by no other branch or tag. With the integration
  agent; per the `p0-018b` card only `DisplayPage.qml` conflicts.
- `task/p0-021-agent-colors` @ `68c4280` — a different branch that also
  addresses this item, including `9200d3f fix(agents): four of seven agent
  states rendered as the palette's foreground` and `37fa439 fix(theme):
  status tokens had one value, picked on a dark surface`. Diverges at
  `9141ea7`; merged into neither `roadmap/v2.2` nor the branch above.

Which of the two is "the" P0-021 fix has not been reconciled by anyone. The
`p0-finish` unit deliberately wrote no commit here: doing so would have
changed what the integration agent is merging and would have picked a winner
between the two branches by side effect.

## Status

`partial`, and much closer than the roadmap records. Criteria 1, 2, 3 and 5
are closed or N/A with the measurements above. Criterion 4 needs one test case
on whichever branch wins. The "212 sites" remainder in the current evidence
is real but belongs to light mode, not to this item; the "no human has looked
at the badge weights on a real panel" remainder still stands and still needs
Andre.

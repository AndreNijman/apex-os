# P3 — progress and resume point

## What P3 is, and how it was derived

The roadmap's priority table stops at P2. Its §23 implementation order has ten
phases, and every one of them is now done: phases 1–4 were P0, 5–8 were P1,
9–10 were P2. So "P3" is not a label the roadmap uses — it is **whatever the
ten phases do not cover**.

Mapping all 27 sections against the phases:

| section | phase | state |
| --- | --- | --- |
| §2, §3 agent runtime, TUI agents | 1 | P0 |
| §4 agent permissions and security | 2 | P0 |
| §5, §6, §7 checkpoints, projects, worktrees | 3 | P0 |
| §18 labwc Floating | 4 | P0 |
| §16, §17 plugin platform, compositor independence | 5 | P1 |
| §8, §9 capsules, universal package command | 6 | P1 |
| §10 declarative blueprint | 7 | P1 |
| §11, §12, §13 modes, gaming, workload manager | 8 | P1 |
| §14, §20 local AI, device handoff | 9 | P2 |
| §22 boot architecture | 10 | P2 |
| **§15 unified search and command surface** | **none** | **P3** |
| **§19 recovery, repair and disposable execution** | **none** | **P3** |
| **§21 make APEX understand intent** | **none** | **P3** |
| §1, §24, §25, §26, §27 | — | direction, criteria, anti-goals, architecture diagram, positioning — nothing to implement |

So P3 is exactly three sections. Two are substantial features; the third is
one missing concept.

## §21 is one concept, not eight

§21's example is a **Task**:

```
Task: Fix APEX installer bug
  Project:     apex-os
  Environment: Fedora build capsule
  Windows:     editor, browser, logs
  Agents:      Claude, Codex reviewer
  Checkpoint:  before changes
  Permissions: project files, GitHub apex-os, network
```

Seven of the eight concepts it lists already exist and shipped in P0–P2:
Projects (§6), Agents (§2), Environments (§8 capsules), Worktrees (§7),
Checkpoints (§5), Capabilities/permissions (§4), Trusted devices (§20). The one
that does not exist is **Task** — the binder that references the others and can
be put down and resumed.

That is the whole of §21's implementable content, and it is why §21 is smaller
than it looks: it is not eight features, it is one record type plus the verbs
that make it worth having.

## §15 already has its extension point

P1's §16 work shipped a **`launcher-provider`** plugin extension point
(apex-shell, "feat(plugins): launcher-provider and quick-settings-tile"). So
§15's "provider API so third parties can add results" exists. What §15 asks for
beyond it is the set of **built-in** providers — apps, files, settings, windows,
clipboard, calculator, commands, projects, agents, SSH hosts, package search —
and they should go through the same contract a third-party provider uses, not a
privileged side channel. If a built-in needs something the plugin contract
cannot express, that is a finding about the contract.

§15's fourth bullet is the one with teeth: *"Actions should have clear
permissions and previews before destructive system changes."* A launcher that
can `install Blender` or `restart Bluetooth` is a command surface with system
reach, and the preview is what stops a fuzzy match from doing something
irreversible.

## Standing constraints, unchanged

* Nothing merged to `apex-os/main` or `apex-shell/main` until Andre asks for
  the single image build. Merge targets so far: apex-os **#35** (P1), apex-os
  **#36** (P2), apex-shell **#15** (P2 §20 shell half).
* Never cause a polkit or keyring password prompt.
* Never run a test that opens a window on the developer's desktop.
  `APEX_LABWC_SESSION_TESTS` stays unset; `test-apex-firstrun.sh` now asserts
  its own socket count for this reason.
* The katana on the LAN is free for builds, VMs, clippy and shellcheck.
* Conventional Commits, zero AI attribution, author AndreNijman only.

## Status

| phase | state |
| --- | --- |
| P3 scope derived and written down | this file |
| 15.x §15 unified search and command surface | not started |
| 19.x §19 recovery, repair, disposable execution | not started |
| 21.x §21 the Task concept | not started |

## The UI mismatch had one cause, and it was measurable

Andre reported that the Agent tab "didn't match apex shell at all". That turned
out to be a single systematic error rather than a matter of taste.

`Theme` exposes two scalers, and they are not interchangeable:

```
px(v) = Math.round(v * scale)
fs(v) = Math.max(7, Math.round(v * scale))     <- text legibility floor
```

The floor exists because text below about 7px is illegible at any DPI. It is
correct for a font size and **wrong for everything else**, because it silently
clamps small geometric values up to 7.

Every geometric property in the Agent Center used `fs()` — 42 call sites across
seven files. Sixteen were under the floor:

| written | rendered | error |
| --- | --- | --- |
| `spacing: Theme.fs(2)` | 7px | 3.5× too loose |
| `spacing: Theme.fs(3)` | 7px | 2.3× |
| `leftMargin: Theme.fs(4)` | 7px | 1.75× |
| `radius: Theme.fs(3)` | 7px | 2.3× too round |
| `radius: Theme.fs(5)`, `fs(6)` | 7px | — |

So the tab's spacing rhythm and corner radii matched nothing else in the shell.
Nothing was broken, every test passed, and no check looked: the wrong function
was simply being called.

**It was also spreading.** The remote-agents section added hours earlier copied
the surrounding idiom and contributed twelve of the 42. A wrong local convention
reproduces itself, which is the argument for a check rather than a one-off fix.

Fixed in apex-shell `ad53f5d` on `p2/remote-agent-status` (PR #15). All 32
`font.pixelSize` calls still use `fs()`, asserted during the conversion rather
than assumed.

`tests/check-scale-tokens.sh` prevents recurrence — 5 assertions, wired into
`ci.yml`, anchored to a property assignment so prose cannot satisfy it, and
self-testing in both directions. Its strongest control is not a synthetic
mutant: **run against the tree as it was before the fix, it fails.**

### The polish debt that remains, measured

**66 hardcoded hex colours** in QML outside `src/theme/`. Worst: `UpdatePopup`
8, `KanbanBoard` 6, `CenterContent` 6, `KeybindsPage` 4, `BatteryStatus` 4.

For contrast, the discipline that *is* fully applied: **zero** raw
`font.pixelSize` literals in the whole tree. The codebase can hold a standard;
colour simply never got one.

Delegated with the instruction that this is judgement rather than
find-and-replace — a literal mapped onto a token that merely looks similar
today produces a shell that breaks the moment the palette changes, which is
worse than the literal because the mistake is invisible.

## The endgame, and the authorization for it

Andre has now authorized what was held back all session: **when everything is
complete, merge everything and kick off the real final image build.**

Merge order, and why:

1. **apex-shell first.** PR #15 (P2 §20 + the scaler fix), then the P3 shell
   branches. apex-shell has no image build of its own, and `Containerfile.base`
   resolves the shell by `git ls-remote refs/heads/main` **at build time** — so
   the shell must be on `main` before the OS build starts, or the image vendors
   a shell without this work.
2. **apex-os P1 #35**, then **P2 #36**, then P3. P2 branched from P1 and P3 from
   P2, so this is the order they were built in.
3. `build-image.yml` fires on push to `main` for paths
   `Containerfile*|files/**|apexd/**|config/**|kernel/**|.github/**`. So
   **merging to `main` IS the final build** — no separate dispatch needed.

Before merging, honestly: every suite green, `cargo clippy --all-targets
--locked -- -D warnings` clean, shellcheck clean, and nothing that opens a
window on his desktop. Do not merge red.

## The polish pass found a worse bug than the one it was sent for

Branch apex-shell `p3/ui-polish`, four commits, pushed.

**Two home-dashboard cards never followed the wallpaper.**
`src/services/home/CalendarCard.qml` and `ProfileCard.qml` had
`Qt.rgba(166/255, 208/255, 247/255, α)` and `Qt.rgba(205/255, 214/255, 244/255, α)`
compiled in. Those are exactly `#a6d0f7` and `#cdd6f4` — the **fallback** values
`ColorLoader.qml` declares at lines 17–18 for `active` and `text`. Verified
directly against both files.

So those two cards rendered the fallback palette permanently, while every other
surface tracked matugen from the wallpaper. They looked correct on one palette
and were silently wrong on every other. That is the same shape as the scaler
bug: correct on the developer's configuration, invisible to every test, wrong
everywhere else.

**A second systematic layer, invisible to the audit that found the first.**
The 66 hex literals were the visible half. Writing the check exposed 45
occurrences of `Qt.rgba(248/255, 113/255, 113/255, α)` — hex with the digits
filed off, which no grep for `#` can see. 43 became tokens.

**Seven new tokens**, sourced from the dominant existing value at each role so
no existing install shifts: `danger` `#f87171` (12 of 24 red sites), `warning`
`#f5c47a` (7 of 13), `success` `#a6e3a1`, plus `fixedLight`, `fixedDark`,
`dangerFill`, `dangerFillHover`.

They are deliberately **not** wired to matugen, and the argument is worth
keeping: M3 guarantees an `error` role but no amber and no green, so a red that
followed the wallpaper would sit beside a green that could not. `wsUrgent` is
the precedent — `Colors.qml` has held a fixed status red since it was written.

**Two more mismatches fell out.** `NotificationToast` and `NotificationList`
disagreed about normal urgency, so a notification's accent bar changed colour as
the toast expired into the list. And `AppearancePage`'s wallpaper grid is a
second copy of `WallpaperPopup`'s thumbnail ring that snaps where the original
eases, at radius 9 against 10.

### One visible behaviour change, flagged rather than buried

`BatteryStatus` used `#ff4444 / #ff6b00 / #ffcc00 / #ff9900` at 5 / 10 / 20 /
30 percent. That is **not monotonic**: 20% drew a calm yellow while 30% drew a
more urgent orange, so the icon became *less* alarming as the battery drained
past 30. Accretion, not a scale.

Both battery surfaces now use two tokens — `danger` at ≤10, `warning` at ≤30.
Monotonic and correct, but it **changes what the bar looks like between 10% and
30%**, and it trades two steps of granularity for correctness. Kept, because a
non-monotonic warning scale is a defect; recorded here because it is the one
change in this pass a person would notice without being told, and reverting that
hunk alone restores the four steps.

Four monotonic steps would be better than two, but choosing the two intermediate
colours means inventing a ramp without being able to see it — which is the
"similar-looking token" mistake this pass exists to avoid.

### The check

`tests/check-color-tokens.sh`, 15 assertions, wired into `ci.yml`. Its allowlist
is **(file, colour) pairs each carrying a stated reason**, with the total
asserted exactly — a per-file tally would pass while someone swapped one literal
for another in the same file. Colours are extracted by a comment- and
string-aware scanner rather than a line grep, so prose can neither satisfy it
nor trip it.

Negative control, run here rather than taken on trust: against
`p2/remote-agent-status` it reports **6 passed, 9 failed**; against the fixed
tree, **15 passed, 0 failed**.

| suite | before | after |
| --- | --- | --- |
| `node tests/*-test.js` | 494 / 0 | 494 / 0 |
| `tests/check-*.sh` | 284 / 0 | 299 / 0 |
| `qmllint-qt6` warnings across the 34 changed files | — | unchanged |

### Deliberately not attempted

Radius was measured and left alone: the large literals are all hand-written
`height/2` pills, containers use `Theme.cornerRadius`, small controls sit on a
6–10 scale. No surface speaks its own dialect the way the Agent Center did. The
real smell there is that a hand-maintained `height/2` breaks silently when a
height changes — a follow-up, not blind churn.

Shared-component migration was not attempted, by instruction: it is a large
refactor with real regression risk, and the reported defect was not caused by it.

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

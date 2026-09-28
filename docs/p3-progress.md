# P3: progress and resume point

## What P3 is, and how it was derived

The roadmap's priority table stops at P2. Its §23 implementation order has ten
phases, and all ten are now done: phases 1-4 were P0, 5-8 were P1, 9-10 were
P2. The roadmap never uses the label "P3". Here it means **whatever the ten
phases do not cover**.

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
| §1, §24, §25, §26, §27 | n/a | direction, criteria, anti-goals, architecture diagram, positioning; nothing to implement |

That leaves P3 with exactly three sections. Two are substantial features; the
third is one missing concept.

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

Seven of the eight concepts it lists already exist and shipped in P0-P2:
Projects (§6), Agents (§2), Environments (§8 capsules), Worktrees (§7),
Checkpoints (§5), Capabilities/permissions (§4), Trusted devices (§20). The
missing one is **Task**: the binder that references the others, which you can
put down and resume.

That is the whole of §21's implementable content, so §21 is smaller than it
looks: one record type plus the verbs that make it worth having, rather than
eight features.

## §15 already has its extension point

P1's §16 work shipped a **`launcher-provider`** plugin extension point
(apex-shell, "feat(plugins): launcher-provider and quick-settings-tile"). So
§15's "provider API so third parties can add results" exists. Beyond it, §15
asks for the set of **built-in** providers (apps, files, settings, windows,
clipboard, calculator, commands, projects, agents, SSH hosts, package search),
and they should go through the same contract a third-party provider uses
instead of a privileged side channel. If a built-in needs something the plugin
contract cannot express, that is a finding about the contract.

§15's fourth bullet carries the most weight: *"Actions should have clear
permissions and previews before destructive system changes."* A launcher that
can `install Blender` or `restart Bluetooth` is a command surface with system
reach, and the preview stops a fuzzy match from doing something irreversible.

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

Andre reported that the Agent tab "didn't match apex shell at all". The cause
was a single systematic error rather than a matter of taste.

`Theme` exposes two scalers, and they are not interchangeable:

```
px(v) = Math.round(v * scale)
fs(v) = Math.max(7, Math.round(v * scale))     <- text legibility floor
```

The floor exists because text below about 7px is illegible at any DPI. It is
correct for a font size and **wrong for everything else**, because it silently
clamps small geometric values up to 7.

Every geometric property in the Agent Center used `fs()`: 42 call sites across
seven files. Sixteen were under the floor:

| written | rendered | error |
| --- | --- | --- |
| `spacing: Theme.fs(2)` | 7px | 3.5× too loose |
| `spacing: Theme.fs(3)` | 7px | 2.3× |
| `leftMargin: Theme.fs(4)` | 7px | 1.75× |
| `radius: Theme.fs(3)` | 7px | 2.3× too round |
| `radius: Theme.fs(5)`, `fs(6)` | 7px | n/a |

The tab's spacing rhythm and corner radii therefore matched nothing else in the
shell. Nothing was broken, every test passed, and no check looked: the code was
calling the wrong function.

**It was also spreading.** The remote-agents section, added hours earlier,
copied the surrounding idiom and contributed twelve of the 42. A wrong local
convention spreads through copying, which is the argument for a check rather
than a one-off fix.

The fix is apex-shell `ad53f5d` on `p2/remote-agent-status` (PR #15). All 32
`font.pixelSize` calls still use `fs()`; the conversion asserted this rather
than assuming it.

`tests/check-scale-tokens.sh` prevents recurrence: 5 assertions, wired into
`ci.yml`, anchored to a property assignment so prose cannot satisfy it, and
self-testing in both directions. Its strongest control uses the real tree in
place of a synthetic mutant: **run against the tree as it was before the fix,
it fails.**

### The polish debt that remains, measured

**66 hardcoded hex colours** in QML outside `src/theme/`. Worst: `UpdatePopup`
8, `KanbanBoard` 6, `CenterContent` 6, `KeybindsPage` 4, `BatteryStatus` 4.

For contrast, one discipline *is* fully applied: **zero** raw
`font.pixelSize` literals in the whole tree. The codebase can hold a standard,
and colour never had one.

Delegated with the instruction that this is judgement rather than
find-and-replace. A literal mapped onto a token that only looks similar today
produces a shell that breaks the moment the palette changes, which is worse
than the literal because nobody can see the mistake.

## The endgame, and the authorization for it

Andre has now authorized what was held back all session: **when everything is
complete, merge everything and kick off the real final image build.**

Merge order, and why:

1. **apex-shell first.** PR #15 (P2 §20 + the scaler fix), then the P3 shell
   branches. apex-shell has no image build of its own, and `Containerfile.base`
   resolves the shell by `git ls-remote refs/heads/main` **at build time**, so
   the shell must be on `main` before the OS build starts, or the image vendors
   a shell without this work.
2. **apex-os P1 #35**, then **P2 #36**, then P3. P2 branched from P1 and P3 from
   P2, so this is the order they were built in.
3. `build-image.yml` fires on push to `main` for paths
   `Containerfile*|files/**|apexd/**|config/**|kernel/**|.github/**`. Therefore
   **merging to `main` IS the final build**, with no separate dispatch.

Before merging, verify rather than assume: every suite green, `cargo clippy
--all-targets --locked -- -D warnings` clean, shellcheck clean, and nothing
that opens a window on his desktop. Do not merge red.

## The polish pass found a worse bug than the one it was sent for

Branch apex-shell `p3/ui-polish`, four commits, pushed.

**Two home-dashboard cards never followed the wallpaper.**
`src/services/home/CalendarCard.qml` and `ProfileCard.qml` had
`Qt.rgba(166/255, 208/255, 247/255, α)` and `Qt.rgba(205/255, 214/255, 244/255, α)`
compiled in. Those are exactly `#a6d0f7` and `#cdd6f4`, the **fallback** values
`ColorLoader.qml` declares at lines 17-18 for `active` and `text`. Verified
directly against both files.

Those two cards therefore rendered the fallback palette permanently, while
every other surface tracked matugen from the wallpaper. They looked correct on
one palette and were silently wrong on every other. The scaler bug had the
same shape: correct on the developer's configuration, invisible to every test,
wrong everywhere else.

**A second systematic layer, invisible to the audit that found the first.**
The 66 hex literals were the visible half. Writing the check exposed 45
occurrences of `Qt.rgba(248/255, 113/255, 113/255, α)`, hex with the digits
filed off, which no grep for `#` can see. 43 of them became tokens.

**Seven new tokens**, sourced from the dominant existing value at each role so
no existing install shifts: `danger` `#f87171` (12 of 24 red sites), `warning`
`#f5c47a` (7 of 13), `success` `#a6e3a1`, plus `fixedLight`, `fixedDark`,
`dangerFill`, `dangerFillHover`.

They are deliberately **not** wired to matugen, for a reason worth keeping: M3
guarantees an `error` role but no amber and no green, so a red that followed
the wallpaper would sit beside a green that could not. `wsUrgent` is the
precedent: `Colors.qml` has held a fixed status red since it was written.

**The pass turned up two more mismatches.** `NotificationToast` and
`NotificationList` disagreed about normal urgency, so a notification's accent
bar changed colour as the toast expired into the list. `AppearancePage`'s
wallpaper grid is a second copy of `WallpaperPopup`'s thumbnail ring that snaps
where the original eases, at radius 9 against 10.

### One visible behaviour change, flagged rather than buried

`BatteryStatus` used `#ff4444 / #ff6b00 / #ffcc00 / #ff9900` at 5 / 10 / 20 /
30 percent. That is **not monotonic**: 20% drew a calm yellow while 30% drew a
more urgent orange, so the icon became *less* alarming as the battery drained
past 30. The steps had accreted one at a time instead of forming a scale.

Both battery surfaces now use two tokens: `danger` at ≤10, `warning` at ≤30.
The result is monotonic and correct, but it **changes what the bar looks like
between 10% and 30%**, and it trades two steps of granularity for correctness.
The change stays, because a non-monotonic warning scale is a defect. It is
recorded here because it is the one change in this pass a person would notice
without being told, and reverting that hunk alone restores the four steps.

Four monotonic steps would be better than two, but choosing the two
intermediate colours means inventing a ramp without being able to see it,
which is the "similar-looking token" mistake this pass exists to avoid.

### The check

`tests/check-color-tokens.sh`, 15 assertions, wired into `ci.yml`. Its
allowlist is **(file, colour) pairs each carrying a stated reason**, with the
total asserted exactly, because a per-file tally would pass while someone
swapped one literal for another in the same file. A comment- and string-aware
scanner extracts the colours instead of a line grep, so prose can neither
satisfy it nor trip it.

Negative control, run here rather than taken on trust: against
`p2/remote-agent-status` it reports **6 passed, 9 failed**; against the fixed
tree, **15 passed, 0 failed**.

| suite | before | after |
| --- | --- | --- |
| `node tests/*-test.js` | 494 / 0 | 494 / 0 |
| `tests/check-*.sh` | 284 / 0 | 299 / 0 |
| `qmllint-qt6` warnings across the 34 changed files | n/a | unchanged |

### Deliberately not attempted

The pass measured radius and left it alone: the large literals are all
hand-written `height/2` pills, containers use `Theme.cornerRadius`, and small
controls sit on a 6-10 scale. No surface speaks its own dialect the way the
Agent Center did. The smell worth fixing there is that a hand-maintained
`height/2` breaks silently when a height changes. That is a follow-up, not a
reason for blind churn.

Shared-component migration was not attempted, by instruction: it is a large
refactor with real regression risk, and it did not cause the reported defect.

## The §24 audit, and what it changed

APEX was audited against the roadmap's own definition of done (§24's ten user
rows and §25's non-negotiable rules), deliberately looking for claims that were
true for the wrong reason. Verdict: **2 of 10 rows met, 6 partly, 2 not met**,
and one §25 rule broken outright. The audit is at
`scratchpad/section24-audit.md`.

The audit was only worth running if it found something. It found the
following, all since fixed.

### `apex game status` was reporting the plan, not the outcome

`irqs_steered` was `steer.len()`, computed **before any write**. The applier
threw every error to stderr and discarded `write_tolerant`'s landed/refused
bool. On a machine that refuses every affinity write, status still said
"N IRQs steered", the only place in the system that stated something untrue.

`SysWriter::apply` now returns `Outcome::Landed | Refused(reason)`. Tolerance is
unchanged: a refused knob is still `Ok` and still must not abort a plan.
`irqs_steered` now means landed, alongside `irqs_attempted` and `irqs_refused`
and a note carrying the kernel's reason. The tests catch all four mutants.

The fix also exposed why no test existed: game mode enumerated from a hardcoded
`/proc/irq` inside an otherwise fixture-rooted daemon, so any test of it could
only read the host's real interrupts. `Ctx` gained `proc_irq_root`.

**The audit was wrong on one detail and the fix says so**: "every caller
discards" the bool was false, because `fan_safe_restore` consumed it to drive
its safety ladder.

### Three CI-wired suites skipped and exited 0

`test-privilege-requests.sh`, `test-project-layout.sh` and
`test-secret-broker.sh` each printed `0 passed, 0 failed (skipped)` and exited 0
when `cargo` was missing. They now exit 2 naming the tool, matching the rule
their sibling suites already state: a missing prerequisite is a failure, never a
skip.

That fix turned up two more problems. None of the three checked for `python3`
despite parsing JSON with it. And `test-project-layout.sh`'s
"a dry run starts nothing" compared `pgrep -c -x sleep || echo 0` on both
sides: with no `pgrep` both read `"0"` and the assertion passed **having
measured nothing**, a second vacuous pass found while fixing the first.

`test-secret-broker.sh` keeps one honest skip (without bubblewrap it cannot
build a confined session at all), but its CI step now sets
`APEX_REQUIRE_SANDBOX`, so the job cannot be green having skipped §4's central
assertion.

The aggregate gate itself was examined and left alone, with the reasoning
recorded: a *job* marked skipped comes from this workflow's own path filters and
is legitimately success; a *suite* that skipped reaches the gate as a successful
step inside a successful job and is invisible to it. The fix belonged in the
suites.

### A niri user changed a setting and nothing happened

`apex-input-apply` told the user, **in a comment**, to hand-add an include line.
No `files/desktop/niri/` existed at all, while Hyprland's include ships as real
config. A niri user therefore changed a touchpad setting, the UI reported
success, and nothing happened. §24's niri row is
*"equal shell/settings parity"*.

niri does support `include` (top level, since 25.11; the image ships 26.04), so
nothing had to be invented: the line had never been written. A missing include
target is a hard parse error that rejects the whole config, the same fatality
Hyprland's `source =` has. That is why the write is append-only, backed up, and
validated before *and* after.

The fix corrected two things on the way. The suspected
`move-window-to-workspace "special:magic"` booby trap is harmless on both legs:
dispatch binds are never emitted to KDL, and `niri validate` accepts it anyway.
And the existing guard admitted a niri that is installed but off `PATH`, so a
bare `niri validate` would exit 127, `!` would read that as "invalid", and the
includes would never be written, silently, at every login, forever.

### The terminal-developer row named six programs; the image shipped two

fish, nushell and tmux joined the existing `dnf5` transaction rather than a new
one, because every dnf transaction rewrites the ~200 MB sqlite rpmdb into its
own layer. **This is a `core` rebuild, and the next fleet update is therefore
multi-gigabyte.** That is the documented cost of the tier rule, taken
deliberately.

`zellij` is not in Fedora 43, so it ships as a pinned static musl release with
its sha256 verified and its single tar member confirmed before extraction,
instead of the non-fatal `|| true` shape used elsewhere.

Installed does not mean usable: `chsh` refuses a shell absent from
`/etc/shells`. fish and tmux register themselves from their own rpm scriptlets;
**nushell's does not**, so the build adds `nu` explicitly, and asserts the
scriptlet-registered entries rather than trusting them.

## Two process hazards worth recording

**Parallel agents on one worktree contaminated each other's commits.** Two used
`git add -A`/`git commit -a` while a sibling had uncommitted work, so `0e7eab0`
and `4e5e974` carry files belonging to another change under their messages.
Nothing was lost and the tree content is correct, but it violates
`AGENTS.md`'s one-logical-change-per-commit rule. Neither agent rebased to split
them, which was the right call: rewriting history across another agent's
in-flight edits risked destroying work to fix attribution.

**A verb can vanish from the binary without breaking the build.** One agent's
commit, built from a copy of `main.rs` taken before another's landed, removed
`mod task;` and the `Cmd::Task` arm. Nothing failed to compile: removing the
`mod` also stops the file being compiled, so there is no orphaned reference, no
dead-code warning and no test failure. `apex task` was gone, and a person
re-reading a diff caught it.

`tests/test-apex-verbs.sh` now asks the built binary what it can do: 44 verbs
enumerated by name. It is proven against the real failure: removing
`mod task;` compiles with zero errors and turns this suite red.

## Shell branches consolidated

Five apex-shell branches existed by the end of P3. `p3/recovery-ui` already
contained `p3/ui-polish`, which already contained `p2/remote-agent-status`, so
the consolidation was three merges rather than five:

| branch | contents |
| --- | --- |
| `p2/remote-agent-status` | P2 §20 shell half, plus the geometry-scaler fix |
| `p3/ui-polish` | ⊃ above, plus 7 colour tokens and the two token checks |
| `p3/recovery-ui` | ⊃ above, plus §19's Settings surface |
| `p3/unified-search` | §15, off `main` |
| `p3/floating-parity` | labwc's three dead verbs + niri portals, off `main` |

**`p3/shell-integration`** is all five. It had one conflict, in `ci.yml`,
where the search and recovery branches each appended to the same required-files
list. The merge resolved it by union and asserted both sides' entries present
afterwards.

Integrated tree, re-run rather than assumed:

| | result |
| --- | --- |
| node suites (6) | all assertions passed |
| `check-*.sh` (9) | **521 passed, 0 failed** |
| `shellcheck -S warning` on `tests/*.sh src/scripts/*.sh` | clean |

## §24's remaining rows, and an honest line under them

Two §24 rows are **not** closable by tonight's work, and padding them would be
worse than naming them:

**Colour management does not exist.** Zero hits for icc, colord,
color-management or `wp_color` anywhere in the tree. It is the Creator row's
defining feature, and it appears nowhere in §1-§23 as a deliverable: the
roadmap never scheduled it, which is why no phase built it. Building an ICC
pipeline unattended, against no specification, is not something to do at 2am.

**GPU controls are NVIDIA-only, and clock locks exist for one machine.**
`gpu.rs` is NVIDIA-only and only `msi-katana-gf76.toml` (the developer's own
laptop) carries a `[gamemode]` section. `generic-desktop`, `generic-laptop`
and `amd-zen` have none. That is a real limit on the Gamer and Creator rows.

> **Corrected 2026-09-07 (P1-043).** The sentence that stood here said closing
> it "needs hardware nobody here has". That was wrong, and it was wrong when it
> was written: the ThinkPad L16 is a Radeon 780M (`0x1002`, amdgpu) and the MSI
> Katana is an Alder Lake-P Iris Xe (`0x8086`, i915) beside an RTX 3070 Mobile
> (`0x10de`). All three vendors were on the two machines the whole time, and
> nothing had asked either of them. `apexd-core/tests/gpu_parity.rs` is built
> from what they actually publish.

Both are recorded as gaps rather than quietly counted as met.

## A local build was vendoring a stale shell, and that is why the check mattered

apex-shell merged first, deliberately: `Containerfile.base` resolves the shell
with `git ls-remote refs/heads/main` **at build time**, so an OS build started
before the shell lands vendors a shell without the work.

After the merge, the validation build was restarted, and its log showed
`--> Using cache` on the shell clone layer. `git clone --branch main` is a cache
hit forever: podman cannot know the remote moved. As a result **every local
build after the first vendored whatever apex-shell was at that first build**,
indefinitely, and the validation about to be trusted was against the pre-merge
shell.

`build-image.yml` never had this bug: it resolves the SHA and passes it, so the
build-arg changes whenever the shell does. `build-local.sh` passed nothing and
inherited the `main` default. It now resolves the same way, which also makes a
local build reproduce what CI produces rather than something older. A failure
to reach the remote is fatal rather than a fallback to `main`, because the
point is to stop a build quietly vendoring a stale shell.

The restarted build's own first line confirms it:
`== shell == vendoring apex-shell 9141ea7f05e71bf36610319e46478c2d3b073aa0`,
the merge commit of apex-shell #16.

This is the third time tonight a green result was produced against the wrong
artifact: `diff` deciding a verdict in a container that has no `diff`, the
rootless-versus-root podman store, and now the cached shell clone. In each case
the code was correct and the *measurement* was not.

## Merge state

| repo | state |
| --- | --- |
| apex-shell | **merged**: PR #16 into `main` at `9141ea7`; #15 auto-closed, its commits contained |
| apex-os | `p3/base`, 179 commits ahead of `main`, containing P1 (`p1/integration-2`) and P2 (`p2/base`), verified with `merge-base --is-ancestor` |

apex-os final verification before merge:

| | result |
| --- | --- |
| `cargo test --locked` | **1064 passed, 0 failed** |
| shell suites (22 files) | **1517 assertions, 0 failures** |
| `cargo clippy --all-targets --locked -- -D warnings` | clean, six crates |
| core image rebuilt with the new shells | fish 4.2.0, nu 0.99.1, tmux 3.7c, zellij 0.45.1, all registered in `/etc/shells` |

Merging `p3/base` to `main` fires `build-image.yml`. That is the final build.

## Merged, and what the first real build found

**apex-shell** merged first at `9141ea7` (PR #16). That order was mandatory,
because `Containerfile.base` resolves the shell with
`git ls-remote refs/heads/main` at build time.

**apex-os** merged at `dd1fed8`, 179 commits, P1 + P2 + P3 together. The
repository disallows both merge commits and rebase, and squashing would have
collapsed 179 commit messages into one; `main` was an unprotected ancestor, so a
fast-forward preserved the history exactly. PR #37 shows MERGED.

### The first build failed, and the cause was a fallback that had never worked

Codeberg returned **HTTP 504** while cloning `awww`, the wallpaper daemon. That
step is written to tolerate exactly this: it has an else branch that records
"BUILD FAILED (wallpaper daemon absent; non-fatal)". The branch ran, and the
build died anyway.

Nothing in `Containerfile.core` creates `/out`. On the **success** path
`install -D` makes `/out/usr/bin` as a side effect, so the status write that
follows happens to work. On the **failure** path nothing has created `/out`, so
`echo … > /out/awww-status` fails with "No such file or directory" and `set -e`
kills the build.

The branch labelled *non-fatal* was therefore only ever non-fatal when the build
had succeeded. It had never once worked in the circumstance it exists for, and
a transient outage at a third-party forge could fail the entire image build at
any time.

Reproduced in a Fedora 43 container before fixing, and the fix verified the same
way:

```
$ set -eux; if false; then :; else echo x > /out/awww-status; fi
bash: line 4: /out/awww-status: No such file or directory     # before
$ set -eux; mkdir -p /out; if false; then :; else echo x > /out/awww-status; fi
awww: BUILD FAILED (wallpaper daemon absent; non-fatal)        # after
```

`yazi`'s equivalent block was checked and is non-fatal: its else branch echoes
to stdout rather than redirecting into `/out`.

This is the fifth time in this run that a green-looking path was wrong for a
reason no test could see, and the fourth where the *measurement* rather than the
code was at fault. It is also the clearest argument for the local build: the
same class of defect, found on the Katana in minutes, would have burned an hour
of runner time each attempt.

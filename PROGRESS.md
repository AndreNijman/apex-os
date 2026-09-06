# APEX-OS roadmap v2.2 — progress

Resume state for autonomous sessions. `roadmap.yaml` holds the per-task status;
this file holds the things the yaml cannot say: where the branches are, what is
running right now, and what the next session should pick up.

Updated: 2026-09-06 19:05 AWST

## How to resume

1. Read this file.
2. `git -C <repo> fetch origin && git log --oneline origin/roadmap/v2.2 -5` in both repos.
3. Read `roadmap.yaml`; anything not `done` is outstanding.
4. Continue from "Next up" below.

## Ground rules for this program

- **Integration branch, not main.** All work lands on `roadmap/v2.2` in both
  repos. `build-image.yml` fires on push to `main` for `Containerfile*`,
  `files/**`, `apexd/**`, `config/**`, `kernel/**` and `.github/**`, and
  `pr-validation.yml` fires on any PR to `main`. Both stay untouched until the
  final integration PR. Pushing `roadmap/v2.2` and task branches fires nothing.
- **Uncommitted work is snapshotted for you, every 3 minutes.**
  `~/.local/bin/apex-wip-snapshot`, on a systemd user timer with
  `Persistent=true` and `Linger=yes`, force-pushes every dirty worktree of both
  repos to `refs/wip/<worktree-name>` on origin. Recover with
  `git ls-remote origin 'refs/wip/*'` then `git fetch origin refs/wip/<name>`.
  It is safe under a live agent: it builds the commit through a throwaway
  `GIT_INDEX_FILE`, so the real index, HEAD, working tree and stash stack are
  never touched, and it skips worktrees mid-rebase. Push anyway — a snapshot is
  a net, not a substitute.
- **Katana runs the heavy work, when it is free.** Rust builds, image builds and
  test matrices go there over ssh, with scratch in `/var/tmp/apex-build/`. It is
  Andre's gaming machine too: check `pgrep -u andre -f "steam|proton"` first, and
  when he is playing, build on the L16 under `nice -n 15` instead.
- **Headless only.** No windows on Andre's desktop, no polkit or keyring
  prompts. QML verification runs under nested labwc or the repo `tests/`
  harness.
- **Worktree isolation** for every repo-writing agent.
- **No AI attribution** in any commit, PR or release.
- **`stop_slop` gates every task** that changes human-facing prose:
  `python3 ~/.claude/skills/stop-slop/scripts/slopcheck.py <files>`.
- **Honest statuses.** A task without evidence for its acceptance criteria is
  `partial` with a note saying what is missing. Never `done` on faith.

## The integration plan (work this out once, follow it at the end)

`Containerfile.base:454` has `ARG APEX_SHELL_REF=main` and clones apex-shell
with `git clone --depth 1 --branch "${APEX_SHELL_REF}"`, which accepts a branch
or a tag. So the whole integrated stack can be built and validated on katana
**before** anything reaches `main`:

```sh
podman build -f Containerfile.base --build-arg APEX_SHELL_REF=roadmap/v2.2 ...
```

That is how end-to-end validation happens without GitHub CI, which is what Andre
asked for.

**Merge order at the end is load-bearing.** `Containerfile.base` vendors
apex-shell from a remote ref, so apex-shell must land on `main` before or with
apex-os, or the OS image is built against a shell that does not have the changes
and the shell-side work is inert in the image. Order:

1. apex-shell `roadmap/v2.2` → `main`
2. apex-os `roadmap/v2.2` → `main`, which triggers `build-image.yml`

`main` requires a PR plus `PR validation` plus resolved threads, with Andre as
the only bypass. Merge commits are disabled, so rebase or squash.

## Branches

| Repo | Integration branch | Forked from |
|---|---|---|
| apex-os | `roadmap/v2.2` | `origin/main` @ 57f593a |
| apex-shell | `roadmap/v2.2` | `origin/main` @ 9141ea7 |

Worktrees now live on disk at `/var/tmp/apex-work/`, not in the session
scratchpad. The scratchpad is tmpfs: it is quota-limited, and it disappears when
the session ends, which took the integration worktree with it once already.
`git worktree prune` after that happens, then recreate.

- `/var/tmp/apex-work/int-os` → apex-os `roadmap/v2.2`
- `/var/tmp/apex-work/int-shell` → apex-shell `roadmap/v2.2`

## Machines

| Host | Booted digest | Role |
|---|---|---|
| L16 | `sha256:308127d9` | Andre's workstation. Light use only. |
| katana | `sha256:308127d9` | Build box **and** gaming machine. 20 cores, 62 GB RAM. Yield to a running game. |

Katana was pruned on 2026-09-06 (reclaimed 235.6 GB) and is booted on the
current image, so it doubles as P0-001 hardware evidence.

## Task counts at start

126 tasks: 42 P0, 62 P1, 19 P2, 3 Later.
Starting statuses: 95 todo, 19 verify_existing, 10 partial, 2 blocked.

## Dependency waves

Computed from `depends_on` in `roadmap.yaml`:

- Wave 0 — 56 tasks, no dependencies (all 18 BASE, most P0 UI, much of P1/P2).
- Wave 1 — 27 tasks.
- Wave 2 — 15 tasks.
- Wave 3 — 14 tasks (most of the Cloudflare capability sets).
- Waves 4-7 — 14 tasks (Cloudflare deploy, then the Android app chain).

## Status

### Done

- **Roadmap repair**: added the missing `P0-025` (Hyprland hyprlang → Lua migration)
  to `roadmap.yaml`. It was specified in `ROADMAP.md` section 50 and
  `CURRENT_ISSUES.md` UI-011 but absent from the canonical task file, so no agent
  reading the yaml would ever have done it. 127 tasks now; yaml parses, no
  duplicate ids, no dangling `depends_on`.

- **All 18 BASE tasks verified**, none left at `verify_existing`: 11 `done`,
  7 `partial`. Every verdict carries evidence from commands actually run on real
  hardware, and every `partial` names what is missing. Roughly 1,100 shell
  assertions and 1,080 Rust tests were run across the five agents.
- **`fix/gaming-signal-eacces`** pushed (2 commits, `ed5243c`): a defect class
  found in four places by two independent verifications.

### The EACCES sweep, finished

All ten catalogued sites addressed on `roadmap/v2.2` (**1273 tests**). Two were
already fixed by the first pass. Two turned out worse than catalogued:

- **`boot.rs`**: a whole-directory efivarfs refusal takes `LoaderInfo` down with
  `StubInfo`, so `detect_bootloader` falls back to the cmdline and answers
  `"grub"` — and the rescue-target route would then offer GRUB-menu
  instructions to someone on a systemd-boot and UKI machine. Not a wrong label,
  wrong recovery advice.
- **`apex-env`**: `create`'s overwrite guard used the same stat test as the
  reporting path, so an unreadable existing capsule record was **silently
  overwritten** rather than merely misreported.

It also corrected my own earlier commit. I had justified the fix partly on
`/sys/kernel/security` and `tpm0/` being "commonly mounted 0700"; they are 0755
on this machine, only the leaf file is `0440 root:tss`, so a bare stat would not
fail there under plain DAC. The code fixes stand — a sandboxed environment can
still refuse the stat — but the comments now say what was verified rather than
what was assumed.

One item was skipped for a good reason: `secret.rs` is deleted outright by
P0-002's branch, so fixing it would have been work thrown away.

Disclosed by the agent and checked by me: a few test runs went without
`XDG_STATE_HOME` re-exported. Andre's state is intact, the daemon is active, and
the only damaged files remain the three transcripts truncated at 10:18.

### The EACCES defect class, fixed

Four readers collapsed "permission denied" into "absent", and each then reported
the guess as a checked fact:

| Where | What the user saw |
|---|---|
| `apexd-core/src/gaming.rs` `file()` | `apex gaming` always warned the sudoers rule was missing, because `/etc/sudoers.d` is 0750 and the command runs unprivileged |
| same, `executable()` | a mode it could not read reported as a mode without the executable bit |
| `apex/src/recover.rs` `package_row` | `apex recover status` told every non-root user with packages installed that they had **none**, marked `Verified` |
| `files/system/libexec/apex-pkg` `cmd_list` | `apex pkg list` said the same |

The codebase already had the right pattern next door: `apex boot status` reads
the ESP, also 0700, and reports `entriesUnavailable` with a reason.
`Health::Unavailable` is documented as "never a synonym for fine". The tests
missed all four because their fixtures are owned by whoever runs the tests, so
nothing ever hit EACCES. New tests seal a directory to 0000, verify the seal
actually took (root walks through 0000), and assert both halves so a real
absence stays a measurement. Clippy clean, 1080 tests pass.

### Landed on `roadmap/v2.2`

**apex-os `583698b`** — 11 commits, 1158 tests, clippy clean.
**apex-shell `05d9412`** — 7 commits.

| What | Why it mattered |
|---|---|
| P0-020 | sliders no longer take the wheel; five controls audited and changed, five navigation ones kept |
| P0-021 | four of seven agent states rendered as the palette foreground |
| P0-022 | the Agents guide, with every CLI verb read out of the clap enums |
| P0-018 (half) | the confirmation dialog lived inside the window the apply destroyed |
| P0-004 | the six permission dimensions, plus `PR_SET_NO_NEW_PRIVS` on unconfined sessions |
| EACCES class | four readers that reported a refused stat as a checked absence |
| multilib | six defects that made `apex install steam` impossible |

### The multilib story, and what it left on katana

`apex install steam` had never worked. The guard asked `rpm -q <name>` with no
architecture, so on multilib it saw the x86_64 build and deleted every i686
library as "already provided"; `REFUSE_RE` refused the rest; `download_rpms`
never asked for the 32-bit closure; rpm's 32-bit content at `/lib` never
reaches a sysext, which merges `/usr` and `/opt` only; and `install` died on
fontconfig's dangling relative `/etc` symlinks.

Fixing those exposed a sixth: `extract_rpms` unpacked both architectures through
one `rpm -Uvh --replacefiles`, so the i686 `/usr/bin/fc-list` shadowed the
image's. **53 of katana's `/usr/bin` binaries are currently 32-bit**, including
`ldconfig`, `pipewire`, `dconf`, `gio`, `gsettings` and every `fc-*` tool. That
is why `fc-list` returns zero fonts and Steam draws no text.

Audio survives only because pipewire started at 09:09, before the extension
merged at 12:41. A pipewire restart, a logout or a suspend will exec the 32-bit
binary. **Do not suspend katana until it is repaired.**

Repair script ready at `scratchpad/katana-repair.sh`. It unmerges the extension,
which removes Steam, so it waits until nothing is being played.

### Recovering from the 12:50 usage limit

Eight agents were killed mid-work. The push-after-every-commit rule held for
most of them:

| Task | Outcome |
|---|---|
| P0-004, P0-018, P0-022, EACCES sweep | pushed before dying, all replayed and landed |
| P0-017, P0-002 | **lost entirely**, never pushed. Re-dispatched. |
| greeter names, disposable aliases | lost, re-dispatch pending |

The lesson is in every new agent prompt: push after each self-contained piece,
not once at the end.

### P1-044, done by the orchestrator between waves

**APEX had no firewall.** `firewalld` was not installed, `nftables` shipped with
an empty ruleset, and nothing in any Containerfile referenced either — so every
port a program bound was reachable from the network. Seventeen were listening on
the L16, including a model server on 11434 and a Steam remote-play socket.

Landed on `roadmap/v2.2` as `432b83b`: a default-drop policy, an `apex firewall`
helper that manages exceptions by service name into named nftables sets rather
than by rewriting the ruleset, a unit that loads before `network-pre.target` so
there is no unfiltered window at boot, and 25 assertions. `ssh` is open on
purpose: `apex host run` and remote agents are ssh, and a policy without it
strands the user on the machine they were driving from.

Criterion 2 (exceptions in the APEX UI) still wants a shell page. The CLI it
would drive exists.

### Integration order is now the constraint, not capacity

Four tasks have landed on `roadmap/v2.2` since the last wave and two needed a
rebase handed back to their own agent. That is working as intended — the agent
that wrote a change resolves its conflict — but it means branches should be
folded in as they report rather than batched, and a branch left unmerged for an
hour costs a rebase.

**P0-002 is the interesting one.** Criterion 2 says the normal API cannot return
a raw secret. It landed that as a type-system property rather than a policy:
`SecretValue` implements neither `Serialize` nor `Deserialize`, and `Response`
derives `Serialize`, so **a reply carrying a credential does not compile**.
Three tests keep it true as the API grows, including one that pins the response
variant list so a new variant fails a test rather than quietly opening a path.

The end-to-end proof is a real git smart-HTTP server on loopback that 401s
without auth: it asserts the server *received* the credential while no reply,
no output and no audit line contained it. That is the difference between a
brokered operation and an exit-zero no-op.

It also moved grants and the audit trail behind the root boundary. Both were
user-writable before, so a session could grant itself a capability and then
rewrite its own audit.

### The 17:00 usage limit, and why it cost nothing

Six agents were killed. Four had pushed and lost nothing. Two — P0-009 and
P0-011 — had not, and I reported their work as lost.

It was not. The snapshot timer, installed immediately afterwards, found both
worktrees still on disk and rescued **2691 insertions** from P0-009 (including
an entire untracked 2368-line `profile.rs`) and **930** from P0-011. Both are
now on `refs/wip/` and both tasks have been redispatched to build on them.

The first version of that tool would have rescued almost none of it. It used
`git stash create --include-untracked`, which snapshots tracked modifications
only: it accepts the flag, ignores it, and prints nothing at all when the only
change is a new file. Measured on the same worktrees, stash captured 323
insertions where the temp-index version captured 2691. **Test a safety net
against the case it exists for.**

### Landed while the limit was in force

A parallel OpenCode session repaired Steam on katana and left a handoff in
`CLAUDE.md`. It found two more defects downstream of the multilib work, both now
fixed on `roadmap/v2.2`:

- **The rebuild deleted 26 image-owned `/etc` files**, among them
  `/etc/fonts/fonts.conf`, `/etc/ld.so.conf`, `/etc/krb5.conf`,
  `/etc/pki/tls/openssl.cnf` and both nssdb databases. A 32-bit rpm carries the
  same `/etc` paths as its 64-bit sibling byte for byte, so a run that installed
  the i686 set recorded them as ours, the next run stopped shipping them, and
  the has-the-user-touched-it test passed *because* the file was pristine. The
  removal pass now asks the rpmdb who owns a path and fails closed.
- **A 32-bit application twin cannot share a transaction.** steam ships
  1.0.0.85 for i686 and 1.0.0.87 for x86_64; rpm refuses the older as a
  downgrade and `--oldpackage` then hits file conflicts. A non-host-arch rpm
  whose name appears at a different EVR for the host arch is now dropped as an
  application fork rather than a multilib library.

### The 19:53 usage limit, and the four agents it stopped

All four in-flight agents died at 19:53 on the same session limit. The
`task-notification` for each said `failed`, and a later reminder said each was
still running with a plausible progress string — both cannot be true, and the
tie-breaker was `stat` on their output files: last write 19:53:38–19:54:00, so
they were dead and the "still running" reminders were stale. **Check the mtime,
not the reminder.**

Nothing was lost. Every worktree was intact, and the two with uncommitted edits
still had them (`wt-p0-005`: main.rs + registry.rs; `wt-p0-018b`: six files).
All four were resumed with `SendMessage` to their original task-id, which
restores the agent's own transcript — so each picked up mid-verification rather
than re-deriving its approach. That is the cheap recovery path and it should be
the first thing tried after any limit; fresh dispatch throws the context away.

### `fix/locked-hint` was never pushed, and P0-015 needs it

P0-015's evidence said the work was "dispatched as fix/locked-hint in
apex-shell". No such branch existed on the remote — the agent died before it
pushed. The only copy was `refs/wip/wt-lockhint-`, written by the snapshot
timer, and its worktree was already gone from disk. Recovered at `87c84f3`:
`src/services/system/LockedHintService.qml` (170 lines), a `qmldir` entry and
ten lines of `Lockscreen.qml` wiring, now pushed as `fix/locked-hint`. Without
the snapshot timer this work would simply not exist. That is the first time the
net has actually caught something.

### A landed-check that is worth trusting

`git merge-base --is-ancestor` is useless here, because the integration flow
rebases task branches onto `roadmap/v2.2` — every landed branch reads as
unmerged. `git cherry origin/roadmap/v2.2 <branch>` compares patch-ids and is
close, but a commit whose conflicts were resolved during the rebase also reads
as unlanded. Both false alarms fired today: `task/p0-022-agents-help` and
`task/p0-004-policy-split` each showed a full commit unlanded, and their content
was on the tip. The check that settles it is a two-dot diff scoped to the
commit's own files:

```
git diff origin/roadmap/v2.2 origin/<branch> -- $(git show --name-only --format= origin/<branch>)
```

Empty means identical. Three-dot (`A...B`) does not answer this question — it
shows everything the branch changed since the merge base, landed or not.

### Genuinely orphaned work found by the sweep

Three apex-shell branches from 21–23 August were never merged to `main` and are
not in `roadmap/v2.2`: `fix/labwc-desktop-parity` (10 commits, a compact app
dock, VPN/caffeine indicator truthfulness, pointer input on the power menu),
`fix/popup-first-open-and-media-keys` (5, first-open popup delivery, notification
toasts, CTRL+SUPER media keys) and `fix/screenshot-off-hyprland` (2, screenshot
on labwc and niri). They predate the roadmap program and branched around PR #4,
since when P0-016..025 have rewritten most of Bar, status, popups and labwc.
They must be ported **per commit, asking whether each bug still exists on the
tip**, not rebased as a block.

### In flight — six agents

| Task | Branch | State |
|---|---|---|
| P0-003 credentials into the broker | `task/p0-003-cred-broker` | resumed 23:24, was writing vault notes |
| P0-005/006/007 native mode, break-glass, session grants | `task/p0-005-permission-modes` | resumed 23:24, owed a SIGTERM-escalation test |
| P0-018/021/025 shell-side finishing | `task/p0-018-display-finish` | resumed 23:24, owed a mutant on the "exact" arm |
| P1-030..034 fish, nushell, tmux, zellij, layouts | `task/p1-030-shell-integrations` | resumed 23:24, owed a review pass |
| P1-045/046/047 state migration, channels, trust status | `task/p1-045-updates-trust` | dispatched 23:26 |
| P1-002/003/004 Cloudflare provider, identity, Workers | `task/p1-002-cloudflare` | dispatched 23:26 |

Six is the ceiling. Four agents plus the orchestrator hit the session limit
twice today; eight does not get more work done per limit-window, because the cap
is usage rather than wall-clock — it only gets more agents killed mid-task, and
the snapshot timer saves files, not plans.

### Held back deliberately

- **P1-040 mixed-DPI shell scaling** — same files as the in-flight P0-018 shell
  agent. Dispatch after it lands.
- **P1-039/041/042 Night Light, colour management, creator input** — also touch
  AppearancePage and the display pages. Same reason.
- **P0-014/015** — depend on P0-007, in flight. P0-015 additionally needs
  `fix/locked-hint` integrated first.
- **P0-001** hardware qualification — gated on a real reboot of the L16 onto the
  integrated image, which is the last step before main.

### Autonomous resume crons

| id | schedule | role |
|---|---|---|
| `a64bb3d6` | `49 12,17,22,3,8 * * *` | primary five-hourly resume |
| `35dc07f3` | `23 1,6,11,16,21 * * *` | offset backstop, halves worst-case dead time after a limit resets |

Both are session-only and expire after 7 days. They read this file first.

### Where the whole roadmap stands

| | done | partial | todo | blocked |
|---|---|---|---|---|
| P0 (25) | 15 | 8 | 2 | — |
| P1 (62) | 1 | 7 | 54 | — |
| P2 (19) | — | — | 19 | — |
| BASE (18) | 10 | 8 | — | — |
| Later (3) | — | 1 | — | 2 |

28 of 127 done. P1 and P2 are the bulk of what remains, 73 of the 99
outstanding. The P1 blocks worth dispatching as units: the Cloudflare suite
(16 tasks, all gated on P1-001, which is now done), MCP auth and sandboxing
(P1-018/019), agent UI and notifications (P1-020..029), desktop and creator
(P1-038..043), remote agents (P1-050..052) and the Android chain (P1-053..060,
eight tasks and the largest single piece of work in the roadmap).

### Constraints that changed

- **Katana is off-limits while Andre games on it.** Everything builds on the
  L16 under `nice -n 15`. Load there is ~0.5, so it copes.
- **Never run `qs -p <path>`.** It launches a shell instance rather than
  parse-checking one. I did this by accident; it exited on its own and his
  session was untouched, but the next one might not.
- **Always set `XDG_STATE_HOME` to a temp dir before a test run.** A suite
  writing to the real one truncated three of his PTY transcripts today.
- **Build output goes on disk, not in the scratchpad.** `/tmp` is tmpfs, sized
  at 50% of RAM, so its ceiling is about 15 GB on this machine and no quota can
  raise it past RAM plus swap. A cargo `target/` is 2-3 GB and six agents
  building at once filled it. Export
  `CARGO_TARGET_DIR=/var/tmp/apex-build-cache/<task>` — `/var` is a 1.5 TB disk
  with 1.2 TB free — and delete `target/` from a worktree as soon as its agent
  reports.

  The per-user quota was raised from 12 GB to the full 15 GB on 2026-09-06.
  Diagnose with `quota -s`, never `df`: when this filled, `df` still read 63%
  while writes were already returning `EDQUOT`, and the symptom was that `echo
  hello` failed while `true` succeeded, because the tool captures output to a
  file it could no longer write.

## Log

- **2026-09-06 09:20** — Roadmap read, 126 tasks parsed, dependency waves built.
- **2026-09-06 09:35** — Katana pruned (235.6 GB reclaimed), confirmed booted on
  308127d9. It doubles as P0-001 hardware evidence.
- **2026-09-06 09:50** — `roadmap/v2.2` created and pushed in both repos.
- **2026-09-06 10:05** — `P0-025` added to `roadmap.yaml`; the file was missing a
  task its own companion document called a release blocker.
- **2026-09-06 10:10** — Wave 0 dispatched: 5 BASE verification agents, 5 P0 UI
  implementation agents.

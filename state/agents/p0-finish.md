# p0-finish — the last five partial P0 items

Items: **P0-007**, **P0-015**, **P0-001**, **P0-002**, **P0-021**.
P0-014 closed 2026-09-12 (round 12), so these are all that stand between the
roadmap and a complete P0.

Repos, branches, worktrees:

- apex-os `task/p0-finish`, from `origin/roadmap/v2.2` @ `f39fd664`,
  worktree `/var/tmp/apex-work/wt-p0-finish`. Pushed before any work.
- apex-shell: **read-only, and deliberately still read-only.** See P0-021
  below — the subject branch is with the integration agent and there is a
  competing branch; a commit from this unit would change what they merge.

Environment notes:

- `claude-memory` MCP is **502** this session, so no `memory_boot` and no
  vault write. Everything is recorded here and in `roadmap.yaml` instead.
- This session's cgroup is `session-4.scope` → `origin::classify` answers
  local-terminal, so `apex-agentd/tests/system_grants.rs` can run directly.
- `sudo -n` works. `podman 5.8.4`, 16 cores, 992 G free on `/var`.
- Side effect of this round's CI reproduction, left alone on purpose: logind
  session `c8` (root, `State=closing`) is `sudo`'s own pam session from the
  `sudo -n systemd-run` probes. It closes itself; it is NOT one of
  `in-login-session.sh`'s (that helper uses `--pty` precisely to avoid leaving
  `closing` records). Not terminated — read-only means read-only.
- **clippy is now installed locally** (0.1.98 / rustc 1.98.0, from Fedora,
  installed by the orchestrator). `cargo clippy --locked --workspace
  --all-targets -- -D warnings` runs in ~13 s. `tests/run-clippy.sh` remains
  the gate of record because it is what CI runs; its header still says
  "neither the L16 nor katana has clippy", which is now stale.

## NEXT

Round 13 finished its code work. Branch tip `392108e5`, 7 commits, all
pushed, worktree clean. Remaining, and none of it is code this unit should
write:

1. **P0-001 stays partial** and cannot close here — see the checklist below.
   The one thing that would move it: a fresh image build from a branch that
   carries BOTH `392108e5` and `f372c089`. Neither is on `roadmap/v2.2`.
2. **P0-021 stays partial.** Criterion 4's missing assertion is specified
   below, ready to write, but it belongs to whoever owns the shell branch.
3. The orchestrator has a decision to make about the two competing apex-shell
   branches (below).

## Round 13 verdict, per item

| Item | Verdict | What closes it / what is missing |
|---|---|---|
| P0-007 | **done** | criterion 3 closed by `f31f4020` + `8609cb55`. Suite: 2530/0. |
| P0-015 | **done** | criterion 1 closed by `4501f620` (6/0 through `lock_tick`), criterion 2 by `7eaa0cdb`. |
| P0-002 | **done** | criterion 1's at-rest half closed by `71c7b5af` + `47455d93`. Suite: `tests/test-secret-at-rest.sh` 19/0 as real root. |
| P0-001 | **partial** | build criterion needs two commits to land; the rest needs hardware and a consented reboot. |
| P0-021 | **partial** | criteria 1, 2, 3, 5 are closed or vacuous; criterion 4 is structurally true but **unasserted**. |

## FOUND

### The image build cannot complete from `roadmap/v2.2`, and not for the reason anyone expected

This is the round's main finding and it is fixed on the branch (`392108e5`).

CI run `34656544347` — the first image build ever attempted from
`roadmap/v2.2` — failed in the **`rust`** job, at `cargo test`, exit 101.
Six of `apex-agentd/tests/budget.rs`'s eight assertions:

    /proc/13146/cgroup places this connection in
    "/system.slice/hosted-compute-agent.service", which is neither a login
    session nor a user service, so there is no way to tell whether a human
    is at this machine

That is `origin::classify` behaving correctly: `apex-agentd` reads the
CONNECTING PEER's cgroup, and a GitHub-hosted runner sits in a system-slice
service. `pr-validation.yml` learned this on 2026-09-07 and runs its tests
through `tests/in-login-session.sh`. `build-image.yml`'s rust job never got
it — it is the same family as the three defects fixed to get this far, and
the next one in line.

**Why it is not merely six tests:** `base` declares
`needs: [changes, rust, core]`. A failing `rust` job SKIPS `base`, and
`image` needs `base`. So one missing wrapper is why the branch can build no
image at all — which is P0-001's first acceptance criterion.

The mechanism was read rather than assumed, because `always()` in a job's
`if:` can override the implicit success gate and would have made this claim
false. It does not here — `base`'s gate is explicit:

    always() && !cancelled()
    && needs.rust.result == 'success'
    && needs.core.result != 'failure'
    && !(…build_iso…)

and `image`'s is `needs.base.result == 'success'`. So a failed `rust` leaves
`base` unrun by an explicit test of its result, not merely by default.

Proved twice, in both directions:

- *Locally, by reproducing the runner's placement* with a transient
  system-slice service running as the invoking uid:
  `/system.slice/…service` unwrapped → **2 passed, 6 failed**, with CI's
  refusal text word for word; wrapped → **8 passed, 0 failed**, the helper
  having minted `session-7.scope`. From this laptop's own `session-4.scope`:
  8 passed. The tests were never wrong; their environment could not reach
  them.
- *In the target environment.* pr-validation run `34640554622`, hosted
  `ubuntu-24.04`: the ✓ **Tests** step — the one wrapped in
  `in-login-session.sh` — passes. So logind `CreateSession` does work on a
  hosted runner, and the fix is not resting on the local proxy alone. (That
  job fails later, at `§48 storage`, which is unrelated and not this unit's.)

**Not copied from pr-validation.yml**, deliberately: its `cargo build -p apex`
step and `APEX_REQUIRE_APEX_CLI=1`, which turn the hook_bridge suite's skip
into a failure. A real gap and a stronger gate, but a second change with its
own failure modes; folding it in would make one commit answer two questions.
Left for a follow-up.

### The prediction about `Containerfile.base` — untested this run, and still standing

Recorded before the answer was known: run `34656544347` would fail in `base`
at `Containerfile.base:338` of `origin/roadmap/v2.2` @ `61504ca2`,
`test -L /usr/lib/systemd/system/multi-user.target.wants/apex-secretd.service`.

**It was not reached.** `base` needs `rust`, and `rust` failed upstream of it,
so `base` never ran. The prediction is neither confirmed nor refuted by this
run; it stands for the first run that reaches `base`. What is checked rather
than assumed: `git merge-base --is-ancestor f372c089 origin/roadmap/v2.2`
answers **NO**, so the fix for that assertion exists only on `task/p0-finish`.

So the honest statement of the build criterion is:
**`roadmap/v2.2` needs BOTH `392108e5` (or `rust` will fail and skip `base`)
AND `f372c089` (or `base` will fail at line 338). Neither suffices alone.**

**Re-verified at the end of the round against the integration tip as it then
stood, `230ee02a`** (it moved from `61504ca2` while this unit was working, so
the claim was re-checked rather than left resting on a stale ref): both
commits are still absent (`merge-base --is-ancestor` answers NO for each),
`Containerfile.base:338` is still
`test -L /usr/lib/systemd/system/multi-user.target.wants/apex-secretd.service`,
and `build-image.yml`'s rust step is still a bare `cargo test --locked`.
Whether there are further never-true assertions past line 338 is **unknown** —
the local `core -> base -> apex` build that would have found them died with
the predecessor's session and was not restarted, because CI building the image
is stronger evidence than a laptop build.

### `build-image.yml` has no ref gating on push or promote

Recorded by the predecessor; **since fixed by the orchestrator** — promote is
now gated on `github.ref == 'refs/heads/main'`. Kept because it is the record
of the hazard: `workflow_dispatch` carried no branch restriction and the
promote step was gated only on `steps.push.outputs.digest != ''`, which gates
on a push having happened, not on which ref it came from. Not exercised by
this unit. No tag pushed, nothing promoted.

### Two competing apex-shell branches both claim P0-021 — an orchestrator decision

- `task/p0-018-display-finish` @ `7004c2d` — the branch this card has always
  named. 6 commits ahead of apex-shell `roadmap/v2.2`, which has moved **70**
  commits past their merge base (`a85de29`). Not merged anywhere, not
  contained by any other branch or tag.
- `task/p0-021-agent-colors` @ `68c4280` — a *different* branch that also
  addresses P0-021 by its commit subjects, including
  `9200d3f fix(agents): four of seven agent states rendered as the palette's
  foreground` and `37fa439 fix(theme): status tokens had one value, picked on
  a dark surface`. Diverges from the other at `9141ea7`; 3 commits ahead
  there, and **not** merged into `task/p0-018-display-finish` nor into
  `roadmap/v2.2`.

Nobody has reconciled these. Which one is "the" P0-021 fix is not this unit's
call, and picking one by committing to it would have pre-empted the choice.

## P0-001 — the remainder checklist, as assertions

Written as: what is closed, what is missing, and what it would take. The
existing evidence at `ROADMAP/evidence/P0-001-hardware-qualification.md`
(probed 2026-09-06) stands; this is the delta, and it corrects two rows.

| Criterion | Verdict | The assertion that is missing, and what it takes |
|---|---|---|
| Daily and Gaming images build cleanly | **SPLIT — the old PASS is about a different tree** | PASS for digest `308127d9`, built from `main`, which both machines boot. **FAIL for `roadmap/v2.2` @ `61504ca2`**: run `34656544347` died in `rust`. Closes when a branch carrying BOTH `392108e5` and `f372c089` builds green through `image`. Costs one CI run, no hardware. |
| Fresh install succeeds | UNVERIFIED | Needs a wipe and a real install on real hardware. Not attemptable — this laptop is Andre's daily machine and katana is off-limits. Only Andre can authorise it. |
| Upgrade succeeds | **PASS, re-measured today** | `bootc status` on the L16: booted `sha256:308127d9` (2026-09-05T13:36), rollback `sha256:5e206de5` (2026-09-05T03:29) — two different digests of the same `:daily` tag, so an upgrade was performed and the machine is running the newer one. |
| Rollback succeeds | **UNVERIFIED, but better-positioned than the roadmap says** | The roadmap's narrative leans on katana, whose rollback slot holds the *same* digest as the booted one, so a reboot there would prove little. **The L16's does not**: rollback digest `5e206de5` ≠ booted `308127d9`, and ostree checksum `c9230df8…` ≠ `f3f505fc…`. A rollback reboot *here* would therefore prove something. What it takes: **one consented reboot**, which this unit must not perform. Nothing else. |
| Hyprland/niri/Floating boot and workflows | PARTIAL | All four sessions still registered (`apex-gaming`, `apex-labwc`, `hyprland`, `niri`); `Hyprland`, `niri`, `labwc` all on PATH (`gamescope` correctly absent — on-demand). Per-compositor boot and a workflow pass are unrecorded. Needs a human logging into each. |
| NVIDIA/AMD/Intel baseline | PASS at driver level | Unchanged from 2026-09-06. Per-GPU workload testing is a separate question. |
| Suspend/resume | PARTIAL | L16: **39** resumes in 30 days via `systemd-suspend.service`, all finished. katana: still zero, still untested, still off-limits. |
| Wi-Fi / Bluetooth / audio / portals | PASS | Unchanged. |
| Multi-monitor | UNVERIFIED | Needs physical displays. Ties to P0-018. |
| Steam | N/A by design | Installed on demand. |
| Recovery | PARTIAL | `apex doctor` present, rollback deployment present, flow not exercised. |
| Failed units | **clean, with a caveat worth stating** | System bus: **zero** failed units. User bus: 10 failures, every one a `libpod-*` / `libpod-conmon-*` / `podman-*` transient scope left by container runs on this machine. (They were observed BEFORE `tests/run-clippy.sh` ran in this session, so they are from earlier container work, not from it — stated that way because the sequence rules out the tempting inference.) **No image unit is failing.** Deliberately not `reset-failed`'d — reporting state, not tidying it. |

Secure Boot on the L16: **enabled (deployed)**, confirmed today. Katana's
disabled state from 2026-09-06 is unchanged and unverifiable from here.

## P0-021 — criterion by criterion, with the one gap named

Re-derived this round (the probe reporting on it died with the predecessor's
session). Everything below is from `task/p0-018-display-finish` @ `7004c2d`.

1. **"Agent cards, icons, state badges, and text use APEX theme tokens rather
   than hard-coded white" — CLOSED, and the roadmap's stated remainder is
   misfiled.** Of the 212 `Qt.rgba(1,1,1,α)` sites `check-color-tokens.sh`
   ratchets, **zero** are in agent files. Checked independently, not taken on
   trust. Widening the search to every hard-coded colour form
   (`"#rrggbb"`, `"white"`, `"black"`, `Qt.rgba(`, `Qt.hsla(`) across all 15
   files of `src/services/agents/` plus `AgentService.qml`, `agentstate.js`
   and `AgentsPage.qml`: **30 hits, 28 of them `Qt.rgba(Theme.<token>.r, …,
   α)`** — a token with an alpha, which is what the criterion asks for. The
   two that are not: `StateBadge.qml:70`, derived from `toneColor`, itself
   `Theme[AgentState.token(state)]` (so token-derived via a property, not a
   violation), and `AgentHelpPanel.qml:57` `Qt.rgba(0, 0, 0, 0.45)` — a scrim
   behind a panel, not a foreground. **Zero hard-coded white on the surface
   this criterion names.** The 212 are a *shell-wide light-mode* remainder
   (Kanban 24, AudioControl 15, AppLauncher 11, settings pages 20, …) and
   belong to whatever item owns light mode — not to P0-021.
2. **"states visually distinct in both light and dark" — CLOSED.**
   `run-agent-state-render-test.sh`, 17/17 through the real Theme in a
   headless compositor, both palettes, worst light contrast 4.81:1.
3. **"If PTY/TUI output is embedded, ANSI/256/truecolor preserved" — VACUOUS
   by its own `If`, and deliberately so.** No PTY is embedded anywhere in the
   shell. No `QMLTermWidget`, no `TerminalDisplay`, no `forkpty`/`openpty`/
   `ptmx`. `AgentCenter.qml`'s own header says it: *"A SUPERVISOR AND A
   NAVIGATOR … not a replacement for the terminal … there is no output pane,
   no prompt box … because a second and worse terminal is not worth
   building."* `SessionRow`/`RemoteSessionRow` render only structured fields
   off a `SessionInfo` record; `focusTerminal(id)` execs
   `/usr/libexec/apex-agent-focus` to focus the *real* terminal. The only ANSI
   code in the tree strips escapes from a system-info command's output
   (`SystemStats.qml:97`) and has nothing to do with agents. This criterion
   should be marked N/A with that reason recorded, not left looking unmet.
4. **"Claude, OpenCode, Codex, Gemini and generic render consistently" —
   STRUCTURALLY TRUE, UNASSERTED. This is the one real gap.**
   Agent identity reaches exactly one thing: `AGENT_NAMES` in
   `src/services/agentstate.js`, a display-name string map
   (`claude: "Claude"`, …). `StateBadge.qml` derives everything from
   `sessionState` — `readonly property color toneColor:
   Theme[AgentState.token(badge.sessionState)]` — and takes **no agent
   parameter at all**. No per-agent colour, icon or accent map exists under
   `src/`. So the five render identically by construction. But the 17 render
   cases are a *state × palette* matrix, and the fixture hardcodes
   `agent: "claude"` for every one of them — so nothing anywhere would notice
   the day someone adds a per-agent tint.
   **The missing assertion, ready to write:** in
   `tests/agent-state-render-test.qml`, five `session()` fixtures differing
   only in `agent` (`claude`, `opencode`, `codex`, `gemini`, `generic`),
   asserting an identical `toneColor` and badge geometry across all five for a
   fixed state, and that the only value that differs is the label text.
   Mutation: give one agent its own tone in `StateBadge` and it must fail.
   Cost: one test case, no new harness — the headless render harness already
   exists and already drives the real components.
5. **"Contrast meets accessibility targets" — CLOSED** by the same 17/17:
   every state clears 4.5:1 on card and hovered in both palettes.

**Why this unit did not write assertion 4.** `task/p0-018-display-finish` is
with the integration agent (p0-018b's card: only `DisplayPage.qml` conflicts),
it is 70 commits behind apex-shell `roadmap/v2.2`, and a second branch
(`task/p0-021-agent-colors`) independently claims the same item. A commit from
here would change what the integration agent is merging and would silently
pick a winner between two branches. The assertion is specified above precisely
enough to be written by whoever owns that branch once the question is settled.

## DONE — round 13, 7 commits, all pushed

Baseline on this worktree at `f39fd664`: **2517 passed / 0 failed**.
After this round: **2530 passed / 0 failed**, `cargo test --locked
--no-fail-fast`, XDG_CONFIG_HOME redirected. Clippy clean both locally and
through `tests/run-clippy.sh` (the container gate, exit 0).

**Every mutation verdict on this card was re-taken after the coordinator's
`cp -p` correction, and all of them hold.** They were not at risk — every
restore here used a plain `cp` with no `-p` and no `--preserve`, so no mtime
was carried back and no run could have measured a mutant's binary against
pristine source. Verified rather than asserted, three ways: the worktree is
clean and the restored lines are what is committed (`git show HEAD:…` for
`secret.rs`, `agent.rs` and `test-secret-at-rest.sh`), so the SOURCES are
provably right; and the suite was then re-run with the five touched files
forced stale, which reported **5 crates recompiled** — so the rebuild
demonstrably happened rather than hitting a cache — for **2530 passed / 0
failed, 0 error lines**. `./tests/test-secret-at-rest.sh` re-run: **19/0**.
`cargo clippy --locked --workspace --all-targets -- -D warnings`: clean.

Inherited from the predecessor and committed by this round:

- **`aacd15ec` fix(protocol)** — the ratchet moved into `protocol.rs` pinned
  the three revision-5 guards with `== PROTOCOL_VERSION - 1`, an equality
  about a *gap*: the next bump to 7 breaks all three for a change none is
  involved in, and the only ways green are to edit constants that did not move
  or delete the assertion — the same way a ratchet stops ratcheting that the
  commit before it was written to fix. Now `<`, over all three, with the panic
  naming all three numbers.
- **`4501f620` test(lock)** — P0-015 criterion 1. Two tests through
  `lock_tick` against a real `Daemon` and registry, asserting on
  `/proc/<pid>/stat` (`T` = stopped), not on `info.paused`, which is the
  daemon agreeing with itself. Covers the default hold, the SIGCONT on unlock,
  and `--remote continue`. The child is a real `sleep` in its own process
  group, because `hold_session` signals a process GROUP and a fabricated pgid
  would signal nothing or signal the test runner.
- **`8609cb55` fix(agent)** — `--capabilities` landed greedy (`num_args = 1..`)
  beside a POSITIONAL `prompt`, so
  `apex agent run --capabilities install "fix the bug"` parsed as two verbs and
  no prompt, and the user narrowing a grant was told their prompt is not a
  capability. Nothing would have caught it: the existing tests build `RunArgs`
  by hand. New test parses the real `Cli`. Mutation-proved — restoring
  `num_args = 1..` fails with `left: Some(["install", "fix the bug"])`.
- **`47455d93` fix(secret-test)** — the at-rest suite found its daemon with
  `pgrep -f … | head -n1`, and `sudo -n setsid apex-secretd --socket X` leaves
  **sudo** resident with that exact string in its argv. So "the daemon's real
  uid is 0" was reading *sudo's* uid and passing for the one reason it exists
  to rule out. Mutation-proved: restoring the old line reports `comm is 'sudo'`
  while the uid assertion still PASSES beside it — which is the whole finding.
  The pid is now selected against a pre-spawn set, not sorted for. 19/0.
- **`41a2f86e` fix(handoff-test)** — `4147d843` named this hazard and left it.
  `apex task new` writes `config_home()/apex/tasks.toml`, and `config_home()`
  reads `XDG_CONFIG_HOME` FIRST, falling back to `$HOME/.config` only when
  unset — so setting `HOME` did not contain the fixture, and on any machine
  that exports the variable (how this repo's suite is documented to be run) the
  task record landed in the developer's real config, was never removed, and
  `apex task new` refuses a name that exists. Passed once, failed every run
  after. Proved against the polluted config that caused it, left in place:
  8/0 where it had been 6/2, twice in a row, and the out-of-root `tasks.toml`
  byte-identical afterwards.
- **`20332ca1` fix(secret)** — clippy `assertions_on_constants` on the guard
  `4147d843` added. Fixed as `const _: () = assert!(…)`, which is a
  strengthening rather than a concession: it fails the BUILD, not one test.
  Mutation-proved by tightening to `> 99` → `error[E0080]: evaluation
  panicked`, a compile error.
- **`392108e5` ci(image)** — the login-session wrapper. See FOUND above.

Earlier commits on this branch, from the predecessor (unchanged):
`f372c089` fix(build), `f31f4020` feat(grant) P0-007 criterion 3,
`7eaa0cdb` test(lock) P0-015, `71c7b5af` test(secret) P0-002,
`4147d843` fix(tests).

## What is already proven, per item (read before adding anything)

### P0-007 — session-scoped system-access grants

Proven (p0-005, `85df34f`…`d580779`): grant is session-bound and
time-limited; the agent cannot renew its own grant; the polkit prompt is
outside the agent PTY; audit attribution separates requested-and-covered from
requested-and-granted. Criterion 3 (capability-scoped) was honoured by the
*type* and not by the *CLI* — `grants.rs:235` filled `capabilities` from
`Verb::names()` for every `SystemAccess`, so every session grant covered all
eight verbs. Closed by `f31f4020` (`--capabilities`, enforced at
`GrantAuthority::covers`, PROTOCOL_VERSION 5 → 6 because a dropped key here
WIDENS the grant) and `8609cb55` (it was unusable as shipped).

### P0-015 — lock-state policy

`spawn_expiry_thread` constructs `LockWatch` and `Loginctl` and calls
`lock_tick` every tick; `lock_tick` reads `Config::load().lock` per tick and
performs `hold_session`, `resume_session` and `daemon.grants.revoke`, with
break-glass routed through the same `end_session_for_grant` the expiry path
uses. Criterion 2 closed by `7eaa0cdb`, criterion 1 by `4501f620`.

### P0-002 — apex-secretd

Two crates; store at `/var/lib/apex-secretd/users/<uid>/`; `SecretValue`
implements neither `Serialize` nor `Deserialize`, so a reply carrying a
credential does not compile; criterion 3 proved end-to-end against a real git
smart-HTTP server on loopback; peer identity by `SO_PEERCRED`. The at-rest
half of criterion 1 — argued from the unit file, never measured — closed by
`71c7b5af`, corrected by `47455d93`.

# p0-finish — the last five partial P0 items

Items: **P0-007**, **P0-015**, **P0-001**, **P0-002**, **P0-021**.
P0-014 closed 2026-09-12 (round 12), so these are all that stand between the
roadmap and a complete P0.

Repos, branches, worktrees:

- apex-os `task/p0-finish`, from `origin/roadmap/v2.2` @ `f39fd664`,
  worktree `/var/tmp/apex-work/wt-p0-finish`. Pushed before any work.
- apex-shell: read-only so far. `task/p0-018-display-finish` @ `7004c2d` is
  the P0-021 subject and it is **not** merged into apex-shell
  `roadmap/v2.2` (@ `cd324ec`).

Environment notes:

- `claude-memory` MCP is **502** this session, so no `memory_boot` and no
  vault write. Everything is recorded here and in `roadmap.yaml` instead.
- This session's cgroup is `session-4.scope` → `origin::classify` answers
  local-terminal, so `apex-agentd/tests/system_grants.rs` can run directly.
  (An agent dispatched by `apex-roadmap-resume.timer` cannot — see p0-014.)
- `sudo -n` works. `podman 5.8.4`, 16 cores, 992 G free on `/var`.

## NEXT

P0-007, P0-015 and P0-002 are done and pushed. Outstanding: the local
`core -> base -> apex` build for P0-001 (running), the P0-001 remainder
checklist, and P0-021 (a probe agent is reporting on the shell branch).

Branch tip: see `git -C /var/tmp/apex-work/wt-p0-finish log --oneline -1`.
Baseline on this worktree at `f39fd664`: **2517 passed / 0 failed**
(`cargo test --locked --no-fail-fast`, XDG_CONFIG_HOME redirected).

## What is already proven, per item (read before adding anything)

### P0-007 — session-scoped system-access grants

Proven (p0-005, commits `85df34f`…`d580779`; 1511/0 at the time): grant is
session-bound and time-limited; the agent cannot renew its own grant; the
polkit prompt is outside the agent PTY; audit attribution separates
requested-and-covered from requested-and-granted.

**Remainder, named by the predecessor and still true on `f39fd664`:**
criterion 3 (capability-scoped) is honoured by the *type* and not by the
*CLI*. `grants.rs:235` fills `capabilities` from `Verb::names()` for every
`GrantKind::SystemAccess`, so every session grant covers all eight verbs and
there is no way to ask for fewer.

### P0-015 — lock-state policy

**The roadmap's evidence is stale and the p0-014 card's remainder 3 is stale
too.** Checked on `f39fd664`, not assumed:

- `apex-agentd/src/main.rs:249` `spawn_expiry_thread` constructs `LockWatch`
  and `Loginctl` and calls `lock_tick` every tick.
- `lock_tick` (main.rs:377) reads `Config::load().lock` *per tick*, observes,
  calls `watch.step`, and performs the actions: `hold_session`,
  `resume_session`, and `daemon.grants.revoke` — with break-glass grants
  routed through the same `end_session_for_grant` the expiry path uses.
- `apex agent lock --agents/--remote/--root-grants` exists (agent.rs:211).

So criterion 2 **has a subject now**: P0-006/007 built the grants and P0-014
round 12 wired the actor. What is missing is not code, it is an assertion —
nothing anywhere exercises `lock_tick`. `LockWatch::step` is unit-tested in
`apex-agent-core/src/lock.rs`, but `step` returns a list and touches nothing;
no test asserts a real grant in a real `GrantAuthority` is closed when the
observer flips to `Locked`.

### P0-001 — qualify the integrated image on real hardware

Proven 2026-09-06, `ROADMAP/evidence/P0-001-hardware-qualification.md`: both
machines boot digest `sha256:308127d9` from different tags with the same
ostree checksum; zero failed units; three GPU vendors clean; Wi-Fi, BT,
audio, portals working.

Newly testable: **"Daily and Gaming images build cleanly"**, because
`roadmap/v2.2` could not build at all from `5cb71ea0` until `706489ec` today,
and `build-image.yml` only ever ran on `main`.

The rest needs hardware. Katana is off-limits by instruction.

### P0-002 — apex-secretd

Proven: two crates; store at `/var/lib/apex-secretd/users/<uid>/`;
`SecretValue` implements neither `Serialize` nor `Deserialize` so a reply
carrying a credential does not compile; criterion 3 proved end-to-end against
a real git smart-HTTP server on loopback; peer identity by `SO_PEERCRED`.

Remainder: the daemon has never actually run as root in testing, so the
at-rest half of criterion 1 is argued from the unit file, not measured.

### P0-021 — Agent Center / terminal colour

Proven on `task/p0-018-display-finish`: light palette reachable
(`WallpaperService` mode persisted, passed to both matugen invocations);
`run-agent-state-render-test.sh` 17/17 through the real Theme headless, worst
light contrast 4.81:1; `check-color-tokens.sh` ratchets the 212
`Qt.rgba(1,1,1,α)` sites.

Remainders to establish: (a) the branch is unmerged; (b) criteria 3 and 4
(ANSI/256/truecolor preserved; Claude/OpenCode/Codex/Gemini consistent) are
not addressed by the existing evidence at all.

## FOUND

- **`build-image.yml` has no ref gating on push or promote.** `on:` carries
  `workflow_dispatch` with no branch restriction, and the push/sign/promote
  steps are gated only on `steps.push.outputs.digest != ''` — which gates on
  a push *having happened*, not on which ref it came from. So
  `gh workflow run build-image.yml --ref <any-branch>` pushes `:core`,
  `:base` and `:apex` and then promotes `daily`, `gaming-mesa`,
  `gaming-nvidia` and `edge` to that branch's digest — i.e. ships an unlanded
  task branch to every deployed machine on the next `apex update`. Not
  exercised; the build for P0-001 is being done locally with podman instead.

## DONE

- Worktree created, `task/p0-finish` pushed, this card.

- **`f372c089` fix(build)** — the SECOND assertion in `Containerfile.base`
  that can only ever fail, one RUN block after the one `706489ec` fixed
  today. `test -L /usr/lib/systemd/system/multi-user.target.wants/
  apex-secretd.service`, and `systemctl enable` has never written a symlink
  there. Measured inside `ghcr.io/andrenijman/apex-os:core`: it creates
  `/etc/systemd/system/multi-user.target.wants/apex-secretd.service` and
  leaves `/usr/lib` untouched. The four sibling checks in the same file all
  name `/etc`. So `roadmap/v2.2` still could not build an image after
  `706489ec`.

- **`f31f4020` feat(grant)** — P0-007 criterion 3. `--capabilities` narrows
  a session grant to named verbs, enforced at `GrantAuthority::covers`.
  `capabilities_for` beside `ttl_for`; unknown verb refused not dropped;
  break-glass refused outright; PROTOCOL_VERSION 5 -> 6 with
  `SCOPED_GRANT_VERSION`, because a dropped key here WIDENS the grant;
  forwarded over §20's hop. Four mutation pairs.

- **`7eaa0cdb` test(lock)** — P0-015. Four tests through `lock_tick`
  against a real `GrantAuthority`, only the screen faked. Three mutation
  pairs. See the P0-015 section above for why the old evidence was stale.

- **`71c7b5af` test(secret)** — P0-002 criterion 1's at-rest half.
  `tests/test-secret-at-rest.sh`, 18/0, apex-secretd as REAL root via
  `sudo -n`. Mutation: same daemon without sudo -> 8/10, including "the
  credential is readable at .../store/users/1000/demo.secret".

## IN PROGRESS

- Local `podman build`, all three stages from this branch:
  `core -> base -> apex`. The published `:core` is built from `main` and
  predates `Containerfile.core` installing `xdg-terminal-exec`, so building
  `base` against it fails on a correct assertion — that was my shortcut,
  not a branch defect, and it is why core is being built too.

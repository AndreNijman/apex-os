# suite-reliability
items: three test/CI-infrastructure defects found by other units, none theirs to fix
repo: apex-os
worktree: /var/tmp/apex-work/wt-suite         branch task/suite-reliability
base: origin/roadmap/v2.2 @ f39fd664 (no rebase)

## NEXT
All three defects are FIXED, mutation-proven, committed and pushed. The only
thing outstanding is the image build dispatched at the end:

    https://github.com/AndreNijman/apex-os/actions/runs/34639685114
    gh run view 34639685114            # what it did
    gh run view 34639685114 --log-failed   # if it is red

Nobody has ever seen this branch's content build an image. If that run is red,
READ IT BEFORE FIXING ANYTHING — the failure is the most valuable thing here,
and it has been invisible for five days.

## THE FOUR COMMITS
    25f583e2 fix(agentd): a spawn that never reaches exec must fail, not hang
    29074a4a fix(agentd): build the child's environment before the fork, execve it
    2034c752 test(agentd): proxy tests wait for an answer with a deadline
    0c30a1f2 test(apex): handoff fixture pins XDG_CONFIG_HOME, and proves it did
    50453a88 ci(image): a build from a non-main ref must not release to the fleet

## COUNTS
    baseline   apex-agentd 218/0, handoff_packet 8/0   (226 total, 0 failed)
    after      apex-agentd 222/0, handoff_packet 8/0   (230 total, 0 failed)
    +4 tests, all in pty.rs. No test was deleted, skipped or ignored.

## MUTATION PAIRS (every one restored with cp, every file verified identical)
1. child takes glibc's env lock again (unsetenv+putenv restored, exec mechanism
   untouched): 5 runs of 5 FAILED, wedging on spawn 1 or 2 of 300. Fixed code:
   300/300 clean, 5 runs of 5, ~3.3 s each.
2. leak one dup of the egress server end (mem::forget on a try_clone):
   `the_proxy_answers_a_denied_destination...` goes from a PERMANENT HANG to a
   failure in 10.44 s that names the held descriptor.
3. drop `.env("XDG_CONFIG_HOME", ...)` from the CLI  -> named assertion RED.
4. drop it from the daemon fixture                   -> named assertion RED.
5. CI guard proven by EXECUTING the extracted shell against stub podman/skopeo:
   non-main writes 4 per-SHA tags and no floating name; main writes 17 tags, a
   set byte-identical to the pre-patch script's.

## FOUND — read this even if nothing else

### 1. Task item 3's premise was FALSE, and the real defect was far worse
`workflow_dispatch` was ALREADY in build-image.yml, on `main` and on
`roadmap/v2.2`, since `48fb1b26`. `gh workflow run build-image.yml --ref
roadmap/v2.2` was always available. Only the *push* trigger is main-only.

The reason nobody had built the integration branch is that doing so would have
been destructive. `Promote to every published tag` was guarded by
`steps.push.outputs.digest != ''` and nothing else, and `push-retry.sh` (core
and base jobs) always moved the floating `:core` and `:base`. So one dispatch
from any ref would have moved `:apex`, `:daily`, `:gaming-mesa`,
`:gaming-nvidia`, every `platform-*`, `edge`, `:core` and `:base` — i.e. shipped
unreviewed integration content to every deployed machine on its next `apex
update`, and left a `:core` the next main build would have built on top of.
Fixed by `PUBLISH: github.ref == 'refs/heads/main'`.

**A dispatch runs the workflow file FROM THE REF DISPATCHED.** That is why the
run above is on `task/suite-reliability` (= roadmap/v2.2 + this unit's commits)
and NOT on `roadmap/v2.2`: dispatching `roadmap/v2.2` would run its own
unguarded copy and release it. Do not dispatch `roadmap/v2.2` until this lands.

### 2. The pty deadlock is reachable in PRODUCTION, not only in tests
No non-test code in `apexd/` mutates the environment — grepped; `paths.rs` goes
out of its way to avoid `set_var`. So the *observed* trigger was test-only. But
the child also called `putenv`, which can reallocate `environ` and so take the
MALLOC lock, and the daemon forks from a process with egress, session and
registry threads all allocating. The old child was one unlucky fork away from
wedging a real agent session, with no deadline to end it. Severity was "test
flake"; it was not.

### 3. `clear_env` never did anything a caller could observe
`unsetenv(name)` then `putenv("name=v")` and a bare `putenv("name=v")` leave
identical environments — putenv overwrites. The two differ only when the block
carries the same name twice. Preserved exactly (`child_env` honours the flag),
but `disposable.rs:122` and `:382` reason about `clear_env` as if it were a
narrowing, and it is not one. Worth a look by whoever owns disposable sessions.

### 4. `session.rs:70` pre-checks the program against the DAEMON's PATH
`pty::resolve_program` reads this process's `PATH`, while the spawn now resolves
candidates against the PATH the session is being GIVEN. A session that overrides
PATH can therefore be refused for a program that would have run. Pre-existing,
untouched, and not this unit's to change.

### 5. `XDG_CONFIG_HOME` governs more than tasks.toml
`paths::config_home()` is also how `blueprint.toml`, `memory.toml` and
`games.toml` resolve. Any fixture that pins HOME but not XDG_CONFIG_HOME can
write all four into a developer's real config. The structural fix is
`Command::env_clear()` plus an allowlist for every fixture in the tree; this
unit did the assertion instead, which is narrower but catches the same class.
Checked, not assumed: Andre's `~/.config/apex` does not exist and
`XDG_CONFIG_HOME` is unset on this machine, so nothing of his was written.

### 6. clippy is NOT INSTALLED on this machine
`cargo clippy` -> "no such command"; no `cargo-clippy` or `clippy-driver`
anywhere on PATH. Any round reporting "clippy PASS" did not run it here.

## HAZARDS OBSERVED
- The memory vault MCP was down all session (502 Bad Gateway), so none of this
  is in the vault. It is all in this card and in the five commit messages.
- No agent process of mine outlived a test: the new deadline SIGKILLs and reaps
  a wedged child, which the old code could not do.

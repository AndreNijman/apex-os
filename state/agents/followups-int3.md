# followups-int3
items: (hardening)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-int3
branch: task/followups-int3
forked from: origin/roadmap/v2.2 @ b2d7905 (2026-09-07)
env: CARGO_TARGET_DIR=/var/tmp/apex-build-cache/followups-int3
     XDG_STATE_HOME=/var/tmp/apex-fi3-state XDG_CONFIG_HOME=/var/tmp/apex-fi3-config

## NEXT
Item 4 (last): document `apex mcp connect` / `list` / `run` / `confine` /
`policy` and `apex secret grant --everywhere` in `docs/agent-runtime.md`,
in the "MCP servers, and the one line of JSON that undid the store" subsection
(around line 975) and the `apex secret` block at line ~874. Say why
`cloudflare.account.read` does NOT qualify for `--everywhere`. Then run
`APEX=/var/tmp/apex-build-cache/followups-int3/debug/apex tests/check-doc-verbs.sh docs/agent-runtime.md`
(the INSTALLED apex predates these verbs; the script's own header says to point
APEX at the build). Then tests/run-clippy.sh once, then push.

## DONE
- **#1 SECURITY — 400af1c** `same_everywhere` is now a mandatory field on
  `OperationSpec` (apex-secret-core/src/operation.rs). No `Default`, no `..`
  anywhere in the tree, so a new operation does not COMPILE until its author
  states it. `ProviderSpec::validate` refuses `same_everywhere && !names_nothing()`.
  `service::may_be_granted_everywhere` is now just `op.same_everywhere` — the
  `id == "mcp.request"` allow-list is deleted.
  The load-bearing test is `providers::tests::an_operation_that_claims_to_reach_
  the_same_thing_everywhere_binds_the_same_in_two_projects`: it binds every
  claiming operation in a project with an `apex.toml` account binding AND in a
  bare one, and requires identical `Bound`. Two EMPTY dirs would not catch it.
  MUTATIONS red/green:
    M2 cloudflare.account.read declares true -> 110/3, bind test prints
       "read account example-account [0123...]" vs "list the accounts this
       credential can see". ENDPOINT ALONE WOULD NOT CATCH IT — both are
       https://api.cloudflare.com. `detail` is what does it.
    M3 gate computes names_nothing() again -> 111/2
    M4 validate coherence rule removed -> apex-secret-core 74/1
  Behaviour unchanged: mcp.request only; cloudflare.account.read still
  per-project, so `apex cf status` still needs a per-project grant.
  RENAMED, flag for the integrator: end_to_end.rs
  `a_grant_held_in_every_project_is_only_for_an_operation_that_names_nothing`
  -> `..._that_reaches_the_same_thing`. Same test the journal calls flaky.
  Also corrected: `apex secret grant --everywhere` clap help said "Only for an
  operation that names nothing", which is the rule that was wrong.
- **#2 SECURITY — 0fc6a4d** `run_curl` now runs `/usr/bin/curl -q` (const
  `broker::CURL`), `-q` FIRST so `.curlrc` is disabled before it is read.
  New live test `a_curlrc_in_the_owners_home_cannot_configure_the_brokered_request`
  in broker.rs: fake owner home with a planted `.curlrc`, loopback header
  recorder, real `perform_http`. MUTATION M1 red/green: drop `-q` -> the
  recorded headers contain `X-Curlrc` -> restored, apex-secretd bin 112/0.

- **#3 FEATURE — 3a50741** DONE, not deferred. `run_curl` now returns
  `CurlOutput { code, stdout, stderr }` with the streams APART; the merge moved
  into `merged()`, called only by `perform_http`, so the MCP path is
  byte-identical. api.rs's 25-line child is one call to `broker::run_curl`;
  its `const CURL` and `std::process::{Command, Stdio}` are gone.
  THREE DELIBERATE BEHAVIOUR CHANGES, all in the commit message: Cloudflare
  gains `NO_PROXY=*`, Cloudflare gains the 3 MiB reply cap, MCP gains
  USER/LOGNAME. `TransportError::NoCurl`'s message changed from "could not be
  started" to "could not complete the request" because it now also carries the
  cap.
  New test `a_connection_that_never_happens_is_a_status_of_zero_and_curls_own_
  message` (port 9, discard) covers the `status == 0` path, which NOTHING
  covered before.
  MUTATION M5 red/green: put the stderr merge back inside `run_curl` -> that
  test fails "stderr was dropped" (curl's "Failed to connect" lands in stdout,
  fails the status parse, body comes back empty). Restored: secretd 114/0,
  e2e 15/0, `apex --bin apex cloudflare::` 13/0.
- **test hardening — 55832ce** the bind-invariance test's `(Err, Err)` arm used
  to PASS. A provider whose `bind` fails in both dirs for a fixture reason would
  have bound nothing and been reported proven. Now panics.

## IN PROGRESS
- Item 4 (docs) only. Branch PUSHED at 3a50741. Workspace 1804 passed /
  1 failed (the environmental scheduled-job one, present at the fork point).

## FOUND
- **BASELINE IS NOT 1801/0 IN THIS ENVIRONMENT.** At the untouched fork point
  b2d7905 I measure **1800 passed / 1 failed**, 29 binaries. The one failure is
  `apex-agentd --test system_grants
  renewing_a_grant_that_does_not_exist_is_refused_without_asking_anybody`:
  "a scheduled-job request cannot renew a system-access grant".
  CAUSE, verified not guessed: `cat /proc/self/cgroup` here is
  `/user.slice/user-1000.slice/user@1000.service/app.slice/apex-roadmap-resume.service`
  — I am running *inside the autoresume systemd service*, and
  apex-agentd/src/origin.rs maps a systemd service cgroup to `scheduled-job`,
  which §7 refuses elevation from. integrate-3 measured 1801/0 from a login
  session. Same defect class as the journal's "a gate whose only caller cannot
  fail it": these tests ask the rule about the *running process*.
  CONSEQUENCE FOR THE ROADMAP: the resume timer's own environment cannot pass
  the workspace suite. Anything dispatched by `apex-roadmap-resume.service`
  will see this failure and must not read it as a regression.
- Pre-existing flake seen once: an apex-secretd bin run gave 111/1, the next
  two gave 112/0. integrate-3's card names
  `tests::a_stale_socket_from_a_dead_daemon_is_replaced` as the known one.
- `run_git` already sets `GIT_CONFIG_GLOBAL=/dev/null` for exactly the reason
  `run_curl` was missing `-q`. The precedent was in the same file.
- **A THIRD hardening drift between the two curls, not in the brief:** the
  broker's `run_curl` set `NO_PROXY=*` and `providers/cloudflare/api.rs` did
  not. So until 3a50741 a Cloudflare call would honour `http_proxy` if one were
  ever in the daemon's environment. Closed by the move.
- **The `status == 0` path in the Cloudflare provider had NO test.**
  `mod.rs:605` branches on it to say "the api could not be reached", and
  nothing exercised it. Added with the move.
- Vault notes written (the vault is UP):
  `decisions/2026-09-07-same-everywhere-declared-fact.md` and
  `errors-and-fixes/autoresume-service-cgroup-reads-as-scheduled-job.md`.

## BLOCKED ON
- nothing

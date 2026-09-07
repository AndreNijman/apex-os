# followups-int3
items: (hardening)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-int3
branch: task/followups-int3
forked from: origin/roadmap/v2.2 @ b2d7905 (2026-09-07)
env: CARGO_TARGET_DIR=/var/tmp/apex-build-cache/followups-int3
     XDG_STATE_HOME=/var/tmp/apex-fi3-state XDG_CONFIG_HOME=/var/tmp/apex-fi3-config

## NEXT
Item 3: change `broker::run_curl` (apexd/apex-secretd/src/broker.rs:536) to
return stdout and stderr APART instead of appending stderr onto the body, make
it `pub(crate)`, add USER/LOGNAME to its env for parity with api.rs; keep
`perform_http` merging them exactly as it does today so MCP behaviour is
byte-identical. THEN move `providers/cloudflare/api.rs::call` (line ~305, the
`Command::new(CURL)` block) onto it. If that turns out to be more than ~100
lines, stop, write down what is left, and leave #3 for the next owner.

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

## IN PROGRESS
- nothing. Branch PUSHED at 400af1c. Workspace 1803 passed / 1 failed
  (the 1 is the environmental scheduled-job one, present at the fork point).

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

## BLOCKED ON
- nothing

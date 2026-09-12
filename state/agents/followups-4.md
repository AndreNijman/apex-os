# followups-4
items: (no roadmap ids — CI and lint debt)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-4
branch: task/followups-4

## NEXT
Read dispatch run 34714159369 (`gh run view 34714159369 --log-failed`) for the two
new engine steps — do NOT bank exit 0, read the "measured: passed=.. failed=.. skipped=.."
line each step now prints. Meanwhile: add the §7 placement arm to `harness!` /
`Harness::start` in apexd/apex-remoted/tests/relay.rs.

## DONE
- (round 2) dropped dd28fd3d; merged origin/roadmap/v2.2 @ cd4a799e.
- `ed1429da` test(ci): test-apex-firewall.sh + test-device-image.sh wired into the
  engine job of pr-validation.yml, both lines deleted from tests/suites-not-in-ci.txt.
  Each step now parses its own summary line: firewall fails on ANY skip and on a
  pass count below 32; device-image fails on a pass count below 39. Proven both
  ways locally (7 cases, listed in the commit message). Pushed (force-with-lease,
  to drop dd28fd3d from origin). Dispatched run 34714159369.
  suites-not-in-ci.txt is now 9 lines (7 DEBT + a new upstream one for
  test-labwc-session.sh).

## IN PROGRESS
- relay.rs §7 skip arm — design settled, not yet written. See FOUND.

## FOUND
- **My own near-miss, recorded because it nearly landed:** resolving a `git stash pop`
  conflict and then running `git checkout --theirs .` throws the resolution away and
  restores the stashed side only. It silently deleted upstream's two new
  `test-apex-backup-{ssh,s3}.sh` steps from pr-validation.yml. `check-suites-run-in-ci.sh`
  is what caught it — the gate found a defect in the commit that was wiring the gate.
- CI already runs `../tests/in-login-session.sh cargo test --locked` (pr-validation.yml,
  rust job), which mints a real logind session through PAM. That is WHY the five
  relay.rs tests are green on the runner and red from a systemd user service:
  peer cgroup `user@1000.service/...` -> `classify` -> ScheduledJob ->
  `control.rs::may_pair` refuses. Reproduced locally, 5 failed / 3 passed.
- `apex-remoted` is a BINARY-ONLY crate (no lib.rs), so an integration test cannot
  import `may_pair`/`MAY_PAIR`. The reachable equivalent is
  `apex_agent_core::origin::observe_pid(pid)` + `RequestOrigin::is_local()`, which
  `control.rs`'s own `every_origin_that_may_pair_is_local_and_no_other_is` pins to
  MAY_PAIR — so using `is_local()` in the test is not a second copy of the rule.
- The skip must be defeatable or it becomes a swallow: if PAM fails on the runner,
  `in-login-session.sh` "says out loud why it could not and runs the tests exactly as
  before", which would turn 5 red into 5 green. Plan: an `APEX_REQUIRE_*` knob in the
  family of `APEX_REQUIRE_APEX_CLI`, set in the workflow's `Tests` step env.
- tests/test-device-image.sh measures 39 assertions, not the 41 the exemption line
  claimed. Line deleted, so the stale number is gone with it.

## BLOCKED ON
- nothing

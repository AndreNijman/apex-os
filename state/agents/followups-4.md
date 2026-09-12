# followups-4
items: (no roadmap ids — CI and lint debt)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-4
branch: task/followups-4

## NEXT
Read dispatch run 34714159369 (`gh run view 34714159369 --log-failed`; also
`gh run view 34714159369 --log | grep "measured: passed="`). Do NOT bank exit 0 —
read the measured counts the two new engine steps print. Then dispatch again for
commit 10bc2aa5 and start job 1's next suite (test-apex-lid.sh) + its shellcheck line.

## DONE
- (round 2) dropped dd28fd3d; merged origin/roadmap/v2.2 @ cd4a799e.
- `ed1429da` test(ci): test-apex-firewall.sh + test-device-image.sh wired into the
  engine job, both lines deleted from tests/suites-not-in-ci.txt. Each step parses
  its own summary line: firewall fails on ANY skip and on a pass count below 32;
  device-image fails below 39. Proven both ways locally, 7 cases.
  Dispatched run 34714159369.
- `10bc2aa5` test(remote): the relay.rs COULD-NOT-RUN defect (the orchestrator's
  third job). `Harness::offer` -> `Option<PairingOffer>` deciding all four
  combinations of (this process local?) x (daemon offered?); the placement is
  established independently via `apex_agent_core::origin::observe_pid` on this
  process. `APEX_REQUIRE_LOCAL_ORIGIN` turns the skip into a failure and is set on
  the workflow's `Tests` step. Same knob + placement assertion added to
  end_to_end.rs's `open_offer`, which had the identical swallow.
  Proven: no knob -> 8 passed / 5 SKIP; knob -> 5 failed; end_to_end 10 passed /
  knob -> 7 failed; MAY_PAIR mutated to include ScheduledJob -> 5 failed with
  "a caller §7 does not call local was handed a pairing code". clippy clean.
  Pushed.

## IN PROGRESS
- nothing half-written.

## FOUND
- **CI already mints a real logind session.** pr-validation.yml's rust job runs
  `../tests/in-login-session.sh cargo test --locked`, and run 34703613155's log
  says `in-login-session: logind session 3, 0::/user.slice/user-1001.slice/session-3.scope,
  uid 1001`. That is why the five relay tests are green on the runner and red from
  a systemd user service. Measured, not assumed — it is what makes
  APEX_REQUIRE_LOCAL_ORIGIN safe to set.
- **My own near-miss:** resolving a `git stash pop` conflict and then running
  `git checkout --theirs .` throws the resolution away and restores the stashed
  side only. It silently deleted upstream's two new `test-apex-backup-{ssh,s3}.sh`
  steps from pr-validation.yml. `check-suites-run-in-ci.sh` caught it — the gate
  found a defect in the commit that was wiring the gate.
- `apex-remoted` is a BINARY-ONLY crate (no lib.rs); integration tests cannot
  import `may_pair`/`MAY_PAIR`.
- tests/test-device-image.sh measures 39 assertions, not the 41 the exemption
  line claimed.
- 7 DEBT lines left in suites-not-in-ci.txt (+ 1 new upstream one for
  test-labwc-session.sh, and 3 legitimate `-live` ones).

## BLOCKED ON
- nothing

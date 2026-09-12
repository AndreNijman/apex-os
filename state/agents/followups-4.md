# followups-4
items: (no roadmap ids — CI and lint debt)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-4
branch: task/followups-4

## NEXT
Reproduce the relay.rs pairing refusal locally (`cargo test -p apex-remoted --test relay`
from this non-login cgroup), then add a §7 placement arm to `harness!`/`Harness::start`
in apexd/apex-remoted/tests/relay.rs that SKIPs with the reason when
`apex_agent_core::origin::observe_pid(self)` is not local — and ASSERTS the refusal
happened, so a non-local environment where pairing succeeds is RED, not a skip.

## DONE
- (round 2) dropped dd28fd3d (the card commit that must not be on the branch);
  merged origin/roadmap/v2.2 @ cd4a799e (fast-forward); resolved the stash-pop
  conflict in pr-validation.yml keeping BOTH upstream's new backup-ssh/backup-s3
  steps and the predecessor's firewall/device-image steps. YAML parses.

## IN PROGRESS
- `.github/workflows/pr-validation.yml`, engine job, +55 lines (uncommitted):
  nftables install step + `test-apex-firewall.sh` + `test-device-image.sh`,
  all `if: ${{ !cancelled() }}`. Written by the predecessor. Needs a dispatch
  run read before it is committed.

## FOUND
- CI already runs `../tests/in-login-session.sh cargo test --locked` (pr-validation.yml
  line ~767), which mints a real logind session through PAM. That is WHY the five
  relay.rs tests are green on the runner and red from a systemd user service: the
  connecting pid's cgroup is `user@1000.service/...` → `classify` → ScheduledJob →
  `control.rs::may_pair` refuses. The defect is real but CI is not currently red on it.
- `apex-remoted` is a BINARY-ONLY crate (no lib.rs), so an integration test cannot
  import `may_pair`/`MAY_PAIR`. The reachable equivalent is
  `apex_agent_core::origin::observe_pid(pid)` + `RequestOrigin::is_local()`, which
  `control.rs`'s own `every_origin_that_may_pair_is_local_and_no_other_is` pins to
  MAY_PAIR — so using `is_local()` in the test is not a second copy of the rule.

## BLOCKED ON
- nothing

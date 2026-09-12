# followups-4
items: (no roadmap ids — CI and lint debt)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-4
branch: task/followups-4

## NEXT
Read dispatch run 34715628612 (`gh run view 34715628612 --log | grep -E "measured:|suite coverage:|shellcheck coverage:"`).
Do NOT bank exit 0 — the run is red from THREE pre-existing failures that are
not mine (Static "Validate Containerfile layer order"; engine "Run
virtualization assertions"; rust "§26 channels"). The evidence is the
`measured:` lines. What is still unconfirmed on a runner: devices,
trust-enforcement, netns and multilib (lid is confirmed green, 60/0, run
34715048790).

Then: Job 2, the 27 shellcheck scripts — NOTE the runner has shellcheck 0.9.0
and this laptop has 0.11.0 and they disagree; check every fix against BOTH
(`podman run --rm -v <dir>:/m:ro,Z docker.io/koalaman/shellcheck:v0.9.0 -S warning -x /m/<f>`).
Then the last 3 debt suites: test-apex-firewall-ssh.sh (needs a second host —
may belong in the exempt section with that reason), test-secret-at-rest.sh
(21 sudo calls), test-labwc-session.sh (nested compositor; engine job already
installs one — headless backend only, NEVER a window on Andre's desktop).

## DONE
- (round 3) `b4f47cce` ci: the two gates were being switched off by an
  unrelated red step ABOVE them (run 34714159369: "Run virtualization
  assertions" failed, both gates reported `-`). Both now carry
  `if: ${{ !cancelled() }}`. Also restored the plugin step's comment block,
  which had had two steps inserted mid-sentence. CONFIRMED on runner
  34715048790: both gates ran and reported.
- `995c6a60` test(ci): test-apex-lid.sh into the RUST job (it drives the
  binary). CONFIRMED GREEN ON RUNNER 34715048790: `measured: passed=60
  failed=0`. Step proven with 6 fixtures x 2 exit codes; body diffed against
  the tested body. ALSO: widened the rust selector from three narrow files/
  paths to `files/` — SIX rust-job suites read something under files/ and the
  selector reached NONE of them (seventh instance of this file's oldest bug,
  latent, not introduced by one commit). Measured both ways with the
  classifier's own greps lifted out of the live file.
- `15df7504` test(ci): test-apex-devices.sh (engine, floor 55) and
  test-apex-trust-enforcement.sh (rust, floor 77). Each reads its summary back;
  both fail on a bare SKIP line — a guard that EARNED its place during the
  mutation run, where a SKIP above a green summary went green.
- `66ca2dfb` fix(pkg): apex-pkg SC2120 on `state_kind` — RUNNER-ONLY. The
  runner has shellcheck 0.9.0, this laptop 0.11.0, and only 0.9.0 flags it.
  Reproduced in a container. Fixed with a disable + reason (the parameter's
  caller is tests/test-apex-pkg.sh, 8 times). test-apex-pkg.sh still 78/0/0.
- `84af0b76` test(ci): test-device-services-netns.sh (floor 12, skipped must be
  0 — its report() exits 0 on an all-skipped run) and test-apex-multilib.sh
  (floor 3). Multilib's THIRD assertion had never once been made: `download
  --resolve` only fetches what is not installed, so a native overlap could
  never appear and it printed SKIP on every run it has ever had. Probe now
  installs `zip` then downloads it, so the decision is really taken. Both
  remaining `echo SKIP`s in that suite are now `bad`. Proven by two mutations
  against a real container, both of which ran GREEN before this commit.
- Debt: 8 DEBT lines -> 3. Suites run 61 -> 66, exempt 11 -> 6.

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

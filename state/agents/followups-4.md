# followups-4
items: (no roadmap ids — CI and lint debt)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-4
branch: task/followups-4

## NEXT
Read dispatch run 34717346972 — the ONLY thing still unconfirmed on a runner is
the last two steps: `P0-002 secrets at rest` (floor 19) and `P3 labwc` (floor
18, plus its package install). Everything else is runner-green, listed below.

    gh run view 34717346972 --log | grep -E "measured:|suite coverage:|shellcheck coverage:"

Do NOT bank the run's colour. It is RED from THREE pre-existing failures that
are not mine and were red before this round: Static "Validate Containerfile
layer order", engine "Run virtualization assertions", rust "§26 channels".
Those three are the next thing worth owning if this unit is continued.

If labwc's count comes back below 18, the likely cause is the two portal
assertions — they are guarded by `[ -d /usr/share/xdg-desktop-portal/portals ]`
and do not run at all if the directory is absent, which drops the count to 16
without a skip. The install step is meant to prevent that; if it did not,
lower the floor to what the runner measures and say why in the step.

## DONE — round 3. Both jobs are finished; what is left is reading one run.

**Job 1 — unrun suites: 8 DEBT lines -> 0.** Suites run 61 -> 69, exempt 11 -> 4,
and all four exemptions are now reasons rather than debt.
- `b4f47cce` the two gates were being switched OFF by an unrelated red step
  ABOVE them (run 34714159369: virtualization failed, both gates showed `-`).
  Both now `if: ${{ !cancelled() }}`. This is what made every measurement below
  possible. RUNNER-CONFIRMED.
- `995c6a60` test-apex-lid.sh -> rust job. **RUNNER GREEN 60/0.** Also widened
  the rust selector to `files/`: SIX rust-job suites read under files/ and it
  reached NONE of them — seventh instance of that file's oldest bug, latent
  rather than introduced.
- `15df7504` test-apex-devices.sh **RUNNER GREEN 55/0/0**; test-apex-trust-
  enforcement.sh (see 658f7aa4).
- `84af0b76` test-device-services-netns.sh **RUNNER GREEN 12/0/0**;
  test-apex-multilib.sh **RUNNER GREEN 3/0** — its third assertion had NEVER
  been made on any run ever (`download --resolve` cannot produce a native
  overlap), and both its stray `echo SKIP`s are now `bad`.
- `658f7aa4` the trust suite minted its CA with OpenSSL 3.5-only flags while
  the runner has 3.0.13, so root.pem was never written and six assertions
  failed as though verify.rs were broken. One path now (`openssl ca
  -startdate/-enddate`), proven identical under both versions.
  **RUNNER GREEN 77/0.**
- `73da7287` test-secret-at-rest.sh -> rust job (floor 19, local 19/0);
  test-apex-firewall-ssh.sh moved to the exempt section — it needs a SECOND
  machine and refuses to target its own.
- `5600b4d7` test-labwc-session.sh — gave the suite a headless render path that
  STRIPS WAYLAND_DISPLAY/DISPLAY rather than trusting WLR_BACKENDS, so it can
  run on a runner and safely on a desktop. Local 18/0/0, no window drawn.

**Job 2 — shellcheck: 27 -> 0, list emptied.** `23a862b5`, plus `66ca2dfb`
(apex-pkg SC2120, runner-only). 152 scripts discovered, 0 failing under BOTH
0.11.0 and the runner's 0.9.0. **RUNNER-CONFIRMED** ("0 known-failing, 0 newly
failing"). Included two real defects: `! grep` refusals in vendor-apex-shell
that errexit ignores, and a backwards `>>file 2>&1` capture in
test-secret-at-rest.sh. Two dead atspi.sh captures were WIRED UP, not silenced —
and `Run greeter AT-SPI assertions` is runner-green, so neither new guard fires
spuriously.

**Merged origin/roadmap/v2.2 @ 375ca0dd** and re-ran both gates after it.

- (round 3) `b4f47cce` ci: an unrelated red step ABOVE the two gates switched
  both off (run 34714159369). Both now `if: ${{ !cancelled() }}`. CONFIRMED on
  runner 34715048790.
- `995c6a60` test-apex-lid.sh -> RUST job. **RUNNER GREEN, 60/0** (34715048790,
  34715628612). Plus: rust selector widened from three narrow files/ paths to
  `files/` — SIX rust-job suites read under files/ and it reached none of them.
- `15df7504` test-apex-devices.sh (engine, floor 55) + test-apex-trust-
  enforcement.sh (rust, floor 77). devices **RUNNER GREEN 55/0/0**.
- `66ca2dfb` apex-pkg SC2120 — runner-only (shellcheck 0.9.0 vs local 0.11.0).
- `84af0b76` test-device-services-netns.sh (**RUNNER GREEN 12/0/0**) and
  test-apex-multilib.sh (**RUNNER GREEN 3/0**). multilib's third assertion had
  NEVER been made: `download --resolve` cannot produce a native overlap, so it
  SKIPped on every run ever. Probe now installs `zip` first. Both its remaining
  `echo SKIP`s are now `bad`.
- `23a862b5` all 27 shellcheck scripts fixed; the list is EMPTY. 150 discovered,
  0 failing under BOTH 0.11.0 and the runner's 0.9.0 (checked in a container).
  Included two real defects: `! grep` refusals in vendor-apex-shell that errexit
  ignores (checked against the live /usr/share/apex-shell first — both pass on
  real data), and a backwards `>>file 2>&1` capture in test-secret-at-rest.sh.
  Two dead atspi.sh captures WIRED UP rather than silenced.
- `658f7aa4` the trust suite minted its CA with OpenSSL 3.5-only flags; the
  runner has 3.0.13, so root.pem was never written and 6 assertions failed as
  if verify.rs were broken. Now ONE path (`openssl ca -startdate/-enddate`),
  proven identical under 3.0.13 in a container and 3.5.7 here. 77/0 unchanged.
- Debt: 8 DEBT lines -> 3. Suites run 61 -> 66, exempt 11 -> 6.
  shellcheck known-failing 27 -> 0.

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

## FOUND — the thing worth handing on, measured rather than guessed

**Three pre-existing red steps are switching off 47 steps below them.** None of
the three is mine; all three were red before this round. A failed step skips
every step BELOW it in the same job, and almost nothing in either job carries
`if: ${{ !cancelled() }}`, so one defect blanks most of the job.

Measured on run 34716774107 (the same shape on every dispatch this round):

  Static  "Validate Containerfile layer order"  -> reds the job
  engine  "Run virtualization assertions"       -> 35 steps below it SKIPPED
  rust    "§26 channels"                        -> 12 steps below it SKIPPED

engine, skipped:
    P2-008 the vmlab, and its verdicts wherever this runs
    Run browser capsule assertions
    P2-012 the browserlab, and its verdicts wherever this runs
    Run capsule device-profile assertions (live where the hardware is)
    Run session-picker wording assertions
    Run shared-machine (guest, kiosk) assertions
    Run account (standard vs administrator) assertions
    Run resolver assertions
    Run display-settings assertions
    Run the Hyprland Lua config assertions
    Run the live input-settings assertions
    Vendor apex-shell for the keybind model
    Run labwc keybind generator assertions
    Install a compositor and a synthetic keyboard
    Run the keybind reload assertions
    Run the safe-graphics recovery assertions
    Install bubblewrap
    Run privilege-request assertions
    Run file-injection assertions
    Run worktree-status assertions
    Run disposable-capsule assertions
    Run root-approval assertions
    Run agent-profile assertions
    Run project-layout assertions
    Install fish and nushell
    Run fish and nushell agent-integration assertions
    Install tmux and zellij
    Run terminal layout template assertions
    Run secret-broker assertions
    Run credential-migration assertions
    §13.5 backup — encrypted at rest, versioned restore, and the four verdicts
    §13.5 backup over ssh — a real sshd on loopback
    §13.5 backup to S3 — SigV4 against an independent verifier
    ShellCheck the privileged helpers
    Run plugin CLI assertions

rust, skipped:
    §19 recovery, repair and the scoped factory reset
    Every verb is in the binary
    Docs and the CLI agree, in both directions
    Containerfile assertions can actually pass
    Run trusted-device assertions
    The shipped editors are launchable
    The desktop AI apps ship with the system and bring no updater
    Run remote-compute and handoff assertions
    Run local inference assertions
    §21 task — binder assertions, refusals and the no-prompt tripwire
    P1-062 chaos — four faults injected, proven, and survived
    Post Cache Cargo

That list includes `ShellCheck the privileged helpers`, `Run plugin CLI
assertions`, the whole backup trio, the bubblewrap suites, `Containerfile
assertions can actually pass` and `Docs and the CLI agree` — checks that have
been reporting nothing, on every PR, for as long as those three have been red.
It is the same "a skipped check counts as success" shape this repository has
now recorded seven times, one level up: not a selector that names nothing, but
a job that stops half way.

Two ways to fix it, and the file already argues for both: move a known-red step
to the BOTTOM of its job (the comment above `Run plugin CLI assertions` says
exactly why), or give the steps below it `!cancelled()` (what `b4f47cce` did for
the two gates, which is the only reason any of this round's numbers exist).

## ALSO WORTH KNOWING

- **test-apex-storage.sh (40/0) and test-apex-shared-machine.sh (60/0) were
  verified LOCALLY ONLY.** I changed their `rm -rf` targets to `${VAR:?}`. Their
  steps are in the skipped list above, so no dispatch this round ran them.
- **files/scripts/vendor-apex-shell now fails a build it used to pass silently.**
  Its two `! grep` refusals are exempt from errexit and could not refuse
  anything; they are now `if … exit 1`. That script runs in **build-image.yml**,
  not pr-validation.yml, so the blast radius is the image build. Checked first
  against the tree live at /usr/share/apex-shell — both refusals pass on real
  shipped data — but that is what a PREVIOUS build vendored, not the ref the
  next one will. If an image build goes red on "paths above still point at the
  developer tree", that is this change working, not a regression.
- **The rust job now runs on any `files/` change.** Deliberate; the reasoning is
  in the classifier's comment block. It costs one more concurrent runner on
  files/-only PRs and closes a hole that hid six suites.
- **Two new steps need the network by design.** multilib pulls a Fedora image
  and hits mirrors; labwc apt-installs seven packages. A mirror outage now reads
  as RED. That is the standard being applied, not a bug — do not "fix" it back
  into a skip.
- **`10bc2aa5` on this branch is the predecessor's, not landed yet** (relay.rs
  and end_to_end.rs, the COULD-NOT-RUN defect). The rust `Tests` step ran green
  with APEX_REQUIRE_LOCAL_ORIGIN set on runs 34715628612 and 34716774107, so it
  is runner-verified and should land with the rest rather than be dropped.

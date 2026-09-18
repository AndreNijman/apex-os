# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
SIGPIPE sweep — the 4 sites in FOUND below. Idiom is `94a3a2ac`'s: materialise
the listing to a file under $WORK, then grep the FILE. Prove the fail-open
standalone at scale first (a small fixture tree fits in the 64 KB pipe buffer,
so planting the canary in situ goes red under the OLD code too and proves
nothing). Then the 0/0 skip visibility (`hypr-lua`, `input-live`).

## DONE
- **ROUND 28 — THE ZELLIJ RACE IS CLOSED, AND THE CARD'S OWN PRESCRIPTION WAS
  THE TRIGGER.** `fddc5710` + `746b1f78` + `d0be7f7b`. `zellij_build` sent the
  layout with `zellij --session NAME --layout-string KDL` — the command that
  STARTS a session, which against an existing one adds a tab and exits 0 either
  way, so `build` returned success having built nothing. Interleaved 20-round
  loop on zellij 0.45.1, all variants in ONE run so load drift cannot explain
  the spread: send immediately `0 bad/20`; send after a 1s pause `3 bad/20`;
  **send after waiting for `zellij_has` — this card's prescribed fix — `5
  bad/20`, every one rc=0**; the fix `0 bad/20`. Underneath both: for a window
  after creation the server ANSWERS `dump-layout` and ACCEPTS `action new-tab`
  (rc=0, prints a tab id) and then discards it. Not the startup-tip About pane
  (still 1 drop in 20 with it off). Not a slow arrival (30s poll, 5 of 12 never
  arrived). Cure = wait for the server + `action new-tab` over the IPC bus +
  verify + RE-SEND, then `die`. 24 consecutive builds: 24 ok, exactly one
  `apex` tab each, 5 of them needed the re-send.
- Test end: `sleep 2` and `sleep 1` gone (`build` now returns 0 only after
  seeing the tab); `zdump` retries only while the dump is EMPTY, never while
  the tab is missing; NEW assertion "the layout landed exactly once". Mutation
  1 (send aimed at a nonexistent session) → 4 reds incl. the `die`. Mutation 2
  (send the layout twice) → the new assertion is the ONLY red. Restored
  byte-identical with `cp`, sha verified. Suite: 47 passed, 0 failed.
- Gates held: `check-shellcheck-coverage.sh` 165 scripts / **0 known-failing**;
  `check-suites-run-in-ci.sh` **71 of 75**, 4 exemptions, 0 undeclared.
- Runner: dispatched **35351419809** on `746b1f78` (pushes do NOT trigger CI
  here — every run on this branch is `workflow_dispatch`). Check `mux-layouts`
  in it; it does NOT contain `d0be7f7b`.
- **ROUND 27 VERIFICATION** — run 34803818557 job 103851638407 `Package
  engine` on `479105b6`: **62 steps green, 1 red**, and every count
  byte-matches what I measured locally last round: `inject 48/0`,
  `worktrees 61/0`, `disposable 58/0`, `profile 48/0`, `secret-broker 66/0`
  (= `bd69bb2f`), `shell-agent 58/0` (= `5229bef4`), `root-approval 20/0`
  (= `479105b6`). All seven persistent engine reds are CLOSED ON THE RUNNER.
  The one red is `apex-vm: 74 passed, 60 failed` — the run predates
  `df6c4797`, which is its fix. Expected, not a regression.
- **THE labwc KEYBIND "INTERMITTENT" IS CLOSED, AND IT WAS NEVER
  INTERMITTENT.** It is red on exactly the one run that LACKS `cbb1f7b1`
  (34796578198 @ `9df69d6b`) and green on both that HAVE it (34802332142 @
  `94a3a2ac`, 34803818557 @ `479105b6`). The red log names the mechanism
  verbatim: `keybind model: apex-shell has no branch 'task/followups-5';
  using its default branch`, then 3 FAILs reading the model from
  `../apex-shell @ 9141ea7` — apex-shell `main`, where the screen-reader bind
  is `[]`, against apex-os `rc.xml` which ships `W-A-s`. My own `cbb1f7b1`
  from last round closed it. Nothing to do.
- `df6c4797` vm: `have_kvm` read the real /dev/kvm in a $PATH-faked suite.
- `479105b6` root-approval: bind the DIRECTORY, not a file that does not exist.
- `5229bef4` shell: guard read ambient PATH, assertions a scrubbed one.
- `bd69bb2f` agent: 5 suites re-enter via `tests/in-login-session.sh`.
- `94a3a2ac` chaos: `tar -tzf A | grep -q P` failed when P WAS present.
- `13e7ec84` ci: 93 `run:` steps carry `if: ${{ !cancelled() }}`.
- `69ef58cf` `cf7722d9` `2a8ada6f` `c07574b7` — see git log.

## IN PROGRESS
- The SIGPIPE sweep and the 0/0 skip visibility, per NEXT.
- Run 35351419809 on `746b1f78` in flight.

## FOUND
- **THE 1 → 8 IS NOT A REGRESSION.** `13e7ec84` (`!cancelled()`) is why the
  engine job reports EIGHT/NINE reds where it reported ONE. A red step used to
  switch off every step below it, so the count was never a count of defects —
  it was a count of how far the job got. READ THE RISE AS VISIBILITY, NOT
  DECAY. Now vindicated: same job on `479105b6` ran all 63 steps, 62 ✓ / 1 X.
- **`hypr-lua` and `input-live` report `0 passed, 0 failed, 1 skipped`** on
  every runner — they need labwc+Hyprland, which ubuntu-24.04 lacks. They skip
  loudly to stdout, but their CI steps run them BARE and go green, unlike the
  7 steps at pr-validation.yml:1116/1159/1208/2764/2862/2904/2975 which grep
  their own output for `^SKIP`. Two gates that assert nothing and say so
  nowhere a reader of the run will see.
- **SIGPIPE SWEEP: 4 real hits**, all `find TREE | grep -q` (558 raw `| grep
  -q` hits, 27 with an external producer, but only an UNBOUNDED one can
  exceed the 64 KB pipe buffer — a `sed` range over a config cannot).
  `tests/test-apex-backup-s3.sh:252`, `tests/test-apex-backup-ssh.sh:307`,
  `tests/test-apex-ai.sh:433` are **negative leak assertions that FAIL OPEN**:
  canary present -> grep exits -> find SIGPIPE -> 141 -> `ok "no object NAME
  holds the plaintext"`. Worse direction than 94a3a2ac.
  `tests/test-apex-backup-ssh.sh:280` is the 94a3a2ac direction.
- **THE GENERAL CLASS** — a gate reading state owned by the environment it
  runs in: §26 channels (/proc/mounts, runner rw vs L16 ro); the apex-shell
  parity + keybind fallback (another repo's default branch); chaos
  diagnostics (uploaded nothing for its whole life).
- **Read a result out of the LOG, not off a pipeline's exit code.**
  `cargo test … | tail -60` reports `tail`'s 0 over a truncated log.

## BLOCKED ON
- nothing

# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
**Read run 35353055680 on `0e839f6e`** — dispatched at the end of round 28 and
carrying everything below. Two things to check in it, neither yet confirmed on
a runner:
1. `Run terminal layout template assertions`. The previous run 35351419809 went
   RED there: on the runner `action new-tab --layout-string` landed NOTHING in
   four tries over twelve seconds, while here it lands 20/20 and the top-level
   `--layout-string` is the one that drops. `0e839f6e` alternates BOTH forms,
   six attempts, verifying after each. If it is still red, the runner cannot
   land this layout by any route and the next move is to capture what the
   runner's zellij server actually says (drop the `2>&1` on the send, dump
   `zellij list-sessions` and the server log) rather than guess again.
2. `Run the Hyprland Lua config assertions` / `Run the live input-settings
   assertions` must now be GREEN WITH A `::warning::` in the run summary saying
   they asserted nothing. Green and SILENT means the parse is wrong.

Then: `Run secret-broker assertions` was RED on 35351419809 with `FAIL the
session's script actually ran` (after `unable to access
https://127.0.0.1:1/demo.git/`). The card records it CLOSED at 66/0 on run
34803818557, so this is new, a flake, or arrived with the `roadmap/v2.2` merge
at the top of round 28. Nothing in round 28 goes near it.

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
- **SIGPIPE SWEEP CLOSED** — `d9ce8ea5`. `find TREE | grep -qF X` under
  `pipefail`: grep exits at its first match, find takes SIGPIPE, pipeline = 141.
  Proven in bash with the canary PRESENT: 1.36 MB listing **5 inversions in 5**;
  717-byte listing correct 5 in 5 (find fits the 64 KB pipe buffer and exits
  first); materialised correct 3 in 3. The tree's SIZE, not the code, decided
  whether a leak could be detected — and every fixture is small, so these were
  green for their whole lives without ever being able to fail. Fixed all 4 the
  94a3a2ac way. 3 were fail-OPEN negative leak assertions (s3 object names, ssh
  object names, ai stray sockets), 1 was the false-red direction (ssh
  head.json). Each mutation-tested by making the denied thing happen — socket
  bound, canary planted as a name, head.json deleted — all red; sources
  restored byte-identical, sha verified. Suites: ai 44/0, s3 27/0, ssh 32/0.
  Repo-wide sweep says these 4 were all of it; `… | head -N` in a command
  substitution under `set +e` is unaffected.
- **THE 0/0 SKIP VISIBILITY IS CLOSED** — `d00745db`. `hypr-lua` and
  `input-live` ran BARE and went green over `0 passed, 0 failed, 1 skipped`.
  Both steps now parse their suite's summary: the skip is SAID as a
  `::warning::` in the run summary, and the count is FLOORED at 22 so a suite
  quietly dropping assertions on a machine that can run it is an error. The
  step body was EXTRACTED FROM THE YAML and exercised in 8 directions — real
  suite with compositors present (rc=0, 22/0/0); real suite with Hyprland,
  hyprctl, labwc and niri removed from PATH, which is the runner's condition
  made to happen here (rc=0 + the warning); suite exits non-zero; exits 0 but
  reports a failure; 17 passed instead of 22; no summary line; 0 passed and
  0 skipped; two lines matching the summary — the last six all rc=1 with a
  distinct `::error::`.
- **THE RUNNER DISAGREES WITH THIS LAPTOP ABOUT ZELLIJ, AND THAT COST A ROUND
  TRIP.** `0e839f6e`. Run 35351419809 on `746b1f78` went red: `action new-tab
  --layout-string` landed NOTHING on ubuntu-24.04 in four tries over 12s, same
  zellij 0.45.1 from the pinned release tarball, same layout its own parser
  assertions accepted in that run. Here it is 0 bad/20 and the top-level form
  is the one that drops 5/20; on the runner the top-level form had been green
  on 3 of the 4 runs before. Not reproducible here under TERM=dumb. So both
  forms are now sent alternately, 6 attempts, verified after each. Locally
  16/16 with one apex tab each, mux-layouts 47/0.
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
- Run 35353055680 on `0e839f6e` in flight — see NEXT.

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

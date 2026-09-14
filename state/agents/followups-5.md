# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
Reproduce the zellij race locally (zellij 0.45.1 IS installed here, the same
version CI pins): `zellij_build` in `files/system/libexec/apex-mux:220` runs
`attach --create-background` then IMMEDIATELY `--layout-string`, and the test
`tests/test-mux-layouts.sh:225` reads `dump-layout` after a fixed `sleep 2`.
Fix both ends (wait for the session, poll for the tab), commit, push.

## DONE
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
- zellij race, above. Then the pipefail sweep, then the 0/0 skip visibility.
- Run 34814935040 on tip `654fa854` in flight (dispatched by the landing push,
  so it already IS the fresh run on the tip — do not dispatch another).
  Check it for `apex-vm: 138 passed, 0 failed`.

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

# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
Commit the `test-apex-channel.sh` two-root fix, add `browser` to VERBS in
`tests/test-apex-verbs.sh`, merge `origin/roadmap/v2.2` (fdf0a8e6), rebuild,
re-run both suites `--with-binary`, push, then
`gh workflow run pr-validation.yml --ref task/followups-5` and read it.

## DONE
- `13e7ec84` ci: 93 `run:` steps in static/rust/engine now carry
  `if: ${{ !cancelled() }}`. LANDED. **It worked**: run 34795907584 on
  `roadmap/v2.2` reports all 22 rust steps past `Tests` and names THREE
  distinct failures instead of stopping at one.
- `69ef58cf` fix(ci): a path inside a grep PATTERN is not a read. LANDED and
  **confirmed green**: `Static validation` is ✓ on 34795907584 (1m3s), where it
  was X on 34728030705.

## IN PROGRESS
- §26 channels: fix measured and ready to commit (see FOUND).
- `Every verb is in the binary`: reproduced locally, one line to fix.

## FOUND
- **§26 channels root cause, measured.** `apex channel`'s verdict has TWO
  readers: `failed_units()` honours `$APEX_TRUST_ROOT`, `recover::health_rows()`
  is `Sys::from_env()` and honours `$APEX_RECOVER_ROOT`. The fixture set only
  the first, so the `filesystem` row read the REAL `/proc/mounts`. A GitHub
  runner mounts `/usr` **rw** → row goes Attention → "a healthy machine is not
  held" fails. This L16 mounts `/usr` **ro** (`sysext /usr overlay ro,...`),
  which is why it was green here and red there for a day.
  Both directions with the built binary: fixture `/usr` **ro** → 57 passed /
  0 failed; fixture `/usr` **rw** → 56 / 1, "a healthy machine was held".
- **The predecessor's `why()` helper was inert.** It read `["reasons"]`, but
  that is `apex channel report --json`'s flat shape; `status --json` nests it
  at `health.reasons`. Every call returned `error:'reasons'` — a diagnostic
  that diagnosed nothing, in the failure arm that had already gone red for a
  day naming no row. Fixed, wired into the held case, and asserted: mutating
  the key back turns 59/0 into 57/2.
- **`apex browser` is in the binary and not in `test-apex-verbs.sh`'s list.**
  Reproduced locally: `FAIL every verb in apex --help is in this file's list
  unlisted: browser`. The gate is working — the list is deliberately
  enumerated, so adding a verb means adding it there.
- **Static is fully green now.** `Input page and generator agree` also went
  green on 34795907584 — that was somebody else's fix, not mine.
- **Three rust steps still red on 34795907584**: `§26 channels`,
  `Every verb is in the binary`, `Chaos diagnostics`.
- P3 labwc thread CLOSED (run 34717723637).
- **77 steps, not 45.** Static loses 20 as well as engine's 35 and rust's 22.

## BLOCKED ON
- nothing

# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
Read the 66-line `Run terminal layout template assertions` step of run
34802332142 (job 103847365659) in
`/var/tmp/apex-work/scratch-followups-5/e.log` — its one FAIL is "the layout
landed as a tab zellij can describe" (mux-layouts: 45 passed, 1 failed) — then
the 60-of-134 virtualization reds. Then dispatch a fresh run.

## DONE
- `479105b6` test(root-approval): `mount --bind` cannot create a target only
  APEX machines have. PUSHED. `/usr/libexec/apex-pkg` is `ops::PKG_ENGINE`;
  ubuntu-24.04 has no such file, so the stub bind died and took the 8
  after-the-engine assertions with it. Now the DIRECTORY is mirrored and bound,
  one code path. **Near-miss caught and recorded**: the first version `cp`'d
  the stub over a mirrored SYMLINK to the real engine — it failed here only
  because /usr is ro, and would have overwritten the shipped engine on any rw
  /usr, i.e. on every runner. Measured: absent-target before 13/8 (the runner's
  own line), after 20/0; plain here 20/0; the new `cmp -s` guard fires 13/8
  rc=1 naming itself. Runner reproduced with `./tests/in-login-session.sh
  bwrap --dev-bind / / --tmpfs /usr/libexec -- ./tests/test-root-approval.sh`.
- `5229bef4` test(shell): the guard asked the ambient PATH, the assertions a
  scrubbed one. PUSHED. All 23 nushell FAILs were `env -i
  PATH="${BIN}:/usr/bin:/bin"` missing `/usr/local/bin`, where CI installs
  `nu`; `command -v nu` two lines up said it was there. Now symlinked into
  `${WORK}/shells`, appended last, and the guards read `[ -x "${SHELLS}/nu" ]`.
  **Four directions**, the runner's split reproduced with `bwrap --ro-bind
  <non-exec file> /usr/bin/nu`: before 34/23/1 — the runner's own line exactly
  — after 58/0/0; nu absent 33/0/1 rc=0 loud skip; plain here 58/0/0.
- `bd69bb2f` test(agent): five engine suites stopped at the first session CI
  could not place. PUSHED. inject/worktrees/disposable/profile/secret-broker
  now re-enter through `tests/in-login-session.sh`, the block
  test-privilege-requests.sh already carried. **Both directions measured in the
  runner's own cgroup shape** (`sudo systemd-run --uid=1000 --unit=… --pty` →
  `0::/system.slice/<unit>.service`, uid 1000): without the wrapper
  2/1, 5/1, 3/1, 28/1, 55/1 — byte-identical to the runner's five logs — and
  with it 48/0, 61/0, 58/0, 48/0, 66/0. **188 assertions that had never run.**
- `13e7ec84` ci: 93 `run:` steps carry `if: ${{ !cancelled() }}`. LANDED.
  **It worked, and it is the round's biggest win**: run 34795907584 shows all
  22 rust steps past `Tests` and *eight* engine reds instead of one. The reds
  below were invisible before it.
- `69ef58cf` fix(ci): a path inside a grep PATTERN is not a read. LANDED and
  **confirmed**: `Static validation` is ✓ on 34795907584 (1m3s), X before.
- `cf7722d9` fix(test): the channel fixture read the real `/proc/mounts`.
  PUSHED. Local: 59 passed / 0 failed `--with-binary`.
- `2a8ada6f` test(verbs): `apex browser` added to the enumerated list.
  PUSHED. Local: 65 passed / 0 failed.
- `c07574b7` ci(chaos): pack the bundles to a tar before upload. PUSHED.
- `9df69d6b` merge of `origin/roadmap/v2.2` (fdf0a8e6). Rebuilt and re-ran
  after it: verbs 65/0, channel 59/0 binary + 22/0 structural. Both files
  clean under `shellcheck -S warning`.

## IN PROGRESS
- run 34802332142 (round 26, branch tip `94a3a2ac`). **Static ✓ all 28 steps.
  Rust ✓ all 41 steps** — `Pack the chaos bundles` ✓ and `Chaos diagnostics` ✓,
  the first time either has been green. Engine still running; nine reds so far.
- Working the engine reds down, in order. Family A (5 steps) DONE, see above.
  Family A (5 steps), nushell PATH (1) and root-approval (1) DONE — 7 of the
  9 engine reds. Left: virtualization (60 fails), mux-layouts zellij (1).

## FOUND
- **THE 1 → 8 IS NOT A REGRESSION I CAUSED.** `13e7ec84` (`!cancelled()`) is
  why the engine job now reports EIGHT red steps where it used to report one,
  and rust three where it used to report one. Those failures were always
  there; the job stopped before reaching them. A red step used to switch off
  every step below it, so the count was never a count of defects — it was a
  count of *how far the job got*. Read the rise as visibility, not decay.
- **THE GENERAL CLASS, three instances of it this round.** A gate that reads
  state belonging to the environment it happens to run in is green wherever
  that environment agrees with the author's and red wherever it does not, and
  neither colour means what it says:
  1. **§26 channels** — the fixture leaked to the real `/proc/mounts`. A
     GitHub runner mounts `/usr` **rw**; this L16 mounts it **ro**. Green
     here, red there, indefinitely, and no amount of local re-running finds it.
  2. **Input page parity** — the comparison target is chosen by BRANCH NAME,
     and the fallback is the other repository's DEFAULT branch. apex-shell has
     no `task/*` twin for most branches, so the gate compares an integration
     branch against `main` and reports drift that is not there. Here the
     "second environment" is another repo's default branch. Measured: vs
     apex-shell `main` → rc=1, two touchpad defaults; vs apex-shell
     `roadmap/v2.2` → rc=0, "22 settings, identical keys and defaults".
     The step's OWN comment complains about exactly this misfire and then
     leaves the fallback doing it.
  3. **Chaos diagnostics** — the upload asserted nothing about what it
     uploaded, so it uploaded nothing for its whole life.
- **§26 channels root cause, measured.** The verdict has TWO readers:
  `failed_units()` honours `$APEX_TRUST_ROOT`; `recover::health_rows()` is
  `Sys::from_env()` and honours `$APEX_RECOVER_ROOT`. The fixture set only the
  first, so the `filesystem` row read the REAL `/proc/mounts`. A GitHub runner
  mounts `/usr` **rw** → Attention → "a healthy machine is not held" fails.
  This L16 mounts `/usr` **ro** (`sysext /usr overlay ro,...`), so the defect
  **cannot exist here** — it was green locally and red on the runner for a day.
  Both directions: fixture ro → 57/0; fixture rw → 56/1 "a healthy machine was
  held". The runner's own log is byte-identical: `56 passed, 1 failed`.
- **The predecessor's `why()` helper was inert.** It read `["reasons"]`, the
  flat shape of `apex channel report --json`; `status --json` nests it at
  `health.reasons`. Every call returned `error:'reasons'` — a diagnostic that
  diagnosed nothing, in the arm that had just spent a day red naming no row.
  Fixed, wired in, and asserted: mutating the key back turns 59/0 into 57/2.
- **`Chaos diagnostics` has never uploaded a byte.** `upload-artifact@v4`
  refuses any path with a colon (NTFS), and `tests/chaos/lib.sh:499` writes
  `sys/bus/pci/devices/0000:03:00.0/class` into the fixture the bundle
  snapshots. The chaos RUN passed 4/4; the upload then failed the whole rust
  job. Its own preamble says "a diagnostic that only exists inside a finished
  runner is not one" — that was its own condition. Now tarred, and the pack
  step refuses an empty archive or one missing the colon fixture. Four
  directions measured; a real local bundle packs 194 paths / 16K and the colon
  path extracts back out byte-for-byte.
- **`apex browser` was in the binary and not in `test-apex-verbs.sh`'s list.**
  The gate working as designed — the list is deliberately enumerated.
- **Static is fully green.** `Input page and generator agree` also went green
  on 34795907584 — somebody else's fix, not mine.
- **Engine has EIGHT reds** on 34795907584 (job 103828863057), newly visible:
  virtualization, file-injection, worktree-status, disposable-capsule,
  root-approval, agent-profile, fish/nushell agent-integration, secret-broker.
  Six are agent-related and may share one cause. Being trawled.
- P3 labwc CLOSED (run 34717723637). **77 steps gated, not 45.**

## BLOCKED ON
- nothing

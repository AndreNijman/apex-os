# Findings — round 5 (orchestrator, 2026-09-07)

Three defects found by running the tip rather than a branch. **None was caused
by this round's landing** — each was checked, not assumed — and each needs an
owner.

## 1. `apex ai status --json` returns non-JSON on any machine with `nvidia-smi`
   installed but no loaded driver — OWNER: P1-043 (agent `p1-039`)

`tests/test-apex-ai.sh` on `roadmap/v2.2` @ `1ab542c`: **43 passed / 1 failed**.

```
FAIL  status --json is valid JSON
      apexd: nvidia-smi query failed (exit status: 9):
```

`apexd/apexd-core/src/gpu.rs`'s own header says the no-NVIDIA case is detected
by `nvidia-smi` **not being installed**. On the L16 it *is* installed —
`/usr/bin/nvidia-smi` exists — and exits **9** (driver not loaded), because the
GPU is a Radeon 780M. So the reader takes a *failure* for a *presence* and
propagates the error, and `--json` emits a prose error where the suite
(correctly) demands parseable JSON.

This is the **"permission denied is not absence"** class again — already found
in ~14 places in this codebase — wearing "driver not loaded" instead of
"permission denied". The suite's next case is even written against the
absence-only model: it treats `command -v nvidia-smi` succeeding as licence to
list `cuda`, so on this machine APEX may claim a CUDA backend it cannot
demonstrate.

**Not caused by the landing:** `git diff 4ab5f75..1ab542c` touches no `ai` or
`gpu` file and contains **zero** occurrences of `nvidia`.

Fix belongs with P1-043 GPU parity, which already owns "AMD and Intel had no GPU
handling at all". Three states, not two: no binary / binary present but no
usable driver / working. The middle one must read as "no NVIDIA GPU here" for
planning, and `--json` must stay JSON whatever the answer.

## 2. `apex remote` is documented nowhere — OWNER: next integrate or followups round

`p1-050`'s landing added the verb family `apex remote pair | devices | revoke |
status | enable`. `grep -rn 'apex remote' docs/` on the tip returns **nothing**.

`tests/check-doc-verbs.sh` passes (43 valid / 0 invalid) and structurally
**cannot** catch this: it validates documented verb → real command, not real
command → documented. So the gate that exists for exactly this is blind in the
direction that matters here.

`followups-int3` was deliberately told **not** to document this — it is working
from a base that predates the landing, and documenting another unit's work from
a stale tree is how contradictory docs get written. This needs its own follow-up
against the tip, and it should say plainly that `apex-remoted` is not enabled by
default and that its port is offerable but not opened.

Consider also whether `check-doc-verbs.sh` should gain the reverse direction —
every verb in the CLI's help tree appears in some doc, with an explicit
opt-out list — since that is the check that would have caught this.

## 3. The §7 origin gate makes the whole approval path unverifiable under a test
   runner — 9 failing assertions across two harnesses, ONE cause

`tests/test-privilege-requests.sh` on the tip: **30 passed / 8 failed.**

```
FAIL  an unsessioned peer may approve
FAIL  a decided request cannot be re-decided
FAIL  a request can be denied
FAIL  a denied request cannot be flipped to approved
FAIL  every line is one JSON object with argv and event
FAIL  the decision is recorded
FAIL  an approval can be scoped to the project
FAIL  the grant is recorded against the project and the exact package
      apex request: a scheduled-job request cannot approve a root operation;
      §7 reserves that for a human at this machine, whichever origin filed it.
```

**PRE-EXISTING, proven rather than argued.** A scratch worktree at the
pre-landing tip `4ab5f75` gives **30 passed / 8 failed with the identical eight
names in the identical order** (`/var/tmp/apex-int5-logs/priv-fails-base.txt`).
The scratch worktree and its build cache were removed afterwards.

The cause is the same one that produces the single `cargo test` failure
(`renewing_a_grant_that_does_not_exist_is_refused_without_asking_anybody`),
which has been carried as "the cgroup/origin artifact" since integrate-4: a test
runner is started by systemd, its cgroup reads as `scheduled-job`, and §7's gate
refuses a root approval from that origin **before** the behaviour under test is
reached. So one environmental fact silently disables **nine** assertions across
two harnesses — and every one of them covers the approval path, which is the
most security-sensitive surface in the daemon.

Read the failure list again with that in mind: "a denied request cannot be
flipped to approved" and "a decided request cannot be re-decided" are *exactly*
the invariants nobody wants unverified. They are not failing because they are
broken; they are failing because they were never exercised.

Calling this an "artifact" and moving on has now happened in three integration
rounds. It is not an artifact, it is a hole in the harness.

**Likely already in hand:** agent `p0-014` committed `3c3abd3`,
"test(request): the root approval path needed a real uid 0, not …", so it has
started on this class. Confirm against `state/agents/p0-014.md` before assigning
it elsewhere. The fix is a harness that can present a real local origin — not
weakening the gate, which is doing its job correctly.

## Everything else on the tip is green

Full CI suite list — **27 of 30 green, 2 red (both pre-existing, above), 1
skipped** because its name ends `-live` and no window ever opens on Andre's
desktop: `ROADMAP/state/ci-suites-on-tip-1ab542c.txt`. Highlights —
apex-env 265/0, apex-blueprint 140/0, apex-plugin 117/0, apex-gaming 114/0,
apex-input 109/0, apex-resolve 88/0, apex-recover 79/0, apex-pkg 78/0,
apex-task 71/0, apex-modes 67/0, boot-v2 60/0, firstrun 59/0, dispatch 55/0,
host 52/0, agent-profile 48/0, verbs 44/0, device-image 39/0, display 29/0,
channel 22/0, hypr-lua 22/0, schema 15/0, trust 6/0. Plus `cargo test
--locked --workspace` 1964/1 and `run-clippy.sh` rc=0.

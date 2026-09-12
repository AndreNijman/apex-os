# followups — three defects that have outlived three rounds

Repo: both. Branch `task/followups-r18` in each repo you touch, from
`origin/roadmap/v2.2`. Worktree `/var/tmp/apex-work/wt-followups`.

No roadmap ids. Record what you close in this card, not with `set-status.py`.

## 1. `tests/test-apex-task.sh` — 70 passed, 1 failed, and nobody owns it

It arrived with a failing case and has been carried, failing, since. Find out
whether the test is wrong or the product is wrong. Both answers are acceptable;
"deleted the case" is not.

## 2. `apex remote` is documented nowhere

`apex remote pair|devices|revoke|status|enable` appears in no document.
`tests/check-doc-verbs.sh` **structurally cannot catch this**: it validates
documented-verb to real-command, never the reverse, so every undocumented verb
is invisible to it. Two jobs:
  - document the verbs where the other `apex` verbs are documented;
  - make `check-doc-verbs.sh` walk the reverse direction too, and prove it by
    removing one documented verb from the docs and watching a NAMED case fail.

The exact surface, measured by the p1-052 agent (do not re-derive):
`apex remote pair --text` prints ONLY the `apex-remote:` payload on stdout, the
prose goes to stderr; `apex remote devices --json` returns `Device` objects with
`id`, `name`, `public_key`, `paired_ms`, `last_seen_ms`, `revoked_ms`,
`requires_user_verification`, `last_path`; `state()` is `revoked` or `paired`
derived from `revoked_ms` alone; there is **no** `pair --json`.

## 3. A flaky test, named and unowned

`apex-agentd`'s `grants::…another_boot…` failed once on a full run and passes
otherwise. Its own module comment documents a `set_var` race. Either make it
deterministic or prove it is the product. A test that fails one run in fifty is
a test nobody will believe when it matters.

## NEXT

**ALL THREE ARE CLOSED.** Round 18, apex-os branch `task/followups-r18`, from
`origin/roadmap/v2.2` @ `cafd3635`, worktree `/var/tmp/apex-work/wt-followups`.
Three commits, all pushed, ready for the orchestrator to merge. apex-shell was
never needed — everything was in apex-os.

  `5a1a5390`  test(task): the version refusal was right and its shell
              assertion was three days stale
  `060058d8`  test(grants): the daemon's readiness is an answered request,
              not a connected socket
  `f9dff3b7`  docs(remote): APEX Remote was undocumented, and the doc checker
              could not have found it

Nothing is left open on this card. If a fresh agent lands here, the only
useful follow-ons are the two named under "Left deliberately undone" below.

## 1 — `tests/test-apex-task.sh`: the TEST was wrong. 71 passed, 0 failed.

The product was never wrong. The case grepped the refusal for `understands up
to`, which is what `TaskError::UnsupportedVersion` said when the file was
written (`874de9aa`, 2026-09-04). P1-045 (`5c1a9795`, 2026-09-07) rewrote the
message on purpose — §25 makes a version refusal name the remedy, because a
task list from a newer APEX is what `bootc rollback` leaves behind — updated
the Rust unit test beside the message, and did not update the shell one.

Now asserted in the same four parts the Rust test asserts (the file's version,
`reads version [0-9]+`, `rollback`, `Boot the newer deployment`), so they go
stale together. The version number is a regex, not a literal: pinning it is how
this went stale. The two labels for the case also disagreed with each other.

MUTATION: dropped the remedy sentence from the `Display` impl, rebuilt, the
named case went red with `missing: [rollback] [Boot the newer deployment]`.
Restored byte-identical with plain `cp`; `git diff` empty.

## 3 — the flake: a barrier, not a poll. And the test beside it asserted nothing.

`apex-agentd` starts `bind` → `Daemon::new` → `sweep_previous_lives` →
`listener.incoming()`. The socket is connectable from `bind` onwards, so the
harness's `wait_for_socket` returned while the sweep that writes
`privilege-audit.jsonl` had not run, and `audit_lines` turns a missing file
into an empty vector — so losing the race read as "the daemon recorded
nothing". That is the flake p2-010 measured at 1 in 20.

`98a7c248` (earlier the same day) had already papered over the POSITIVE half
with `audit_lines_once_written`, a poll for a line count, which is why it no
longer reproduced. A poll cannot help the NEGATIVE half, and
`the_ending_is_recorded_once_however_many_daemons_see_it` is the negative half:
`audit_lines_once_written(first.len())` was already satisfied, returned
instantly, and read the trail while daemon two had only reached `bind`. It was
passing by being early.

The fix is the barrier that already exists: the accept loop runs strictly after
the sweep, so an ANSWERED request proves the sweep is done.
`wait_until_answering` does one `{"cmd":"requests"}` round-trip;
`Harness::start_with` does not return until it comes back, the second daemon
gets the same treatment, and the audit reads are direct again (a poll after a
real barrier only hides an ordering regression). `main.rs` now states the
ordering both halves depend on — comment only, no behaviour change; sweeping
before `bind` would let a daemon about to fail to start close a RUNNING
daemon's grants.

COUNTS, all on this laptop:
  tip as found (with 98a7c248's poll), 50 runs idle        0 failures
  fixed suite, 50 runs idle                                0 failures
  fixed suite, 50 runs under 16 spinners on 16 cores       0 failures
MUTATIONS (deterministic, not waited for):
  300ms sleep wedged between bind and the sweep, nothing else —
    pre-98a7c248 test  20/20 FAILED   (the flake, executed)
    fixed suite        20/20 PASSED   (the barrier orders the read)
  sweep re-audits an already-closed grant, so the ending IS written twice —
    the_ending_is_recorded_once…  before  30/30 PASSED
    the_ending_is_recorded_once…  after   30/30 FAILED
Both sources restored byte-identical with plain `cp`. Clippy clean.
`cargo test -p apex-agentd`: 231 passed, 0 failed.

## 2 — `apex remote` documented, and the checker walks both ways now

`docs/remote.md` is new and covers all five verbs from the source: `pair
--text` puts only the `apex-remote:` payload on stdout with the prose on
stderr; no `pair --json`; no QR code in this build and why; the `Device` JSON
fields; `state()` from `revoked_ms` alone; `requires_user_verification` as the
device's claim rather than a verified fact.

`tests/check-doc-verbs.sh` gained the reverse pass: every `apex <verb>` and
`apex <verb> <sub>` the built binary offers must be named by a doc, or declared
in the new `tests/doc-verbs-undocumented`. Both passes share one extraction and
the canonical doc set now lives in the script (a reverse pass over a subset
calls everything undocumented). The debt file is a ratchet, not an allow list:
an entry that HAS since been documented fails as stale, so it can only shrink.

MEASURED 2026-09-12: 259 commands, 127 named somewhere, 132 not. Remote was
five of them. The list is dated and counted in the file's own header.

Two things fell out of pointing it at every doc:
  - the default binary is now this tree's `apexd/target/debug/apex`. The
    installed one gave **40 false BADs** on `roadmap/v2.2`, and on a CI runner
    there is no `apex` at all, so the check SKIPped and proved nothing.
    `APEX=apex` still asks the released-doc question.
  - `docs/m4-install-runbook.md` told the reader to run `apex doctor suspend`.
    No such verb has ever existed. Now `systemctl suspend`, then
    `apex qualify record sleep --pass`.

And it RUNS now: it was in no workflow and no suite. It is a step in
`pr-validation.yml` beside "Every verb is in the binary", which already builds
the binary it needs.

MUTATIONS, each restored byte-identical with plain `cp`:
  removed `apex remote pair` from docs/remote.md
    → `BAD   apex remote pair   is in the binary and in no document`, exit 1
  added `apex remote pair` to the debt file while it IS documented
    → `STALE apex remote pair   documented now; drop it from …`, exit 1
  added `apex remote unpair` to docs/remote.md (forward pass still works)
    → `BAD   apex remote unpair                       remote.md`, exit 1
Green on the tip: 157 valid, 3 deliberate, 0 not a command; 127 documented,
132 declared undocumented, 0 undeclared, 0 stale. `shellcheck -S warning` clean.

## Left deliberately undone, for whoever wants them

- **132 undocumented commands**, listed with their date and count in
  `tests/doc-verbs-undocumented`. The worst clusters are `apex shell` (14 of
  16 subverbs), `apex env` (11 of 13), `apex host` (8 of 8) and `apex ai` (7 of
  7). Each is now a line somebody can delete by writing the doc; the ratchet
  checks they did both.
- **`apexd/apexd-core/src/host.rs:185`** still refuses a future `hosts.toml`
  with "understands up to {SCHEMA_VERSION}" and no remedy — the §25 rule
  `tasks.toml` got in `5c1a9795`, on the store next door. NOT touched here: it
  is nobody's defect on this card and its own shell test
  (`tests/test-apex-host.sh:487`) is green against the old wording, so fixing
  the message means fixing that assertion in the same commit.

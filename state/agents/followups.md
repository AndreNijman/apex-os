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

Round 18 agent live in `/var/tmp/apex-work/wt-followups` (branch
`task/followups-r18`, from `origin/roadmap/v2.2` @ `cafd3635`).

**ITEM 3 IS CLOSED — apex-os `060058d8`, pushed.** The harness waited on the
socket connecting; apex-agentd binds BEFORE `sweep_previous_lives` and only
answers AFTER it, so readiness is now one `{"cmd":"requests"}` round-trip
(`wait_until_answering`), and the audit reads are direct again. The bigger find:
`the_ending_is_recorded_once_however_many_daemons_see_it` was asserting
nothing — its `audit_lines_once_written(first.len())` returned instantly and it
read the trail while daemon two had only reached `bind`. Counts: tip-as-found
0/50 idle; fixed 0/50 idle and 0/50 under 16 spinners on 16 cores; with a 300ms
sleep wedged between bind and the sweep the PRE-98a7c248 test failed 20/20
(the flake, executed deterministically) while the fixed suite passed 20/20; and
against a daemon mutated to write the ending twice the once-only test went
30/30 PASS before this commit and 30/30 FAIL after. Clippy clean.

**ITEM 1 IS CLOSED — apex-os `5a1a5390`, pushed.** `tests/test-apex-task.sh`
is 71 passed, 0 failed. Mutation proved: dropped the remedy sentence from
`TaskError::UnsupportedVersion`, rebuilt, the named case went red with
`missing: [rollback] [Boot the newer deployment]`; restored byte-identical.

What is MEASURED, so a fresh agent need not re-derive it:

**Item 1 — the TEST was wrong, not the product.** `tests/test-apex-task.sh:562`
greps for `understands up to`. Commit `5c1a9795` (P1-045, 2026-09-07) rewrote
`TaskError::UnsupportedVersion`'s message to the §25 form ("tasks.toml is
version 99, and this build of APEX reads version 1 … Boot the newer deployment
again to use it"), updated the Rust unit test
(`task.rs::a_future_version_is_refused_rather_than_guessed_at`) and did NOT
update the shell test, which was written 2026-09-04 by `874de9aa`. The product
refuses correctly with rc=2. Fix = assert the same four parts the Rust test
does. Also: the ok/bad labels for that case disagree with each other.
Observation, NOT this unit: `apexd/apexd-core/src/host.rs:185` still says
"understands up to" with no remedy — same §25 family, untouched.

**Item 2 — measured gap.** 56 top-level verbs, 227 verb+subverb commands.
Against the non-historical doc set (docs/*.md minus m*-notes/m0-results/
p*-progress, plus README.md): 98 of 227 commands documented, 129 not; 34 of 56
top-level verbs documented, 22 not (profile battery fan gaming qualify storage
firmware build send open fingerprint resolve plugin skill provenance firewall
devices remote cloudflare backup blueprint sync). So the reverse direction
CANNOT land green — it lands with a dated debt file plus a ratchet (an entry
that has since been documented must fail as a stale waiver). `docs/remote.md`
is a new file; no branch anywhere has a remote doc, so no merge conflict.
`tests/check-doc-verbs.sh` is invoked by NOTHING — not CI, not any suite.

**Item 3 — p2-010 already measured it (state/agents/p2-010.md:805).** Whole
`system_grants` target, 20 runs → 1 failure (~5%); the two tests that alternate
are `a_grant_from_another_boot_is_reported_as_ended_on_the_next_start` and
`the_ending_is_recorded_once_however_many_daemons_see_it`; neither fails alone
in 25 runs. Both count lines in `privilege-audit.jsonl`.
`audit_lines_once_written(want)` waits on a COUNT, not on the predicate, so any
other startup line satisfies `want=1` early; and in the second test
`audit_lines_once_written(first.len())` returns instantly, making its negative
assertion a no-wait one that passes vacuously. Next step is to read where
`sweep_previous_lives()` runs relative to the socket bind in
`apex-agentd/src/main.rs`: if the sweep is after the bind, it is the PRODUCT.

Order: 1 (done), 3 (done). ITEM 2 IS ALL THAT REMAINS.

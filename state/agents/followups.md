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

Nothing done yet. Do them in the order above; 1 and 3 are correctness, 2 is the
one that keeps coming back.

# followups-2 — a failing test on the tip, and the documentation debt behind it

Repo: apex-os. Branch `task/followups-r19`, from `origin/roadmap/v2.2`.
Worktree `/var/tmp/apex-work/wt-followups2`. No roadmap ids: record what you
close on this card, not with `set-status.py`.

## 1. `tests/test-apex-secret-broker.sh` fails on the integration tip

Measured by the p1-014 agent, not inferred: **64 passed / 1 failed** on the
current tip versus 63/2 on its base. The surviving failure asserts that
`'cloudflare.dns.delete' is not an operation` — which **P1-008 made false** when
it implemented all 32 of section 13.2's names. The test is stale, not the
product; confirm that before changing anything, then make the assertion say what
it is actually for. A failing test carried on the integration tip is worse than
an unfinished feature: it teaches everyone to read a red suite as normal.

## 2. 132 of 259 commands are documented nowhere

`tests/check-doc-verbs.sh` now walks binary → docs as well as docs → binary, and
`tests/doc-verbs-undocumented` records the debt as a **ratchet**: an entry that
has since been documented fails as *stale*, so the file cannot rot quietly.
Worst clusters, from that measurement:

    apex shell   14 of 16 undocumented
    apex env     11 of 13
    apex host     8 of 8
    apex ai       7 of 7

Take them cluster by cluster, writing what the command actually does — read the
source, run `--help`, do not paraphrase the verb name back at the reader — and
remove each one from the ratchet as you go. Finishing two clusters properly is
worth more than touching all four.

## 3. `apexd-core/src/host.rs:185` refuses a future `hosts.toml` with no remedy

It still says "understands up to" and stops there. Section 25 says a version
refusal must name the remedy, because a file from a newer APEX is exactly what
`bootc rollback` leaves behind — `tasks.toml` got that treatment in P1-045.
`tests/test-apex-host.sh:487` is green against the old wording, so **both move
together or not at all**.

## NEXT

Worktree `/var/tmp/apex-work/wt-followups2`, branch `task/followups-r19`, four
commits, all pushed. Ready to merge into `roadmap/v2.2`.

**All three items are done.** Nothing is outstanding on this card.

### 1 — the broker test (commit `144cb46e`)

The file is `tests/test-secret-broker.sh`, not `test-apex-secret-broker.sh`;
the card above had the name wrong.

Confirmed stale before touching it, rather than assumed: the tip answers
`apex secret: cloudflare.dns.delete needs a 'type' option`, which is a build
that knows the operation, parses it, and is asking for its missing argument.
Swapped the closed-vocabulary loop to `cloudflare.account.delete` — §13.2
offers `cloudflare.account.read` and nothing else on an account, so it stays
well-formed and stays undeclared — which is the same substitute
`service.rs::the_vocabulary_is_closed_at_the_grant_and_at_the_use` already
made when P1-007 hit this on the Rust side.

Added the arm the stale case was standing in for: a name the vocabulary DOES
hold must not be refused BY the vocabulary. `cloudflare.dns.delete -o type=A`
is stopped by the grant instead, naming the grant that would allow it.
Without it the loop still passes on a build that has lost every operation.

**64 passed / 1 failed → 66 passed / 0 failed.** Mutation: renaming the
operation's id to `cloudflare.dns.remove` in
`providers/cloudflare/mod.rs` and rebuilding took the new arm red by name
(65/1); restored with `cp`.

### 2 — the documentation ratchet (commits `c48b19d9`, `a89dc9ba`)

Two clusters written properly, from the source and `--help`, not four touched:

* **`docs/hosts.md`** — `apex host`, 8 of 8. ssh-destination-not-address and
  why, `BatchMode=yes` turning a password prompt into a failure, add and probe
  as separate outcomes, the two probe paths and why the fallback is
  `key=value`, nothing a probe returns trusted to bound itself, `--all`
  failing only when every target does, `run` exec'ing ssh so the remote exit
  status is the local one, registry vs probe cache.
* **`docs/ai.md`** — `apex ai`, 7 of 7. Per-user service over root-owned 0444
  shared weights, the three provenance cases and why a URL without a digest is
  refused, no trust-on-first-use, verify-in-staging-then-rename, the 300s/60s
  idle timeouts, why `unload` does not stop the service, and the stated
  limitation that a base-URL-only client cannot reach a Unix socket — with why
  APEX prints the socat bridge and its cost instead of shipping it as a verb.

**Ratchet: 132 → 117 entries**, 15 removed (`apex host` 8, `apex ai` 7). The
reverse pass reported all 15 as STALE before the file was edited, which is the
ratchet doing its job. Reverse-pass summary went
`127 documented, 132 declared, 4 undeclared, 0 stale` →
`142 documented, 121 declared, 0 undeclared, 0 stale`; forward stays at
0 not-a-command.

**The reverse pass was already RED on the tip**, and it is a CI gate
(`pr-validation.yml:1046`). Four verbs landed 2026-09-12 — `apex task audit`
(2814a56d), `apex cloudflare preview` (7f4475dd), `apex secret approve` and
`apex secret approvals` (a67eb704) — documented nowhere and declared nowhere.
They are declared now, with a dated note saying which commit each came from.
Declared rather than written up because each belongs to a wholly undocumented
cluster, and one sub-verb alone would take its parent off the list on a
passing mention. That is why 117 + 4 = 121: the four were never in the 132.
The gate exits 0.

Mutation: renaming the `apex host path` heading out of `docs/hosts.md` took
the reverse pass to `BAD apex host path`, 1 undeclared; restored with `cp`.

Three literals in `docs/ai.md` were checked against source rather than
shipped as drafted — `CHAT_PATH` (correct), `qwen2.5-coder-7b` (invented; the
shipped catalogue holds exactly `qwen25-coder`), and `name@sha256:<hex>`
(described as a user-supplied mapping; it is actually resolved through the
catalogue and a mismatch is refused). `apex build --on katana` was cut from an
early hosts.md draft: it would have taken `apex build` off the ratchet on a
passing mention in an example.

### 3 — `hosts.toml` from a newer APEX (commit `fa022c1b`)

`host.rs`'s Display, `host.rs::a_future_version_is_refused_rather_than_
guessed_at` and `tests/test-apex-host.sh` moved in one commit. The message is
now the `tasks.toml` message from P1-045 for the same file shape, and both
tests assert the same four parts — version in the file, version this build
reads, `rollback`, `Boot the newer deployment` — with `reads version [0-9]+`
rather than a literal, since that is the part expected to move. **52 passed /
0 failed**, `apexd-core` 37 host tests pass, clippy clean.

Mutation: deleting the remedy sentence took the Rust test to FAILED and the
shell suite to 51/1, naming `[Boot the newer deployment]` as the missing part.
Restored with `cp`.

### What is left of the documentation debt, for whoever takes it next

121 entries. Worst clusters now, in order: `apex shell` 14 (thin IPC wrapper
over quickshell — readable from source, but the verbs must not be RUN on
Andre's machine, they open windows), `apex env` 11, `apex task` 7 + the new
`audit`, `apex backup` 8, `apex cloudflare` 3 + the new `preview`, `apex
secret` 3 + the two new approval verbs. §20's handoff verbs `apex build`,
`apex send` and `apex open` are named in the ratchet's dated note because
`docs/hosts.md` wants to link to them and cannot.

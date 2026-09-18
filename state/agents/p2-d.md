# p2-d — secure browser automation capsule

items: P2-008, P2-009, P2-012
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-d
branch: **task/p2-d-6** (off roadmap/v2.2 @ 6b531503)

> **PROTOCOL_VERSION IS 10 AS OF ROUND 30.** The per-run browser CA bind
> (`RunRequest::trust_ca`, gap 5) TAKES 10 — `BROWSER_CA_VERSION = 10`.
> **ROUTE B'S TLS FIELDS MUST TAKE 11, NOT 10.** `docs/browser-capsule-auth.md`
> said 10 for route B before this round; round 30 rewrote those three places.
> Why 10 and not a ride on 9: a daemon that drops this field leaves the capsule
> distrusting the CA, so Firefox sits on `UNKNOWN_CA` until `--timeout` kills
> it — five silent minutes and then "the capsule did not finish", which is not
> the loud refusal `second_factor`'s no-bump argument rests on.
> `check_daemon_understands` runs BEFORE `Run` and consumes a number; a
> `SessionInfo` echo could only be read after the browser had already started.

> Rounds 1-5 are landed. `task/p2-d-2`, `-4` and `-5` are merged — do not commit
> onto any of them. The durable account of what was built is
> `docs/browser-capsule.md` + `docs/browser-capsule-auth.md`; round 3's long
> archive is `/var/tmp/apex-work/scratch-p2-d/p2-d-card-round3-archive.md`.

## NEXT

**Round 30 is complete and everything is pushed.** Gap 5 is closed and recorded
against P2-012, P2-008 and P2-009 (evidence re-parsed afterwards; every earlier
round survived — P2-012 is 31.5 KB now).

The next action for whoever picks this up is NOT engineering. It is getting
Andre's answer to the question under "The question, for Andre" in
`docs/browser-capsule-auth.md`: may `apex-agentd` read the plaintext of a
capsule's connection to the one destination that capsule was pinned to, in
order to add a credential the capsule is never given? If yes, route B is a
round of its own and **it takes PROTOCOL_VERSION 11** — 10 is gone. If no,
P2-012's "capability auth" is permanently unmet rather than pending and the
item should say so.

## THIS ROUND'S SCOPE (round 30), narrow on purpose

**Gap 5 only — the per-run CA bind.** Route B is NOT being built: it is blocked
on Andre's decision, written out under "The question, for Andre" in
`docs/browser-capsule-auth.md`. Building the TLS server before the answer
exists would be a round that may have to be deleted.

Design taken (advisor-reviewed) before any code:

- `RunRequest::trust_ca: Option<String>` — an absolute host path to a
  PEM file of certificates. It does NOT make the whole session trust a CA:
  `curl`, `git` and `python` in the same sandbox keep using the system bundle
  and still refuse the intranet host. It installs a **Firefox
  enterprise-policy** root, because that is the only CA install route in the
  image (`nss-tools` is absent, so there is no `certutil`).
- The daemon writes the policy, not the engine. A generic "bind this file over
  that path" wire field would let any caller shadow any path inside a session's
  namespace; `install_browser_ca` mirrors `install_redacted_settings` instead.
- Refused without a confined sandbox — no namespace, nothing to bind into, so
  the field would silently mean nothing. Same shape as `--ttl` with no grant.
- The host's own `/etc/firefox/policies/policies.json` is MERGED, not replaced:
  it carries four `Status: "default"` preferences and the `//` comment keys,
  and a capsule that lost them would be a capsule with different defaults from
  every other browser on the machine.

## DONE (round 30, branch task/p2-d-6)

- `2fc59c6b` — **the docs**. `docs/browser-capsule.md`'s NOT BUILT list no
  longer says a capsule cannot be told to trust a CA; a section replaces it.
  `docs/browser-capsule-auth.md` reserved protocol 10 for route B while gap 5
  did not exist — **route B is 11 now**, corrected in all three places with the
  reason rather than silently. The "What is not decided" bullet is struck and
  records the two things the build added that it did not foresee (the merge,
  and the refusal on an absent policy file). One stale sentence fixed: the
  shape of the shipped `policies.json` IS asserted, by round 29's
  `Containerfile.base` addition.
- `8e1ca9cc` — **the capsule itself says what it sees**:
  `apexd/apex-agentd/tests/browser_ca_bind.rs`, a private daemon and a confined
  session that copies out what it finds at
  `/etc/firefox/policies/policies.json` INSIDE its namespace, reads the path
  that document names, and copies that out too. Four cases, two controls that
  fail differently. `/etc` is read and its sha256 asserted unchanged. Two
  mutations red — deleting the `ro_at` push printed the LIVE machine policy in
  the failure, which is how the file proves it reads through the namespace and
  not through a fixture. **Also fixed a defect in my own `2fff910f`**:
  `BrowserCmd::Run` crossed clippy's `large_enum_variant` threshold when the
  flag was added and that commit was pushed without clippy having been run
  over it. 56 suites green, clippy clean.
- `2fff910f` — **`apex browser run --trust-ca FILE`**: the clap surface, the
  engine, and 11 new suite assertions that the flag reaches the `apex agent
  run` line and lands among the RUNTIME's flags rather than after the `--`
  (after it, firefox has no such flag and the capsule fails naming the wrong
  program). Engine refusals, each asserted to leave no session behind: a
  relative path, a file that is not there, a file inside `$BROWSER_ROOT`.
  Five mutations, all red, all restored byte-identically. One assertion was
  written wrong first — `${#argv%%<pat>*}` is a bad substitution, so it
  printed an error and asserted NOTHING, neither PASS nor FAIL. The dominant
  defect family, inside a test written to avoid it.
  116 passed / 0 failed (was 105); `shellcheck -S warning -x` clean.
- `b0ef1b2c` — **the daemon half: `apex-agentd/src/browser_ca.rs`.** Copies the
  PEM where the session can read and not write it, merges a
  `Certificates.Install` into a copy of the MACHINE's own
  `/etc/firefox/policies/policies.json`, binds that copy over the real one in
  the namespace (`spec.ro` twice + one `spec.ro_at`, which is
  `install_redacted_settings`' shape). Four refusals, not comments: an absent
  host policy REFUSES (a `--ro-bind-try` over a missing target is a silent
  no-op); only `CERTIFICATE` blocks may go in (a key beside the cert is the
  ordinary mistake and the copy lands where the agent can read it); DER is
  refused with the `openssl` line; a machine that already installs
  certificates refuses rather than merging two trust lists silently. Six
  mutations, each restored byte-identically with sha256 checked. The install
  test's scratch directory is reached through a SYMLINK on purpose — with a
  plain path the canonical and literal strings are equal and the assertion
  would be checking nothing.
- `1fff2c45` — **PROTOCOL_VERSION 10, `RunRequest::trust_ca`, and the CLI's
  refusal to send it to a daemon that would drop it.** The revision number was
  the decision and it is argued in the commit and at `PROTOCOL_VERSION`: this
  is the first guarded field whose dropped key fails CLOSED, and it gets a
  number anyway because the closed failure is silent for `--timeout` seconds
  and then blames the timeout. `AgentCmd::Run` is boxed — `RunArgs` crossed
  clippy's `large_enum_variant` threshold when the flag was added, and that is
  the flag's cost rather than tidying. Four mutations, each restored
  byte-identically: the wire key renamed (key assertion red),
  `BROWSER_CA_VERSION = 9` (compile-time ordering assert red),
  `PROTOCOL_VERSION` left at 9 (same), the `--trust-ca` row deleted from
  `settings_a_daemon_could_drop` (table test red, `left: []`).
  55 suites green, clippy `--all-targets -D warnings` clean.

## DONE (round 29, branch task/p2-d-5, 4 commits, all merged as bde4d96c)

- `12ee0cbf` — **the Containerfile gate could not see a `python3 -c` assertion
  at all.** `tests/check-containerfile-assertions.sh` resolved `grep` and
  nothing else; a python segment missed the regex and hit `continue` — not a
  failure, not UNRESOLVED, not counted, which that file's own header calls the
  worst of the three outcomes. It was doing it to seven of its own assertions.
  Now 193 checked instead of 188, with the two it still cannot follow counted
  (one path from a `for` variable, one pipe).
- `73861e55` — **the `policies.json` shape assertion**, which had been round 3's
  NEXT and round 4's. Positive, not a denylist: `policies` carries `Preferences`
  and nothing else, every preference at `Status: "default"`. 194 checked, 0
  failed.
- `68f3e5c1` — **route B's join, measured**, and the question above written into
  `docs/browser-capsule-auth.md` under its own heading.
- `67861c53` — the runner half of `12ee0cbf`'s own argument was **reasoned, not
  measured**, in a file whose subject is that difference. Measured now: on a box
  with no `/etc/firefox` the unrewritten handler fails six assertions with
  `FileNotFoundError` and exits 1, and the shipped one reports 194 checked / 0
  failed there. Comment only.

## FOUND (round 29, kept because it is still load-bearing)

- **`/etc/firefox/policies/policies.json` EXISTS on an APEX machine and is read
  inside every capsule** — the sandbox binds `/` read-only. That is what makes
  gap 5 possible and it is also why the round-29 image assertion exists.
- **Firefox tells Mozilla when an enterprise root is installed.** The probe saw
  it reach for `mitmdetection.services.mozilla.com`. A capsule under
  `--capability` refuses that host because the pin does not contain it.
- Round 4's recorded inconsistency is still there and still unfixed: the
  engine's pre-check at `apex-browser` ~L505 falls back to the bare host, so
  `--allow e.example:8443` passes it when only `e.example` is allowed and the
  DAEMON refuses it. Fail-closed; the two just do not agree.

## FOUND (round 30)

- **Two defects in my own work, both caught by RUNNING a gate rather than
  reading.** The suite assertion "the CA lands before the separator" was first
  written with `${#argv%%<pat>*}` — bash rejects that as a bad substitution, so
  it printed an error and asserted NOTHING, neither PASS nor FAIL. And
  `2fff910f` was pushed without `cargo clippy` over it: `BrowserCmd::Run`
  crossed `large_enum_variant` exactly as `AgentCmd::Run` had. Run the gate
  before the commit, not after the push.
- **Nothing populated `SandboxSpec.ro`/`ro_at` from the wire before this
  round.** That is why gap 5 was a code change rather than a flag, and it is
  why the generic shape — a wire field naming a file AND a path to bind it over
  — was rejected: it would let any client shadow any path in any session's
  namespace.
- **`--ro-bind-try` over a path that does not exist is a silent no-op.** It is
  the whole reason `browser_ca::install` refuses when the machine has no
  `/etc/firefox/policies/policies.json`, and it is worth knowing for anything
  else that binds over `/etc`.

## BLOCKED ON

- **P2-008's USB passthrough is not closable by any suite.** It means detaching
  a physical device from the machine running the tests.
- **P2-009 needs a guest image that carries an agent CLI.** A build, not a test.
- **P2-012's route B is blocked on a product decision, not on engineering.**
  See the question in `docs/browser-capsule-auth.md`. Gap 5 is independent of
  it and is what round 30 is doing.

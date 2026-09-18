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

Edit `apexd/apex-agent-core/src/protocol.rs`: add `BROWSER_CA_VERSION = 10`,
bump `PROTOCOL_VERSION` to 10, add `RunRequest::trust_ca: Option<String>`, add
the `const _: () = assert!(SESSION_ALLOWLIST_VERSION < BROWSER_CA_VERSION)`
ordering guard, move `SESSION_ALLOWLIST_VERSION` into the strictly-behind list
in `every_version_guard_names_a_revision_that_exists` and make
`BROWSER_CA_VERSION` the one asserted `== PROTOCOL_VERSION`. Then
`cargo test --locked --workspace` from `apexd/`.

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

- nothing pushed yet.

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

## BLOCKED ON

- **P2-008's USB passthrough is not closable by any suite.** It means detaching
  a physical device from the machine running the tests.
- **P2-009 needs a guest image that carries an agent CLI.** A build, not a test.
- **P2-012's route B is blocked on a product decision, not on engineering.**
  See the question in `docs/browser-capsule-auth.md`. Gap 5 is independent of
  it and is what round 30 is doing.

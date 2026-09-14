# p2-f
items: P2-017, P2-018, P2-019  (P2-016 — see NOTE at the bottom, unchanged)
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-f
branch: task/p2-f-3   (cut from origin/roadmap/v2.2 @ 13d53c01)

## NEXT
Round 5 commit 1: add `supersedes_credentials: bool` to `OperationSpec` in
`apexd/apex-secret-core/src/operation.rs` (mandatory, no Default — mirrors
`same_everywhere`), write `false` at every one of the ~85 literals the grep
`rg 'same_everywhere' apexd --type rust` lists, add
`may_supersede_credentials(op)` beside `may_be_granted_everywhere` in
`apexd/apex-secretd/src/service.rs`, and a registry test in
`apexd/apex-secretd/src/providers/mod.rs` mirroring
`the_everywhere_gate_reads_the_operations_own_declaration`.

## ROUND 4 PLAN (settled with the advisor; do not re-litigate)
1. ~~OAuth vocabulary + tests~~ **DONE, `9e0a8c7d`, pushed.** Tip merged in.
2. **`replaces` is a `Vec`, not an `Option`.** RFC 6749 §6 lets the server issue
   a NEW refresh token, and Wrangler writes one back — so assume Cloudflare
   rotates. Storing only the new access token would leave the old refresh token
   dead at the server and the account broken until `apex cf connect`. So a
   refresh replaces TWO credentials: `cloudflare` (access) and
   `cloudflare-refresh` (the rotated refresh).
3. **`Replaced { name: String, value: SecretValue }` — value-only, on purpose.**
   No host, no scheme, no username: the framework reuses the EXISTING
   `ServiceInfo` and swaps the value, so a refresh structurally cannot repoint
   a credential's pin. That is a property of the type rather than a check.
   `store.put` overwrites in place and leaves grants/approvals alone (verified
   in `store.rs`), so no new store method and no window with no credential.
4. Framework checks in `service.rs`, mirroring the `creates` block: name is
   `valid_service_name`; the name must ALREADY EXIST (a replace of a missing
   name is a `creates` that skipped the free-name check); `!replaces.is_empty()
   && !may_supersede_credentials(op)` refuses; post-perform each returned name
   must be in the declared set; `replaced` values join the `scrub_all` slice —
   a token endpoint's reply IS the secret.
5. `oauth` provider: copy `s3/mod.rs::call`'s curl-config pattern into
   `broker::run_curl`. `api::call` cannot be reused (hardwired `/client/v4` and
   a Bearer header, no form body). Needs a `#[cfg(test)]` table-injection point
   like `CloudflareProvider::at(port)`, because a loopback double is
   `127.0.0.1` and `oauth_for_auth_host("127.0.0.1")` is `None` by design.
6. `same_everywhere: false` on the refresh op — `true` would drag it into the
   two-projects bind fixture, which is MCP-shaped and panics on a bind failure.
7. Check whether `apex cf refresh` can pass the per-project grant check at all,
   or whether `cf connect` must record the grant when it stores the refresh
   token. That answer decides what `cloudflare.rs:681`'s status line says.

Round 2's branch `task/p2-f-2` landed as `f2229185`.

**Round 3's queue changed on the first look, and this is the reason:**
**item 1 of round 2's list — the S3/R2 SigV4 signer — WAS BUILT BY ANOTHER
UNIT and is on the tip already**, as `5054be77` "feat(secretd): an S3
provider, with a SigV4 signer pinned against botocore" (verified
`git merge-base --is-ancestor 5054be77 origin/roadmap/v2.2`). Do not build it
again. What it leaves behind is in FOUND.

Remaining, in the order this round takes them:

1. **NEXT ACTION**: the OAuth vocabulary in `apex-secret-core/src/account.rs`
   (`OAuth { device_url, token_url, auth_host, scopes, client_secret }` + a
   `oauth_for_auth_host()` lookup covering Google, Microsoft AND Cloudflare),
   then `ProviderSpec::may_supersede_credentials` + `Bound::replaces` +
   an `oauth` provider in `apex-secretd` that performs RFC 6749 §6 refresh.
   Prove it on CLOUDFLARE's shape, because Cloudflare is the only one of the
   three with a transport that can spend the refreshed token.
2. ~~The vapour-scope defect~~ **DONE, `c224eea7`.**
3. P2-019 fleet transport + server side.
4. gvfs — a design paragraph only; not enough budget to build it.
5. P2-018 criterion 2 — the exact recipe that would close it.

## DESIGN DECISIONS TAKEN THIS ROUND (so a successor does not re-litigate them)

- **Refresh is `Bound::replaces`, a sibling of `Bound::creates`.** Rejected:
  a new `Request` variant (that is the second write path `account.rs`'s module
  note refuses) and framework-driven refresh at `Use` time (the right product
  end-state, but `ServiceInfo` has no `expires` and no `refresher` field —
  `added` is its only timestamp — so it needs a protocol change to `Add` and
  orchestration in `service.rs`; that is a round of its own, recorded here as
  the follow-on with those field names).
- `service.rs:1053` refuses a `creates` name that is already taken, for a
  reason that is true of creation and false of refresh: the far side issues
  once. A refresh's whole purpose is to supersede. So `replaces` is gated by a
  STATIC flag on `OperationSpec`, enumerated by a test in `providers/mod.rs`
  mirroring `the_everywhere_gate_reads_the_operations_own_declaration`.
- One `oauth` provider serves Google, Microsoft and Cloudflare: RFC 6749 §6 is
  the same request at all three. `bind` maps the refresh credential's pinned
  host to its token endpoint from a hard-coded table, which is pin-consistent.

## DONE
Round 1: landed as merge 4e8969ef (10 commits).
Round 2: landed as merge f2229185 — P2-018 criterion 1, the recovery verb.
Round 3, on task/p2-f-3 (pushed):
  c224eea7  a scope may not name an operation no provider offers — the
            cross-crate gate in apex-secretd, both mutants run and red.

## IN PROGRESS (round 5)
- `git status` run: worktree CLEAN as of the merge of `origin/roadmap/v2.2`
  (2219ef75) into `task/p2-f-3` as `91c30b14`. Nothing half-written yet.
- About to touch: `apexd/apex-secret-core/src/operation.rs` (`OperationSpec`).

## ROUND 5 COMMIT SEQUENCE (settled with the advisor)
1. `supersedes_credentials` + the ~85 `false` literals + the gate + the
   registry test. The `oauth` provider does NOT land here, so the registry
   test's superseding set is asserted **empty** in this commit and becomes
   `["oauth.token.refresh"]` in commit 3.
2. `Bound::replaces` / `Performed::replaced` / `Replaced { name, value }` +
   the framework checks in `service.rs` + a `Replacer` test provider mirroring
   `Creator` (careless output that contains the new secret, `ran` measured).
3. the `oauth` provider + a loopback double: rotated refresh (two `Replaced`),
   non-rotated (one), non-2xx, a token carrying `"` / `\` / newline.
4. `apex cf refresh` + the `cloudflare.rs` status line + the step-7 answer.

## FOUND
- **`5054be77` landed the S3 provider and SigV4 signer.** Round 2's card
  listed it as this round's item 1; it is done, and by somebody else.
- **VAPOUR SCOPES — a real defect the S3 landing exposed.**
  `apex-secret-core/src/account.rs` lets a user run `apex account grant
  google files.read`, which records a grant for operation `gdrive.file.read`.
  **No provider in `default_registry()` offers `gdrive.*` or `msgraph.*`, and
  `S3_SCOPES` names `s3.object.list` which the new S3 provider does not offer
  either** (it has `s3.object.read` and `s3.object.write`). Nothing checks
  this today because `apex-secret-core` cannot see the registry — but
  `apex-secretd` depends on `apex-secret-core`, so a test THERE can, and that
  is where the gate belongs.
- Nothing in this build refreshes any token, Cloudflare's included, and
  `apex cf status` prints that fact ("a refresh token is stored too, and
  nothing spends it yet") — so if refresh lands, that line has to change.

## BLOCKED ON
(nothing)

## NOTE — card/dispatch mismatch, unresolved on purpose
Round 2's dispatch also named **P2-016**, whose evidence belongs to unit
`p2-016-multiuser-2` (landed 2026-09-12 as merge 5eca2402). Rounds 2 and 3 did
no P2-016 work and did NOT call set-status on it — set-status.py REPLACES
evidence, and writing it would have destroyed that unit's record. Whoever owns
the queue should settle which unit holds P2-016.

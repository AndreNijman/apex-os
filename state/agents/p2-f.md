# p2-f
items: P2-017, P2-018, P2-019  (P2-016 — see NOTE at the bottom, unchanged)
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-f
branch: task/p2-f-3   (cut from origin/roadmap/v2.2 @ 13d53c01)

## NEXT
ROUND 3 IN PROGRESS. Round 2's branch `task/p2-f-2` landed as `f2229185`.

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

## IN PROGRESS
- Nothing half-written. `c224eea7` is committed and pushed.

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

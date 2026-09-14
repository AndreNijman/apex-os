# p2-d — secure browser automation capsule

> **PROTOCOL BUMP IN FLIGHT — `PROTOCOL_VERSION` 8 -> 9** on branch
> `task/p2-d-4`, for `RunRequest::allow` (per-session allowlist narrowing).
> Rebase onto it rather than editing `apexd/apex-agent-core/src/protocol.rs`
> beside it. Landed in a commit? see DONE.

items: P2-008, P2-009, P2-012
repo: apex-os
worktree: /var/tmp/apex-work/wt-p2-d
branch: task/p2-d-4 (off roadmap/v2.2 @ 654fa854)

Rounds 1-3 are landed (`14650ca3`, `13d53c01`, `be9844cc`); their long account is
`/var/tmp/apex-work/scratch-p2-d/p2-d-card-round3-archive.md`, and the durable
version is `docs/browser-capsule.md` + `docs/browser-capsule-auth.md`. Read
those, not this, for what was built.

## NEXT
Add `RunRequest::allow: Option<Vec<String>>` + `SESSION_ALLOWLIST_VERSION = 9`
to `apexd/apex-agent-core/src/protocol.rs`, bump `PROTOCOL_VERSION` to 9, and
fix the two version tests at `protocol.rs:2192-2215` (`assert_eq!(
PLUGIN_POLICY_VERSION, PROTOCOL_VERSION)` becomes the new constant).

## DONE
- (nothing pushed this round yet)

## IN PROGRESS
- nothing half-written yet.

## FOUND
- Round 3's own NEXT was never done: the `policies.json` shape ALLOWLIST
  assertion in `Containerfile.base` (beside line ~501) is still absent. Round 3
  measured that while `/etc/firefox/policies/policies.json` exists it is the
  ONLY policy file any Firefox on the machine reads — capsule or not — so an
  unasserted shape is every capsule's trust surface.

## BLOCKED ON
- nothing.

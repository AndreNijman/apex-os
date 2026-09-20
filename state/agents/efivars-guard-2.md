# efivars-guard-2 — the prevention layer, after the first one turned out inert

items: none (dispatched directly off BOOT-BREAKAGE-2026-09-20.md and the
       second occurrence recorded on `luks-installer`'s card)
repo: apex-os
worktree: /var/tmp/apex-work/wt-efivars-guard-2
branch: task/efivars-guard-2 — 3 commits, `602a8376..1edbd9e9`, all PUSHED, NOT landed
base: roadmap/v2.2 @ 602a8376
evidence: ROADMAP/evidence/efivars-guard-2-20260920.md
suite: tests/test-bootc-install-guard.sh — **71 passed, 0 failed**
files owned: tests/lab/bootc-install-lab, tests/lab/nvram-guard,
             tests/test-bootc-install-guard.sh,
             ROADMAP/evidence/efivars-guard-2-20260920.md
files shared: AGENTS.md (rule 6 rewritten),
             .github/workflows/pr-validation.yml (one step name + its comment),
             ROADMAP/evidence/efivars-guard-20260920.md (superseding banner)
**touched of another unit's territory: `docs/boot-v2.md`, ONE clause** at the
             old line 1376. It said the wrapper "cannot be talked out of the
             efivars mask" — true and irrelevant. `sdboot-migrate` /
             `sdboot-image` own that file; the edit is deliberately one
             sentence so a conflict resolves in seconds. Nothing else in it
             was touched.

## The finding, in one paragraph

The guard that landed on the morning of 2026-09-20 called `--tmpfs
/sys/firmware/efi/efivars` the layer that prevents a loopback `bootc install`
from rewriting this machine's boot entry. **It prevents nothing.** `bootc`
takes `--pid=host` and re-enters the **host's** mount namespace for the
bootloader step, and an unmasked privileged container shows *zero* entries
under `/sys/firmware/efi/efivars` in the first place — there was never a host
efivarfs inside the container to mask. The prevention is `bootc install
--generic-image` ("Changes to the system firmware will be skipped").

## Which layer is which — say this, do not blur it

| layer | kind |
| --- | --- |
| `bootc install --generic-image` | **PREVENTION** — the only thing that stops the write |
| `tests/lab/nvram-guard` | **DETECTION** — cannot prevent; has caught this twice, the only layer that ever has |
| `--tmpfs /sys/firmware/efi/efivars` + the `-v /sys:/sys` refusals | defence in depth, **inert against bootc** |
| device-target / tmpfs-target refusals | hygiene; nothing to do with NVRAM |

## What a caller must pass, and what happens if they do not

**Nothing.** `--generic-image` is not an option on `bootc-install-lab`; the
wrapper puts it in the argv and cannot be talked out of it. If the tagged line
is removed from the file, `generic-image-present` fails, **exit 7**, and podman
is never invoked. The refusal names the assertion and explains the namespace
hop so the next reader does not reach for the mask again.

Proven without writing to firmware: delete the line from a **copy** of the
wrapper, assert exit 7, the assertion name, that the stub podman recorded
nothing, and that no target image was created. No `bootc install` runs, no
container starts, no EFI variable is read or written anywhere in the suite.

## Three things a stranger most needs to know

1. **Neither incident was a `--via-loopback` command.** Both were
   `installer/apex-install` on a `to-filesystem` install against a loop
   **device** it had attached itself. The surviving log of the first is
   `/var/lab-scratch/apex-luks-live.oRDxu4/engine-stdout.txt`, and its
   `Boot0000 … ee26313c-…` line is byte-for-byte the one in Andre's write-up.
   `01:02:17` in that write-up is UTC — **09:02:17 AWST** locally, matching the
   file's mtime. So the repository scan, which keys on the literal
   `--via-loopback`, would not have caught either one. That limit is now
   written into the suite header instead of being left to be discovered.
2. **The repository scan was backwards and is inverted.** It flagged a loopback
   install with no `/sys/firmware/efi/efivars` — the inert property — so it
   would have PASSED a command that writes NVRAM and FLAGGED one that cannot.
   The **old predicate is kept in the suite** and asserted to pass the
   dangerous fixture; if that ever stops holding, the two have converged and
   one is wrong.
3. **The `violating.sh` fixture lied.** It said "this is what the 2026-09-20
   run looked like" and carried `--generic-image`. Had it been a real
   transcript, the flag would not work and this whole unit would be wrong. It
   was a transcript of nothing. Check that before trusting any fixture comment
   in this repository.

## NEXT

1. **Land it.** Three pushed commits, base `602a8376`, suite green 71/0,
   `shellcheck -S warning -x` clean on all three files,
   `check-no-conflict-markers.sh` and `check-doc-verbs.sh` pass. Merge (round
   10 changed landings from cherry-pick to merge — a 12-commit cherry-pick
   conflicted on commit 1).
2. **`installer/test-installer-luks.sh`'s NVRAM section is now the only thing
   covering the path that caused both incidents.** The static scan cannot see
   `apex-install`'s loop-device call sites. If that section is ever weakened,
   nothing replaces it — say so to whoever touches it.
3. **No `to-disk` loopback install has been run since `--generic-image` was
   added anywhere.** `luks-installer` proved it live on `to-filesystem` (run 8,
   41/0, `nvram-guard` verdict `verified`). `bootc-install-lab` is `to-disk`.
   Prevention there rests on bootc's documented semantics plus that evidence,
   **not on a measurement**. Whoever runs the first one: run it under
   `nvram-guard`, keep the `--out` snapshot pair, and append the verdict to
   `ROADMAP/evidence/efivars-guard-2-20260920.md` §7.
4. **`android/tools/release-version.sh` fails `tests/check-shellcheck-coverage.sh`
   on `roadmap/v2.2` already** (SC2034, `head_sha` unused, line 92). Pre-existing,
   not caused here, belongs to `android-release`. Verified against
   `origin/roadmap/v2.2`, not assumed.
5. **`BOOT-BREAKAGE-2026-09-20.md` is Andre's own file and was left alone.** Its
   "Cause" section names `bootc install to-disk --via-loopback` and follow-up 1
   asks for the mask. Both are wrong in the way this unit documents. Ask him
   before editing his write-up; the correction is in
   `ROADMAP/evidence/efivars-guard-2-20260920.md` §3 meanwhile.
6. **Tell `sdboot-migrate`** about the one-clause `docs/boot-v2.md` edit above.

## Traps paid for here

* Inside a **double-quoted** bash string an apostrophe needs no escaping; the
  `'"'"'` idiom is for single-quoted strings and terminated the string instead.
  `bash -n` reported it as an unexpected EOF ~60 lines further down.
* A mutation anchored to a bare phrase (`bootc install to-disk`) also matches
  the file's own header prose. Anchor to the executable line
  (`PODMAN_ARGS+=("$IMAGE" bootc install to-disk`) and assert the count both
  before AND after, or a sed that rewrote a comment "passes".
* `grep -rl` over `/var/lab-scratch` and `/var/tmp/apex-work` exceeds 120 s —
  those trees hold multiple 40 GB disk images. Search transcripts instead.
* Agent transcript timestamps are **UTC**; this machine and its logs are AWST.
  An eight-hour offset is what made the first incident's log hard to find.

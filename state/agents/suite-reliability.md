# suite-reliability
items: three test/CI-infrastructure defects found by other units, none theirs to fix
repo: apex-os
worktree: /var/tmp/apex-work/wt-suite         branch task/suite-reliability
base: origin/roadmap/v2.2 @ f39fd664 (no rebase)

## NEXT
Starting. Card written before the first edit. Order of work:
1. `read_sync` deadline (own commit, so every later run fails in seconds).
2. envp-before-fork + `execve` (the deadlock proper).
3. Measure the stress reproduction; keep it only if it trips reliably.
4. egress test read deadline.
5. handoff_packet `XDG_CONFIG_HOME` + a hermeticity assertion.
6. build-image.yml publish guard, then a dispatch on THIS branch.

## FOUND BEFORE ANY EDIT — task item 3's premise is FALSE, and the real defect is worse

The brief says `build-image.yml` "triggers only on `main`" and asks for
`workflow_dispatch` to be added. **`workflow_dispatch` is already there**, on
both `roadmap/v2.2` and `main`, with three inputs (build_qcow2, build_iso,
force_core), since `48fb1b26`. `gh workflow run build-image.yml --ref
roadmap/v2.2` was always available. Only the *push* trigger is main-only.

So the reason nobody had image-built the integration branch is not that they
could not. It is that doing so would have been **destructive**, and that is the
finding:

- `Promote to every published tag` is guarded by `steps.push.outputs.digest
  != ''` and **nothing else** — no ref check. It moves `apex`, `daily`,
  `gaming-mesa`, `gaming-nvidia`, every `platform-*`, and `edge`.
- `push-retry.sh` (a copy in each of the core, base and image jobs) always
  moves the floating `:core` and `:base` tags after the per-SHA push.
- Those floating names are what installed machines track (`bootc` on
  `:daily`/`:gaming-*`), what `release-shell.yml` builds FROM (`platform-*`),
  and what a base-only main build resolves core from ("resolving the last-good
  core digest from GHCR").

A single `gh workflow run build-image.yml --ref roadmap/v2.2` would therefore
have shipped unreviewed integration content to **every deployed machine** on
its next `apex update`, and left a `:core` behind that the next main build
would have built on top of. The missing thing is not a trigger. It is the guard
that makes the trigger safe to use.

Plan: `PUBLISH: ${{ github.ref == 'refs/heads/main' }}`; per-SHA tags always,
floating names and `edge` only when PUBLISH; the tag-resolves assertion guarded
too, so a non-main run cannot go red for a reason that is not the build. Then
dispatch **on `task/suite-reliability`** (which is `roadmap/v2.2` plus this
unit's commits) — never on `roadmap/v2.2`, because a dispatch runs the workflow
file *from the ref dispatched*, so the guard would not be in effect there.

# build-verify — does roadmap/v2.2 still build?

repo: apex-os
worktree: /var/tmp/apex-work/wt-build-verify
branch: **task/build-verify** (off roadmap/v2.2 @ f666f5c1)

> IN PROGRESS as of 2026-09-19 ~02:30 AWST. This card is rewritten when the
> local build and the CI dispatch finish. Do not act on it until the
> "IN PROGRESS" line is gone.

## What is already measured

- `quickshell --version` from `quickshell-git-0.3.1^860.gitc6a5160-1.fc43.x86_64`
  (errornointernet COPR, on quay.io/fedora/fedora-bootc:43) prints, on **stdout**,
  exit 0, one line, 126 bytes:

      Quickshell 0.3.1 (revision c6a516096dd84d5255b409482eb4bf740b952f88, distributed by Fedora COPR (errornointernet/quickshell))

  The assertion `36535383` added matches it. Replayed verbatim under `set -eux`:
  ASSERTION_PASSED, exit 0.
- No `SHELL` directive in any Containerfile and the RUN is `set -eux`, so
  **pipefail is not in scope** and the `printf | grep -qE` 141-on-match trap does
  not apply here. Checked, not reasoned about.
- CI: the new multilib extraction step on the tip (run 35414179241) genuinely ran
  on the runner — `6 passed, 0 failed`, negative control 14 shadows.

## NEXT

Finish the local build (`/var/tmp/apex-work/build-verify.log`, unit
`apex-build-verify`) and the CI dispatch (run 35415266422), then rewrite.

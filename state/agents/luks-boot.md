# luks-boot — L-002, finish it: the installer makes a LUKS2 disk AND that disk boots

items: L-002
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-boot
branch: task/luks-boot (3 commits ahead of origin/roadmap/v2.2 at dispatch, clean, pushed)

Predecessor cards to read before touching anything:
`ROADMAP/state/agents/luks-installer.md` (first encrypted install, LANDED round 35),
`ROADMAP/state/agents/luks-installer-2.md` (the boot suite + --check-passphrase;
its work is already MERGED into this branch at 3e235bc1 — do not redo it),
`ROADMAP/state/agents/efivars-guard-2.md` (--generic-image, already resolved).

## NEXT (fill in as you go)
1. Re-run `installer/test-installer-luks-boot.sh` so it is NOT killed. See
   FOUND #1: run1 did not fail, it was SIGTERMed at 869s mid-`bootc install`.

## DONE
- Read the branch's three commit messages and both predecessor cards.
- Read `/var/lab-scratch/luks-installer-2-boot-run1.log` — the run the
  predecessor never got to read. See FOUND #1.

## FOUND
1. **run1 measured nothing. It was killed, not failed.** `engine exit=143
   after 869s`, with the literal word `Terminated` printed against the
   `podman ... bootc install to-filesystem` line, while the engine was still
   at `Deploying container image...` (`layers already present: 0; layers
   needed: 296 (15.5 GB)`). 143 = 128+SIGTERM. The predecessor launched the
   suite in the background and its session ended; this is exactly the
   known-defect shape recorded as "a backgrounded podman is SIGTERMed and
   still exits 0 — a dead agent's last run is not a defect until re-run
   foreground". So `0 passed, 3 failed` in that log is NOT evidence about
   the installer, and nothing in it should be quoted as a boot result.
   Artefacts kept at `/var/lab-scratch/apex-luks-boot.qclEik` (26 GB
   half-written `target.img`) — deletable, the install never finished.
2. Preconditions for a re-run, checked on this machine at 11:50 AWST
   2026-09-21: `localhost/apex-os:daily` and `localhost/apex-bootlab` both
   PRESENT in ROOT podman storage (so neither the image build nor the lab
   build has to happen inside the run); 307 GB free on `/var`; 21 GB
   available RAM; `/dev/kvm` present.

## IN PROGRESS
- (nothing yet)

## BLOCKED ON
- (nothing yet)

## FOR L-003 (luks-enroll, not dispatched this round)
- (nothing yet)

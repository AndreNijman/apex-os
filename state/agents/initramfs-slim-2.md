# initramfs-slim-2 — continuation of initramfs-slim

items: none directly (ESP budget; feeds P0-001 and the boot-v2 work)
repo: apex-os
worktree: /var/tmp/apex-work/wt-initramfs-slim-2 (create it)
branch: task/initramfs-slim-2, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/initramfs-slim-2

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## WHAT LANDED — items 1, 2, 3 and 7 are DONE

`task/initramfs-slim` **landed** at `ede9a6f7`, with
`ROADMAP/evidence/initramfs-slim-20260921.md` on the tip. Read
`initramfs-slim.md` for the full history. What it achieved: a 512 MiB ESP now
gets a deployment of **114.6 MiB instead of 375.2 MiB**, and the Containerfile
assertion that FATALed on the branch's own initramfs is replaced by a real
predicate with a 35-assertion CI suite proving it red on a fat image and green
on a slim one.

**The caveat that unit stated rather than buried, and which is now spent:** no
full image build had run the new stanza end to end. One has — build run
`35624291221` at `44c9a5cb` went green through `base` (the first `base` pass in
this program) and katana is booted on the result. So `COPY
files/scripts/check-initramfs-budget.sh` plus its call **do** execute together.
Record that; it closes the open risk on the predecessor's card.

## WHAT IS LEFT — items 4, 5 and 6 of that card's own NEXT

4. **Regenerate the size matrix with attribution** — 4 dracut runs, apex-tier
   flags, so the 375.2 → 114.6 MiB is broken down by what caused each drop
   rather than being one number.
5. **Cross-BUILD reproducibility.** Two independent builds of the same input must
   produce the same initramfs, or the difference must be named. This also answers
   `sdboot-xbootldr`'s NEXT #2 — read `sdboot-xbootldr.md` before starting so you
   answer both with one experiment.
6. **VM boot proof** on a reflink copy of
   `/var/lab-scratch/sdboot-xbootldr/apexgrub.img`. A slim initramfs that does
   not boot is worse than a fat one. Reflink the image (`cp --reflink=auto`) —
   do not mutate the original, another unit's evidence depends on it.

Then evidence file + commit + push.

## THE TRAP THIS UNIT ALREADY WALKED INTO ONCE

Its own first attempt wrote a `Containerfile` assertion that **could not pass**
on the artefact the branch produced — the initramfs contains `cnic.ko.zst`,
`cxgb4.ko.zst` and `qed.ko.zst`, pulled in as SCSI-offload dependencies rather
than by the `network` dracut module, so `! grep -qE
'kernel/drivers/net/ethernet/'` FATALed. That defect family has cost this
program five days of image builds and there is now a checker gating it. Any new
assertion you add must be demonstrated BOTH ways — red on the bad input, green
on the good one — before it goes near a Containerfile.

Related and equally paid for: `errexit` does NOT apply to inverted commands, so
`! grep …` under `set -e` refuses nothing. 46 APEX build refusals failed nothing
for this reason. Use `if … FATAL … exit 1`.

## CONSTRAINTS I AM UNDER

- Never land on `roadmap/v2.2`. Never push `main`. Never open a PR.
- Headless only. Never a window on Andre's desktop. This is L16 work.
- `/var/lab-scratch`, never `/tmp` (15 GB tmpfs on 29 GB RAM).
- **Read `free -h` before every guest boot.** `sdboot-migrate-3` is a concurrent
  KVM lab unit on this machine; two labs once drove available memory to 1.3 GiB.
  Wait rather than start a guest when it is tight.
- podman/qemu in the FOREGROUND or under `systemd-run --user`, never `nohup &`.
- Push your branch and mark `## LANDABLE <sha>` on this card; the orchestrator lands it.

## NEXT

- Record that build 35624291221 executed the new stanza end to end (closing the
  predecessor's stated open risk), then start item 4: the attributed size matrix.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing

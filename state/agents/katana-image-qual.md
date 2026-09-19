# katana-image-qual — the two run-books that were waiting on an image

items: units `gaming-gpu` and `pkg-share`, both RE-OPENED 2026-09-19 (round 33)
repo: apex-os
worktree: **make your own** — `/var/tmp/apex-work/wt-katana-qual2`, off
`roadmap/v2.2`, branch `task/katana-image-qual`
evidence file: `ROADMAP/evidence/katana-image-qual-20260919.md` (new; the round-31
run is `ROADMAP/evidence/katana-qualification-20260919.md` and is already landed —
read it, do not overwrite it)

## Why this unit exists, and why it could not run until today

Two units closed on 2026-09-19 with the same sentence: **RE-OPEN after an image
build.** Their code is landed and headlessly verified; what is missing is one
run on a machine that actually carries it.

- `gaming-gpu` — merge `78f04717`. Run-book: **`docs/gaming-and-sessions.md`
  section 6, six blocks.**
- `pkg-share` — merge `5de97037`. Confirmation commands in the closure and
  repeated under NEXT below.

Both were blocked on katana, and katana was held by the TPM-clear agent. **It is
free now** (checked 2026-09-19 17:00 AWST: greetd on tty1, nobody logged in, no
`steam`/`gamescope`/`proton` process, `rpm-ostree status` idle).

## The one fact that decides the whole round

**Katana's booted image predates both fixes.** It is on
`ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2`,
digest `sha256:ba263890…`, version `apex (2026-09-18T14:33:05Z)` — commit
`266dcc57`, which is **131 commits behind** the integration tip. `git
merge-base --is-ancestor` says NO for both `78f04717` and `5de97037`.

So a run-book executed on katana *as it boots today* measures the OLD code and
produces evidence attributed to the wrong build. **Do not start the run-books on
the current deployment.** The rebase is step 1, not an optional tidy-up.

## The image you are waiting for

CI **35433705393**, build-image, dispatched by the orchestrator 2026-09-19
09:04:38Z on `roadmap/v2.2` @ `7f647470e222cfa23e0853cac45ef3f7e74c252e`. It
carries `78f04717` (gaming-gpu), `5de97037` (pkg-share) and `b0e34371` (p2-b
round 31). ~1 hour.

A non-main dispatch pushes **per-SHA tags only** — `PUBLISH` guard at
`build-image.yml:170` — so it moves nothing any machine tracks. The tag to wait
for is exactly:

    ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e

Poll for it with `skopeo inspect --no-tags docker://<that>` rather than by
reading the run's status: the tag is the thing you actually need, and run
35415266422 proves the tag can land even when the run later fails (it died in
the SBOM step on a GitHub runner shutdown, after core+base+image had all built —
that failure is infrastructure, not ours, and **the `build-verify` agent owns
that question, not you**). If the run fails before the tag appears, say so on
this card and stop rather than rebasing onto something older.

## NEXT

1. **Poll for the tag** above. While waiting, read
   `docs/gaming-and-sessions.md` section 6 end to end and
   `ROADMAP/evidence/katana-qualification-20260919.md`, and write your plan for
   blocks 6.1–6.6 onto this card. Do not fill the wait with unrelated work.
2. **Rebase katana onto that exact tag.** Read `rpm-ostree status` / `bootc
   status --json` FIRST and match the transport that is already in use
   (`ostree-unverified-registry:`), rather than assuming `bootc switch` defaults.
   Keep the old deployment as the rollback. Record the digest you landed on.
3. **`pkg-share`'s confirmation, first** — it gates one of gaming-gpu's rows.
   `sudo apex install steam` on katana, then:
   - `ls /usr/share/vulkan/icd.d/ | grep -c i686` — **was 0**, must now be
     non-zero. This is the gate on P1-038 row 6 (Steam Big Picture).
   - the section 3 shadow count — **was 177**. Use `LC_ALL=C` on every `sort`
     and `comm`; the round that found this got bitten by collation.
   - `grep 'multilib: carrying'` in the install log: the image-owned count
     should dominate the native-pass count.
4. **Then the gaming run-book, blocks 6.1–6.6.** Andre's words are the scope:
   *"test everything on the katana, everything daily, everything gaming, and
   also the gaming modes should properly boot and on the monitor and stuff."*
   Section 6.3's bwrap cause is **CLASSIFIED, NOT ATTRIBUTED** — the round-31
   qualification logged in via `systemd-run --property=PAMName=login` rather
   than greetd, and the session now logs `CapEff/CapPrm/CapAmb`
   unconditionally, so **a real greetd run answers it from the log alone.** Do
   the greetd login; that is the whole point of having the machine.
5. Write evidence as you go into
   `ROADMAP/evidence/katana-image-qual-20260919.md`, commit and push on
   `task/katana-image-qual`, and record each closed criterion with
   `ROADMAP/set-status.py <id> <status> --evidence "…"`. **`set-status.py`
   REPLACES evidence — it has no append flag.** Read the existing text and
   prepend to it, or you will erase the previous round's counts.

## Rules, and the three that are about this machine

- **Katana's NVMe device names are not stable** — `nvme0n1`/`nvme1n1` reorder
  across ordinary reboots, and the standing "never write to nvme0n1, that is the
  Windows disk" instruction protected the *wrong disk* on one boot of the TPM
  run. Identify disks by serial, PCI function, PARTUUID or label. Never by
  `/dev/nvme*n1`.
- **APEX's Boot0000 lives on the WINDOWS ESP.** Do not tidy EFI entries.
- Reboots are authorised. Taking katana's own screen is authorised — it IS the
  test. **Never open a window on Andre's L16 desktop**; drive katana over `ssh`.
- Never run `qs -p`. No polkit or keyring prompts — `sudo` or `--user`.
- Never `pkill apex-agentd`. Never push `main`; never open a PR.
- A backgrounded `podman`/long command is SIGTERMed — run foreground or under
  `systemd-run`.
- Per-agent scratch dir only: `/var/tmp/apex-work/scratch-katana-image-qual/`.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing: it is
  what a fresh agent is handed when you are killed without warning. Everything
  else can be re-derived from git; the next action cannot.

## DONE

- nothing yet.

## IN PROGRESS

- nothing yet.

## FOUND

- (orchestrator, round 33) katana's booted image is 131 commits behind the
  integration tip and predates both fixes this unit is meant to qualify. Any
  run-book executed before the rebase measures the wrong build.

## BLOCKED ON

- CI 35433705393 producing
  `ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`.

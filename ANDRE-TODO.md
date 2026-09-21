# The 36 partial tasks — what each actually needs from you

Written 2026-09-21. **128 tasks: 92 done, 36 partial, 0 untouched.**

Nothing here is unwritten code waiting on an agent. Every one of these 36 is
partial because it ran out of something that isn't effort — a device, a booted
machine, a decision only you can make, or a limit in somebody else's API.

Ordered by **what you'd have to do**, not by task number, so you can batch it.
Each entry says what's already done, what's missing, and what closes it.

Rough totals if you wanted all of it: **9 decisions you can make at a keyboard**,
**~7 items needing hardware you may not own**, **~8 needing a machine freed up
for an hour**, and **12 that are blocked on things neither of us can supply**
and are honestly finished as far as they can go.


---

## ✅ DONE 2026-09-21 — items 1, 2, 3 and 5 are closed, plus the phone

**1. avahi — DONE.** Unmasked, enabled and running on the L16. Verified it does
the thing it was blocking: one `avahi-browse` found a **FUJIFILM ApeosPrint
C325/328 dw** on your LAN over `_ipp`, `_ipps` and `_pdl-datastream`. P2-005's
printer and scanner rows are measurable on the L16 now.

**2. katana's TPM — DONE, and it needed nobody at the machine.** Its PPI reports
`5  4: User not required`, so the documented `echo 5 > ppi/request` + reboot path
applied. Checked the risk before acting: **no LUKS on katana and no BitLocker
anywhere** (every partition read for the `-FVE-FS-` signature), so it could not
cost access to Windows data. Proven by the owner primary key's name changing,
not by the reboot — `ppi/response` says Success before any reboot and means
nothing. **You re-enrol Windows Hello on katana next time you boot Windows.**
L-001's headline gap is closed; evidence in
`ROADMAP/evidence/katana-tpm-clear-20260921.md`.

**3. The Qt bug — NOT filed, deliberately, and this is the right outcome.**
Upstream already fixed it. quickshell #1006 was closed **completed** on 28 Aug by
commit `916a0dd`, "launch: avoid creating multiple QApplications" — which is the
second remedy this program's own analysis named. #1144 reported the same symptom
and the maintainer closed it as a **duplicate on 21 Sep**, hours before I looked.
A third report would have been a duplicate of a fixed bug, under your name.

The symptom persists on the L16 for a different reason: `v0.3.1` was tagged
21 Aug, the fix landed 28 Aug seven commits later, so **no release carries it**,
and your L16 boots a 5 Sep image with `quickshell-0.3.1`. `Containerfile.core`
on `roadmap/v2.2` already installs `quickshell-git` for exactly this reason. So
**P2-003 unblocks on an image update**, not on a bug report.

What is genuinely unfiled is the Qt defect underneath — any app that destroys a
`QCoreApplication` before creating a `QGuiApplication` permanently loses QtQuick
accessibility. That needs bugreports.qt.io and there is no Qt credential on this
machine. A ready-to-paste report, in your voice with no AI attribution and run
through the stop-slop checker, is at **`~/qt-bug-draft.md`**.

**5. The firewall scope questions — DECIDED on measurement.** LLMNR is no longer
accepted at all: `systemd-resolved` ships `LLMNR=resolve`, which asks and never
answers, so the old rule admitted packets to a service that discards them, on
every interface, for no function. mDNS is now scoped to its multicast groups
(`224.0.0.251`, `ff02::fb`), so discovery is untouched while a unicast probe from
an arbitrary host is refused. The live suite reads 34 passed / 0 failed with
three new cases asserting exactly that.

**The Pixel 10a — DONE.** `apex-remote-1.0.0.apk`, versionCode 1573, installed
and byte-identical on the device. **I caught something first:** the APK sitting
in scratch from 20 Sep was signed `CN=THROWAWAY DO NOT SHIP`. Installing that on
your daily would have meant no real release could ever update it, which is the
exact failure the signing key exists to prevent. Rebuilt from the current tip and
verified the signer is `9b2418f3…`, matching the published fingerprint.

**Still yours, and unchanged:** back up `/var/home/andre/apex-android-signing`.

---

## A. Decisions — no hardware, minutes each

These need a yes/no from you and nothing else. Highest value per minute.

### 1. Unmask `avahi` on the L16 — unblocks **P2-005**, helps **P2-007**
The L16 has avahi masked. Without it that machine can discover **neither a
printer nor a scanner**, so the printing/scanning criteria can't be measured
there at all. A mask lives in `/etc` and survives updates, so this won't fix
itself. There's already a real driverless eSCL Canon on your LAN (found from
katana), so the moment avahi is live this becomes measurable.
**You do:** decide, then `sudo systemctl unmask avahi-daemon.service`.

### 2. Say yes to clearing katana's TPM — unblocks **L-001**, then **L-002**, **L-003**
This is the single biggest unlock on the list — three tasks chain off it.
A live Windows install owns katana's TPM, so **clearing it costs you your
Windows Hello PIN** (you'd re-enrol it afterwards). It needs your yes, not a
machine — the two-command PPI procedure is already written down.
**You do:** say go, and be willing to re-set the Windows PIN.

### 3. File one upstream Qt bug — unblocks the last of **P2-003**
There's a known one-line `qtdeclarative` defect that means a screen reader gets
**one node** from the whole shell on a real machine. The fix is understood; it's
deliberately unfiled because posting on a public tracker under your name is your
call, not an agent's.
**You do:** say "file it" (and, if you want, under which account).

### 4. Approve the merge to `main` — **HOLD, do not do this yet**
Updated 2026-09-22. The image now builds green for the first time, so this
*looked* like the last gate. It is not, and the reason is worth knowing.

**Every green you and I have read on `roadmap/v2.2` was partly green on jobs
that never ran.** The `changes` path selector classifies a **push** against the
branch's *previous tip*, so only that push's paths count — while a **pull
request** classifies against `main`, i.e. the whole branch diff. A branch can
therefore be green on every push for weeks and red the moment it is proposed
for merge. `workflow_dispatch` ten lines away already uses the merge-base with
`main`, which is the correct behaviour, so the `push` case looks like an
oversight rather than a decision.

That has already produced one real red: **`Installer safety and UI` was failing
deterministically** on `roadmap/v2.2` and nobody could see it. I fixed and
landed that one (`6fa9ddbc`) — the engine was innocent; the suite was measuring
its own extraction.

**`installer` is one job of five.** The same blind spot applies to `rust`,
`engine` and `android`, and nobody has looked. A full-matrix run is going now
to report what surfaces, deliberately *without* fixing anything, so each red
can be attributed to its own fix.

**You do:** nothing yet. Wait for that report. Merging now would hit whatever
it finds, at the least convenient moment.

### 5. Two firewall scope questions — cosmetic, but they're yours
Currently LLMNR is accepted on **every interface**, and mDNS accepts **unicast
from any source**. Both are deliberate and both are a judgement about how open
you want a laptop on a café network to be.
**You do:** tell me tighten or leave.

---

## B. Hardware — things you may need to buy, borrow or dig out

### 6. A graphics tablet / stylus — closes the last leg of **P1-042**
Four of five legs are measured and landed (MIDI round-trip, pro-audio
scheduling, USB audio on katana's FIIO KA11 and Thronmax mic). The fifth is
"a stylus enumerates, is classified as a tablet, and its settings reach the
compositor". **`libwacom` finds no tablet on either machine.** This isn't a
wait-for-katana thing — somebody has to physically plug a tablet in.
**You supply:** any Wacom-class tablet, once, for ten minutes.

### 7. ~~A second physical monitor~~ — **WRONG, you already have one**
Corrected 2026-09-22. katana has **two connected displays at genuinely
different densities**: `eDP-1` 1920x1080 on 38×22 cm (~128 DPI) and
`HDMI-A-1` 1920x1080 on 54×30 cm (~90 DPI), the external one on the discrete
card. Same resolution, 1.4× apart in density — which is exactly P1-040's case
and why it was easy to miss.

So **P0-001's multi-monitor row, P1-038's display rows and
P1-040-per-output-scaling need nothing from you.** They need the new image and
a session, both of which I can arrange. Evidence:
`ROADMAP/evidence/katana-displays-20260922.md`.
**You supply:** nothing.

### 8. A VRR-capable display — one row of **P1-038**
Measured on katana: **no connector there exposes `vrr_capable` at all**, so VRR
cannot be tested on that machine under any circumstances.
**You supply:** a FreeSync/G-Sync monitor, or accept this row never closes.

### 9. A printer and a scanner, and a network share — **P2-005**, **P2-007**
The software all landed; what's missing is "a print job to a printer, a share
mounted from a server, a card in a slot". The Canon eSCL on your LAN covers
scanning once avahi is unmasked (item 1).
**You supply:** a real print job, and an SMB/NFS share to mount.

### 10. A dock or a hub — **P2-007**
Hotplug and peripheral maturity rows. A container has no udev or seat, so these
only ever moved from "not installed" to "installed and not running".
**You supply:** a dock, and someone at the machine to plug and unplug it.

### 11. A machine with a real discrete GPU free — **BASE-010**
The claim to prove is "the model was placed on the GPU and unloading released
its VRAM". The L16 is an **APU**, where VRAM is system RAM: `apexd` reports
`1024 MiB total, 806 used, 0 spendable`, so it correctly falls back to CPU and a
CPU placement would prove the bookkeeping, not the claim. **katana's RTX 3070 is
exactly the right machine.**
**You supply:** katana free, with `llama-server` installed and a model in the
store.

### 12. An Android tablet or a foldable — **P1-060**
The last production-quality criterion is large-screen and folding layouts.
Neither device exists here.
**You supply:** a tablet or foldable, or accept this stays partial.

---

## C. Machine time — you already own these, they just need to be free

### 13. Close the lid and watch — closes criterion 1 of **P1-063**
Genuinely this simple, and it has never been done. The inhibitor logic is built,
shipped and has been held live. What's needed: the lid closed **while the
inhibitor is held**, and the machine observed to stay up. It must not be claimed
until someone actually does it.
**You do:** close the lid with work running, come back, confirm it's still alive.

### 14. The gaming-mode switch — **BASE-009**
The live Desktop→Gaming switch and the controller-first path. The test
necessarily **kills its own session** (`loginctl terminate-user`), so it needs a
machine where no session matters and a human present to see the switch land.
You already said katana can be the automated test machine.
**You do:** free katana, be present for five minutes.

### 15. Install some ordinary apps, then a session pass — **P1-038**
18 matrix rows are blocked purely on software nobody installed: portals, Firefox
and Chromium screen sharing, OBS, Discord, Steam, gamescope, Wine/XWayland, VS
Code, JetBrains, LibreOffice, Blender, plus fractional scaling, rotation,
refresh rate, suspend/resume and dock hotplug. Can't be done headless, and can't
be done on the L16 while you're using it.
**You do:** install the apps on katana and give it an hour.

### 16. A fresh install onto a wiped disk — **P0-001**
"Fresh install" has never been attempted because it needs a wipe. Also
unanswered: **whether `systemd-sysext refresh` re-merging `/usr` disturbs a live
desktop** — the L16 will be the first machine ever to answer that, under your
working session.
**You do:** provide a disk you don't mind wiping.

### 17. A rollback reboot — **P0-001**
Currently unverifiable for a specific reason: katana's rollback slot **holds the
same digest as the booted one** (a hotfix unlock created a second deployment of
the same commit), so rebooting into it would prove nothing.
**You do:** free katana after it has taken two genuinely different images.

### 18. The five-run Secure Boot procedure — **L-001**
Written up in `docs/boot-v2.md`. Needs Secure Boot **on** and you at the machine.
One sub-case is permanently could-not-run: `fwupdmgr` offers no System Firmware
update for that board, so there's no capsule to apply — and flashing your laptop
uninvited isn't a substitute.
**You do:** follow the documented procedure once, with Secure Boot enabled.

### 19a. A machine that can load a sched-ext scheduler — **P1-043**
All three GPU vendors turned out to be reachable and were measured (the L16 is
AMD — Radeon 780M; katana carries both an Intel Iris Xe as `boot_vga=1` and the
NVIDIA card), so the old note claiming this "needs hardware nobody here has" was
wrong and has been corrected. What's genuinely left is one assertion flagged
**UNVERIFIED because no machine here can load a scheduler to look at it**. Worth
knowing for a different reason: the audit found `apex game status` reporting a
GPU as clock-locked when `nvidia-smi` had actually *refused* the lock — that's
fixed.
**You do:** free a machine where a sched-ext scheduler can actually be loaded —
which likely means the new image, since that's what carries the kernel with BTF
and sched-ext usable.

### 19b. A booted machine for the network rows — **P2-006**
Everything mechanical is done and landed, including the hotspot work (the
firewall now opens DNS/DHCP **scoped to the hotspot link**, not the port, so a
café Wi-Fi doesn't get a resolver). Three assertions that turned out to be
**unable to fail** were found and fixed this round. What remains is the same
shape as P2-005: a container has no D-Bus, systemd, NetworkManager, udev or
seat, so links, connectivity, paired devices and hotplug can only ever read
"installed and not running" until measured on a real booted machine.
**You do:** same session as items 1 and 9 — one booted machine, the six lines
only a booted machine can answer.

### 19. Re-enabling a TPM in firmware — one case of **L-001**
PPI Disable is reachable from the OS, but **re-enabling needs somebody physically
in the firmware setup screen**. Nothing can automate this.
**You do:** be at the machine during that run.

---

## D. Blocked on things neither of us can supply

These are finished as far as they honestly can be. Listed so you know they
aren't forgotten — but there is nothing to hand over.

**Cloudflare (P1-003, P1-004, P1-005, P1-007, P1-009, P1-013, P1-015)** — six of
these hit real limits in Cloudflare's own API rather than missing work. Token
refresh can't be expressed as a `mint` operation; `tail`'s result *is* a
credential (a `wss://` URL that authorises the stream) so it can't be brokered
the same way; R2's temp-access-credentials needs an S3 key the broker doesn't
hold and yields SigV4 credentials a REST-only transport can't spend; a preview
health check is an unauthenticated GET, which the host pin correctly forbids
spending an API credential on. **P1-007** additionally needs an *elevated
credential class* that doesn't exist yet in `apex-secret-core` — that's a real
design change (a field across 28 literals in 6 files), not a blocked measurement.

**P1-046 and P2-019 (update channels, fleet)** — both need a **mutable rollout
percentage**, and an OCI label is baked once per build so it has nowhere to
live. The design answer is a fleet endpoint that doesn't exist yet. Worth
knowing: `apex`, `daily`, `gaming-mesa` and `gaming-nvidia` are currently **four
aliases for one digest**, moved on every successful main build.

**P1-054, P1-056, P1-057, P1-058 (Android)** — protocol gaps, not app gaps.
`Profile` and `Projects` genuinely aren't in the wire protocol. Approve/deny
can't round-trip from a phone because `decide` is refused from any non-local
origin *by design*. "Checkpoint/undo shows consequences before execution" can't
be met from a phone against this protocol. And **there is no push transport
anywhere in APEX** — no FCM, Firebase, UnifiedPush or ntfy — so notifications
only work while a poll loop is alive.

**P1-055** — the terminal repaint loop keeps Compose's Recomposer permanently
busy, so the standard test harness never returns. Also: `TerminalView` is a bare
Canvas, so **a screen reader has nothing to announce on a terminal**. Recorded
honestly rather than asserted away.

**P2-016 (multi-user)** — the honest blocker is **fast user switching, which has
no surface at all**: zero hits for any switch-user string in either repo, and
greetd runs one session on one VT. Written into `docs/multi-user.md` as a gap.

**P2-004 (i18n)** — the catalogue ships and a German user sees German. What's
left is an image-cost trade: putting `qt6-linguist` in `core` is 4 MiB but
triggers a multi-gigabyte update for every machine. That's your call on cost,
not a build stage's.

**P2-017** — landed on `task/p2-f` (10 commits) and **not yet merged**. This one
is just a landing away; I'll pick it up.

**P2-008, P2-009, P2-018, P1-051, P0-001 (parts)** — each has one named flow
never demonstrated end to end, all needing a booted machine rather than new code.

**L-002, L-003** — chained behind item 2 (the TPM clear). L-003 additionally
carries a warning worth reading before it ever ships: on real Intel PTT,
`MAX_AUTH_FAIL` is 32 and **a successful authorisation does not clear the
counter**, so if "where safe" ever includes a PIN, 32 mistypes *over any span of
time* lock the TPM for up to a day — and on a machine whose lockout auth another
OS holds, APEX cannot reset it.

---

## If you only do three things

1. **Say yes to clearing katana's TPM** (item 2) — it's the only item that
   unblocks three others.
2. **Close the lid** (item 13) — thirty seconds, closes a criterion that has been
   open since 12 September.
3. **Unmask avahi** (item 1) — one command, and the Canon on your LAN makes
   scanning measurable immediately.

## Separately, and not on this list

- Back up `/var/home/andre/apex-android-signing` — all four files, two places,
  neither of them GitHub. If that directory is lost, **every install of the
  Android app in the world becomes permanently unupgradeable.**
- Plug the Pixel 10a in when convenient so the signed release APK can go on.

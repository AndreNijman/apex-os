# gaming-gpu — Gaming Mode on the right GPU, and the sessions katana broke
items: P1-043, P1-038 (Gaming Mode half: matrix rows 6 and 7)
repo: apex-os
worktree: /var/tmp/apex-work/wt-gaming-gpu
branch: task/gaming-gpu  (base f666f5c1, five commits, e56e88a8..4fc0ab06)

Cut from `roadmap/v2.2` at `f666f5c1`. **Pushed, not merged.** Re-checked
against `origin/roadmap/v2.2` at `b0e34371` after the tip moved:
`git merge-tree --write-tree` is clean and none of the 16 commits that landed
in between touches a file this branch touches.

Everything here comes from `ROADMAP/evidence/katana-qualification-20260919.md`.
Section numbers below are that file's. The narrative and the run-book live in
`docs/gaming-and-sessions.md` on the branch; this card is the part a stranger
needs that is not in the doc.

## NEXT

**The branch is done and green. What is left needs the machine.** Nothing on
it is waiting on an agent, and nothing below should be simulated.

1. **Run the katana checklist.** It is `docs/gaming-and-sessions.md` §6, six
   blocks, each with the exact commands and the expected output. Run it from a
   **greetd login**, not `systemd-run` — §4 explains why the qualification
   could not, and §6.3's capability question is only answerable on the real
   login path.
2. **Row 6 of the P1-038 matrix (Steam Big Picture) is BLOCKED and must not be
   retested yet.** `apex install steam` ships no `*_icd.i686.json` at all
   (§6.5), so Steam's 32-bit client has zero Vulkan ICDs. The fix is in
   `files/system/libexec/apex-pkg`, owned by unit **pkg-share** this round.
   Gate: `ls /usr/share/vulkan/icd.d/ | grep i686` must be non-empty. Until
   then a Big Picture failure says nothing about Gaming Mode.
3. **Land it.** No dependency on pkg-share for the landing itself — the two
   units touch disjoint files, and `apex-pkg` was never opened here.

## What changed, in one line each

| § | defect | state |
|---|---|---|
| 6.1 | gamescope opened the iGPU and the laptop panel | **fixed**, pending a katana confirm |
| 6.2 | `--rt` passed and silently dropped | **fixed** — the flag is no longer claimed; nothing grants the capability, on purpose |
| 6.3 | Steam never reached a UI inside gamescope | **classified, not fixed** — see below |
| 6 | the status block said "not measured" for a refusal | **fixed** |
| 7.2 | the VRR probe could never answer, and asked about every screen | **fixed** (found in review, not in the evidence) |
| 5.5 | Safe Graphics could not light a dGPU output | **fixed**, pending a katana confirm |
| 5.4 | the niri session ran two bars | **fixed**, verified against the real upstream config with the real `niri validate` |

## Five things that are not in the doc and will cost you time

1. **`test-apex-firstrun.sh` used to END ITSELF** at `python3 "$CHECK" … |
   head -12`. `set -euo pipefail`, the checker prints 13 lines, head took 12,
   python died of SIGPIPE, pipefail surfaced 141, errexit exited — before
   `bad` ran, before the summary, and before every section after it. Fixed on
   this branch with `awk 'NR <= 12'`.

   **The failure it was hiding is real but not ours and not CI's.** The repo's
   `files/desktop/labwc/rc.xml` has a `W-A-s` screen-reader bind that the
   *installed* `/usr/share/apex-shell` on the L16 (commit `9141ea7f`) does not
   know about. CI SKIPs that branch — neither `../apex-shell` nor
   `/usr/share/apex-shell` exists on a runner — and the image build asserts
   against the tree it is vendoring. A local run on the L16 will show
   `apex-shell-firstrun: 69 passed, 1 failed`. That one is it. It belongs to
   whoever owns apex-shell vendoring (see the memory note "The image vendored
   the wrong apex-shell").

2. **`task/p1-043-gpu-parity` is fully landed already.** It shows 8 commits in
   `git log roadmap/v2.2..`, which looks unlanded and is not — landings were
   cherry-picks back then, so the SHAs differ. Checked rather than assumed: the
   diff of `apexd/apexd-core/src/gpu.rs` between it and `roadmap/v2.2` is three
   lines, and `30-apex-gaming-rtprio.conf` is identical. The branch pointer can
   be deleted. Do not re-fold it.

3. **The selection rule is NOT "prefer the discrete GPU".** A discrete GPU with
   no connector attached is a session with no screen. The output decides the
   card. If you change it, the fixtures that hold it are
   `apexd/apexd-core/tests/gpu_parity.rs` — katana, katana unplugged, the L16,
   a dock, two externals, an all-AMD hybrid, a card with no PCI id, nothing
   connected, and a connector whose `status` is refused.

4. **A connector name is unique per CARD, not per machine.** Both fixture sets
   deliberately carry `card1-HDMI-A-1` (disconnected, iGPU) beside
   `card2-HDMI-A-1` (connected, dGPU). That is not padding: a name-only lookup
   sorts to card1 and answers about a port nobody is using. It shipped in one
   commit here and was caught in review of the next; keep the namesake.

5. **`apex gaming --gamescope-device-args` exits 1 on a PARTIAL answer** — a
   screen it could name but no PCI id for the card. The session uses the half
   it got and logs the gap. Do not "fix" that exit code to 0: other consumers
   need to know the answer was incomplete, and do not make the session discard
   partial args either — that trades one silent regression for another.

## §6.3, stated precisely, because it is the row most likely to be misread

Two independent causes were observed and neither is fixed here.

* **No 32-bit Vulkan** — pkg-share's, see NEXT item 2.
* **`bwrap: Unexpected capabilities but not setuid`** — bwrap refuses to start
  when a non-setuid process has a non-empty PERMITTED capability set. The
  qualification ran the session through `systemd-run --property=PAMName=login`
  rather than greetd (§4), and those two can have different capability shapes,
  so this may be a harness artefact. **It is not attributed either way.**
  `apex-gaming-session` now logs `CapEff/CapPrm/CapAmb` unconditionally and
  names the consequence when `CapPrm` is non-zero, so the next greetd run
  answers it from the session log alone.
* `Unable to open X11 display` is expected to vanish on its own: gamescope had
  already died on `card1` before Xwayland came up. Downstream of §6.1.

**The trap in this area:** granting CAP_SYS_NICE to fix §6.2 is the same act
that makes §6.3 worse. pam_cap would put capabilities into the permitted set of
every process in the session, Steam's bwrap included. `setcap` cannot work at
all — `/usr` is a read-only composefs and gamescope arrives through a sysext
overlay, so there is no writable inode for the xattr. If a future gamescope RPM
carries `cap_sys_nice=ep` itself, the session's `getcap` probe finds it and
`--rt` returns with no code change, and the bwrap interaction does not arise.

## How it was verified without hardware

* `apexd/apexd-core/tests/gpu_parity.rs` — 39 cases (15 new), sysfs fixtures.
* `tests/test-apex-gaming-session.sh` — **new**, 31 cases. RUNS the session
  script with a fake gamescope that records its argv, and the REAL `apex`
  binary through `APEX_ROOT` so the rule under test is the one that ships.
  Wired into `pr-validation.yml` and into the hard ShellCheck gate.
* `tests/test-apex-gaming.sh` — 131 (17 new).
* `tests/test-apex-safe-graphics.sh` — 99 (13 new), with a fake labwc.
* `tests/test-apex-firstrun.sh` — 69 (11 new), against niri's REAL
  `default-config.kdl` and the REAL `niri validate`. Only `validate`; never a
  bare `niri`, and no window was opened.
* Mutation-tested, restored byte-identical with `cp` and verified with `cmp`
  each time: reverting the device selection fails 4 rows; reverting the `--rt`
  rule fails 3; laundering EACCES into "disconnected" fails 1; dropping the
  Safe Graphics `WLR_DRM_DEVICES` fallback fails 2; resolving VRR by connector
  name fails 2.
* `cargo test` (all crates) and `cargo clippy --all-targets -- -D warnings`
  clean. Cargo root is `apexd/`, not the repo root.
* Smoke-run against the real `/sys` on the L16, no fixture: `1002:1900` matches
  `lspci -nn` (HawkPoint1) exactly, `adaptive sync: not published`, and
  `sudoers rule: unknown … run \`sudo apex gaming\`` — the §6 defect answering
  correctly on a live machine.

Katana was **not** touched. It was held by the TPM agent for the whole of this
round, and that agent has a second reason to be left alone: the machine
dual-boots Windows off a second NVMe.

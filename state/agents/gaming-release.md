# gaming-release — the two defects the Gaming Mode hardware run left behind

items: **P1-043** and the Gaming Mode half of **P1-038**, both still `partial`
repo: apex-os
worktree: `/var/tmp/apex-work/wt-gaming-release`, branch `task/gaming-release`
  (created 2026-09-19 22:30 AWST off `roadmap/v2.2` @ `de77ae5d`; five commits,
  `de77ae5d -> bfe5f5df`, **pushed, not merged**. `git merge-tree` against the
  newer tip `12e9d454` is clean.)
evidence file: `ROADMAP/evidence/gaming-release-20260919.md` (new, on the
  branch). The *qualification* record it answers is
  `ROADMAP/evidence/katana-image-qual-20260919.md` §3 — a different file, do
  not overwrite it.
predecessor card: `ROADMAP/state/agents/katana-image-qual.md` (its `## NEXT`
  says the run-book is finished; that is still true, and nothing here re-ran it)

## NEXT

**Land `task/gaming-release` (five commits) and then get an image built. Every
remaining question needs one.** Nothing on this branch can be confirmed on
katana until it is in an image: the release fix lives in `apexd` and the
mangoapp fix lives in `/usr/libexec/apex-gaming-session`.

After the image lands, run **`docs/gaming-and-sessions.md` §6.6 and §6.7** — they
were written for exactly this and give the commands. In one line each:

* **§6.6** arm a Gaming Mode session (helpers and the dead-man restore timer are
  in `katana-image-qual.md`, **arm the timer first**), check
  `apex game status` says `owner_pid : <the apex-gaming-session pid>` — if it
  says `0`, the image predates the watch and the row cannot pass — record
  `/sys/fs/cgroup/apex-game/cpuset.cpus` and `/sys/kernel/sched_ext/state`, then
  `sudo systemctl restart greetd` and read, within a few seconds:
  `active : false`, the `apex-game` cgroup **removed**,
  `sched_ext/state` → `disabled`, and apexd's own
  `the session owner is gone` line in `journalctl -u apexd`.
* **§6.7** the same session must log **zero** `Glfw Error` lines, add **zero**
  mangoapp core dumps, and be a normal-sized journal rather than 144 187 lines;
  plus `systemd-analyze cat-config systemd/coredump.conf` → `MaxUse=256M` and
  `KeepFree=2G`.

**The trap that will waste a run:** the CPU governor is **not** a discriminator
on katana. Its default AC tier is already `performance`, so
`scaling_governor`, `energy_performance_preference` and `apex tier` all read
`performance` before, during *and* after Gaming Mode — measured 2026-09-19
22:45 with game mode off. The readings that actually move are the cgroup,
sched-ext and `active`.

One thing the suite cannot reach and a machine can: arm a session with
`APEX_GAMING_EXPOSE_WAYLAND=0` and confirm the MangoHud overlay **actually
renders**. It never has on this system, so "the flag comes back" is all that is
proven today.

## DONE

- **bfe5f5df `docs(gaming)`** — apexd's unit hides nothing from the watch
  (no `ProtectProc=`), and what an apexd restart still loses.
- **fc0a29d5 `docs(apexd)`** — `StartOwnedBy` and `owner_pid` on the GameMode
  D-Bus surface, with why the polkit rule was not loosened instead.
- **a9d7ff73 `docs(gaming)`** — §5a (the release decision), §5b (the mangoapp
  A/B), run-book §6.6/§6.7, the two broken checks fixed, and the evidence file.
- **6bdae56b `fix(base)`** — `files/system/coredump/50-apex-coredump-limits.conf`
  → `/usr/lib/systemd/coredump.conf.d/`, `MaxUse=256M` `KeepFree=2G`, with a
  build assertion that resolves the same search path systemd-coredump resolves
  at runtime. Verified **both ways** against the real
  `ghcr.io/andrenijman/apex-os:base-d12d3450…` image before it was written.
- **b42e4cd2 `fix(gaming)`** — the release fix and the mangoapp fix.

### Defect 1 — Gaming Mode could not release itself

`apexd` now owns the lifetime. `GameMode.StartOwnedBy(pid)` records the owner
**and its `/proc` start time**; a 2 s watch beside the existing AC loop calls
the same idempotent `game_exit()` when the owner is gone and emits the same
`ActiveChanged` / `Status` / `TierChanged` set a D-Bus exit emits.
`apex game start --owner-pid N`; the session script passes `$$`.

Five decisions, all argued in `docs/gaming-and-sessions.md` §5a:

1. **Polkit was not loosened.** `allow_inactive` is *every* local session, so
   any unprivileged user could stop someone else's Gaming Mode — and a
   `SIGKILL`ed session runs no trap for a looser rule to permit anyway.
2. **PID + start time**, because PIDs are reused; `starttime` is read by
   splitting after the **last** `)`, because `comm` may contain spaces and
   parens.
3. **Unreadable `/proc` is a third answer**, never a release.
4. **The owner is watched, not pinned** — pinning it would put every Steam
   process on the p-cores, which nothing has measured.
5. **A session keeps its first owner**; an ownerless one can be adopted.

The `EXIT` trap survives as the second of two paths and **can no longer fail in
silence** — it prints polkit's own `AccessDenied` message and names apexd.

### Defect 2 — the mangoapp crash loop

Cause, diagnosed from the kept dump and then **proved**: the backtrace is two
frames, `XInternAtom` from `main`, i.e. `XInternAtom(NULL, …)`; the session log
shows `libdecor` (GLFW's *Wayland* backend) and four
`Glfw Error 65550: X11: Platform not initialized` per crash, which is
`glfwGetX11Display()` returning NULL off-platform.

Controlled headless A/B on katana, environments read out of
`/proc/<pid>/environ` rather than assumed:

| flags | mangoapp env | 22 s |
|---|---|---|
| `--mangoapp` | `DISPLAY=:0`, `GAMESCOPE_WAYLAND_DISPLAY` | same pid throughout, **0 restarts** |
| `--expose-wayland --mangoapp` | + `WAYLAND_DISPLAY` | **171 restarts** |

APEX passed both unconditionally, so the combination is ours.
`--expose-wayland` stays (§6.1/§6.3 were qualified *with* it, and the overlay
has never drawn a pixel); `--mangoapp` is **gated on it**, so
`APEX_GAMING_EXPOSE_WAYLAND=0` brings it back — which is what makes the gate
testable in both directions.

## FOUND

- **The 4.0 GB of core dumps was NOT a missing bound.** `coredump.conf(5)`:
  `MaxUse=` defaults to 10 % of the filesystem **capped at 4 GiB**. katana's
  `/var` is 954 GB, so the effective cap was 4 GiB and 4.0 GB is exactly it.
  systemd's vacuum was working the whole time; 4 GiB is simply the wrong size
  for an ostree machine. Do not write this up as "there was no limit".
- **gamescope's exit segfault is upstream and now attributed**, from one
  read-only `coredumpctl info 247577`:
  `CVulkanDevice::~CVulkanDevice()` under `__run_exit_handlers`, i.e. a
  static-destruction-order crash after `exit()`. Nothing in it is APEX
  configuration. Recorded, not chased.
- **`apex game …` must never reach the real bus from a test.**
  `tests/test-apex-gaming-session.sh`'s `apex` fake used to `exec` the real
  binary for every verb; with the trap and `game start` now exercised, that
  would have entered game mode on the machine running the suite. The fake now
  intercepts `game` and records its own `PPID`, which is also what lets the
  suite assert `--owner-pid` carries the script's pid rather than a literal.
- **`ulimit -c 0` really does keep a crash out of the dump store.** The A/B
  produced ~400 mangoapp segfaults on katana and every journal entry from them
  reads `Storage: none`; `/var/lib/systemd/coredump` is still 26 MB and `/var`
  still has 53 G free. Use it for any future crash-loop experiment there.
- **apexd can actually read `/proc`, checked rather than assumed.**
  `apexd.service` sets `ProtectHome=yes`, `RestrictRealtime=yes`,
  `MemoryDenyWriteExecute=yes` and explicitly `ProtectControlGroups=no`, and has
  **no `ProtectProc=`, no `PrivateUsers=` and no PID-namespace isolation** — so
  the watch sees every process and no unit edit was needed. If anyone hardens
  that unit later, `ProtectProc=` is the line that would silently break the
  release.
- **apexd restarting mid-session is a pre-existing gap, not made worse here.**
  `GameSession` lives in memory and dies with the daemon (stated in
  `apexd/src/game.rs`'s header, predates this work). A graceful stop already
  releases game mode; a crash with `Restart=on-failure` comes back with no
  session, so the cpuset, IRQ affinities and sched-ext stay applied with nothing
  recorded to undo them. The owner is one more field lost with the rest.
  Closing it means persisting the exit plan across restarts — a different
  decision about a different failure, deliberately not taken here.
- **`cargo test -p apex` alone fails 8 `handoff_packet` rows** with "apex-agentd
  is not built" — environmental, not a regression. `cargo build -p apex-agentd`
  first, or run `cargo test --locked` from `apexd/`.
- **`apex game status` moved from `tests/doc-verbs-undocumented` to documented.**
  `check-doc-verbs.sh` fails on a *stale* entry as well as a missing one, so
  documenting a verb means deleting its line from that file.
- Gates run green on the tip: apexd-core `gamemode` 29/0, apexd 15/0, apex
  656/0 (+ 8/0 handoff after building apex-agentd), `test-apex-gaming-session`
  46/0, clippy clean, shellcheck clean, `check-containerfile-assertions`
  195 checked / 0 failed / 0 inert, `check-doc-verbs` 0 stale,
  `check-shellcheck-coverage` 0 newly failing, `check-suites-run-in-ci` 0 unrun.
- Every new assertion was mutation-tested and each mutation is recorded against
  the row it turned red (evidence §1.4). Sources restored with plain `cp`,
  verified with `cmp`.

## Katana's state, checked rather than assumed at 22:45

Read-only all round. Nothing installed, no unit changed, no package touched,
greetd never touched, and the three kept mangoapp dumps are still there.

```
greetd                         active
apex game status               active : false
/sys/kernel/sched_ext/state    disabled
/sys/fs/cgroup/apex-game       absent
/var/lib/systemd/coredump      26M
df -h /var                     53G available
```

## BLOCKED ON

An image build. Nothing else.

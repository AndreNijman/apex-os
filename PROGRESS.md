# APEX-OS roadmap v2.2 — progress

Resume state for autonomous sessions. `roadmap.yaml` holds the per-task status;
this file holds the things the yaml cannot say: where the branches are, what is
running right now, and what the next session should pick up.

Updated: 2026-09-06 19:05 AWST

## How to resume

1. Read this file.
2. `git -C <repo> fetch origin && git log --oneline origin/roadmap/v2.2 -5` in both repos.
3. Read `roadmap.yaml`; anything not `done` is outstanding.
4. Continue from "Next up" below.

## Ground rules for this program

- **Integration branch, not main.** All work lands on `roadmap/v2.2` in both
  repos. `build-image.yml` fires on push to `main` for `Containerfile*`,
  `files/**`, `apexd/**`, `config/**`, `kernel/**` and `.github/**`, and
  `pr-validation.yml` fires on any PR to `main`. Both stay untouched until the
  final integration PR. Pushing `roadmap/v2.2` and task branches fires nothing.
- **Uncommitted work is snapshotted for you, every 3 minutes.**
  `~/.local/bin/apex-wip-snapshot`, on a systemd user timer with
  `Persistent=true` and `Linger=yes`, force-pushes every dirty worktree of both
  repos to `refs/wip/<worktree-name>` on origin. Recover with
  `git ls-remote origin 'refs/wip/*'` then `git fetch origin refs/wip/<name>`.
  It is safe under a live agent: it builds the commit through a throwaway
  `GIT_INDEX_FILE`, so the real index, HEAD, working tree and stash stack are
  never touched, and it skips worktrees mid-rebase. Push anyway — a snapshot is
  a net, not a substitute.
- **Katana runs the heavy work, when it is free.** Rust builds, image builds and
  test matrices go there over ssh, with scratch in `/var/tmp/apex-build/`. It is
  Andre's gaming machine too: check `pgrep -u andre -f "steam|proton"` first, and
  when he is playing, build on the L16 under `nice -n 15` instead.
- **Headless only.** No windows on Andre's desktop, no polkit or keyring
  prompts. QML verification runs under nested labwc or the repo `tests/`
  harness.
- **Worktree isolation** for every repo-writing agent.
- **No AI attribution** in any commit, PR or release.
- **`stop_slop` gates every task** that changes human-facing prose:
  `python3 ~/.claude/skills/stop-slop/scripts/slopcheck.py <files>`.
- **Honest statuses.** A task without evidence for its acceptance criteria is
  `partial` with a note saying what is missing. Never `done` on faith.

## The integration plan (work this out once, follow it at the end)

`Containerfile.base:454` has `ARG APEX_SHELL_REF=main` and clones apex-shell
with `git clone --depth 1 --branch "${APEX_SHELL_REF}"`, which accepts a branch
or a tag. So the whole integrated stack can be built and validated on katana
**before** anything reaches `main`:

```sh
podman build -f Containerfile.base --build-arg APEX_SHELL_REF=roadmap/v2.2 ...
```

That is how end-to-end validation happens without GitHub CI, which is what Andre
asked for.

**Merge order at the end is load-bearing.** `Containerfile.base` vendors
apex-shell from a remote ref, so apex-shell must land on `main` before or with
apex-os, or the OS image is built against a shell that does not have the changes
and the shell-side work is inert in the image. Order:

1. apex-shell `roadmap/v2.2` → `main`
2. apex-os `roadmap/v2.2` → `main`, which triggers `build-image.yml`

`main` requires a PR plus `PR validation` plus resolved threads, with Andre as
the only bypass. Merge commits are disabled, so rebase or squash.

## Branches

| Repo | Integration branch | Forked from |
|---|---|---|
| apex-os | `roadmap/v2.2` | `origin/main` @ 57f593a |
| apex-shell | `roadmap/v2.2` | `origin/main` @ 9141ea7 |

Worktrees now live on disk at `/var/tmp/apex-work/`, not in the session
scratchpad. The scratchpad is tmpfs: it is quota-limited, and it disappears when
the session ends, which took the integration worktree with it once already.
`git worktree prune` after that happens, then recreate.

- `/var/tmp/apex-work/int-os` → apex-os `roadmap/v2.2`
- `/var/tmp/apex-work/int-shell` → apex-shell `roadmap/v2.2`

## Machines

| Host | Booted digest | Role |
|---|---|---|
| L16 | `sha256:308127d9` | Andre's workstation. Light use only. |
| katana | `sha256:308127d9` | Build box **and** gaming machine. 20 cores, 62 GB RAM. Yield to a running game. |

Katana was pruned on 2026-09-06 (reclaimed 235.6 GB) and is booted on the
current image, so it doubles as P0-001 hardware evidence.

## Task counts at start

126 tasks: 42 P0, 62 P1, 19 P2, 3 Later.
Starting statuses: 95 todo, 19 verify_existing, 10 partial, 2 blocked.

## Dependency waves

Computed from `depends_on` in `roadmap.yaml`:

- Wave 0 — 56 tasks, no dependencies (all 18 BASE, most P0 UI, much of P1/P2).
- Wave 1 — 27 tasks.
- Wave 2 — 15 tasks.
- Wave 3 — 14 tasks (most of the Cloudflare capability sets).
- Waves 4-7 — 14 tasks (Cloudflare deploy, then the Android app chain).

## Status

### Done

- **Roadmap repair**: added the missing `P0-025` (Hyprland hyprlang → Lua migration)
  to `roadmap.yaml`. It was specified in `ROADMAP.md` section 50 and
  `CURRENT_ISSUES.md` UI-011 but absent from the canonical task file, so no agent
  reading the yaml would ever have done it. 127 tasks now; yaml parses, no
  duplicate ids, no dangling `depends_on`.

- **All 18 BASE tasks verified**, none left at `verify_existing`: 11 `done`,
  7 `partial`. Every verdict carries evidence from commands actually run on real
  hardware, and every `partial` names what is missing. Roughly 1,100 shell
  assertions and 1,080 Rust tests were run across the five agents.
- **`fix/gaming-signal-eacces`** pushed (2 commits, `ed5243c`): a defect class
  found in four places by two independent verifications.

### The EACCES sweep, finished

All ten catalogued sites addressed on `roadmap/v2.2` (**1273 tests**). Two were
already fixed by the first pass. Two turned out worse than catalogued:

- **`boot.rs`**: a whole-directory efivarfs refusal takes `LoaderInfo` down with
  `StubInfo`, so `detect_bootloader` falls back to the cmdline and answers
  `"grub"` — and the rescue-target route would then offer GRUB-menu
  instructions to someone on a systemd-boot and UKI machine. Not a wrong label,
  wrong recovery advice.
- **`apex-env`**: `create`'s overwrite guard used the same stat test as the
  reporting path, so an unreadable existing capsule record was **silently
  overwritten** rather than merely misreported.

It also corrected my own earlier commit. I had justified the fix partly on
`/sys/kernel/security` and `tpm0/` being "commonly mounted 0700"; they are 0755
on this machine, only the leaf file is `0440 root:tss`, so a bare stat would not
fail there under plain DAC. The code fixes stand — a sandboxed environment can
still refuse the stat — but the comments now say what was verified rather than
what was assumed.

One item was skipped for a good reason: `secret.rs` is deleted outright by
P0-002's branch, so fixing it would have been work thrown away.

Disclosed by the agent and checked by me: a few test runs went without
`XDG_STATE_HOME` re-exported. Andre's state is intact, the daemon is active, and
the only damaged files remain the three transcripts truncated at 10:18.

### The EACCES defect class, fixed

Four readers collapsed "permission denied" into "absent", and each then reported
the guess as a checked fact:

| Where | What the user saw |
|---|---|
| `apexd-core/src/gaming.rs` `file()` | `apex gaming` always warned the sudoers rule was missing, because `/etc/sudoers.d` is 0750 and the command runs unprivileged |
| same, `executable()` | a mode it could not read reported as a mode without the executable bit |
| `apex/src/recover.rs` `package_row` | `apex recover status` told every non-root user with packages installed that they had **none**, marked `Verified` |
| `files/system/libexec/apex-pkg` `cmd_list` | `apex pkg list` said the same |

The codebase already had the right pattern next door: `apex boot status` reads
the ESP, also 0700, and reports `entriesUnavailable` with a reason.
`Health::Unavailable` is documented as "never a synonym for fine". The tests
missed all four because their fixtures are owned by whoever runs the tests, so
nothing ever hit EACCES. New tests seal a directory to 0000, verify the seal
actually took (root walks through 0000), and assert both halves so a real
absence stays a measurement. Clippy clean, 1080 tests pass.

### Landed on `roadmap/v2.2`

**apex-os `583698b`** — 11 commits, 1158 tests, clippy clean.
**apex-shell `05d9412`** — 7 commits.

| What | Why it mattered |
|---|---|
| P0-020 | sliders no longer take the wheel; five controls audited and changed, five navigation ones kept |
| P0-021 | four of seven agent states rendered as the palette foreground |
| P0-022 | the Agents guide, with every CLI verb read out of the clap enums |
| P0-018 (half) | the confirmation dialog lived inside the window the apply destroyed |
| P0-004 | the six permission dimensions, plus `PR_SET_NO_NEW_PRIVS` on unconfined sessions |
| EACCES class | four readers that reported a refused stat as a checked absence |
| multilib | six defects that made `apex install steam` impossible |

### The multilib story, and what it left on katana

`apex install steam` had never worked. The guard asked `rpm -q <name>` with no
architecture, so on multilib it saw the x86_64 build and deleted every i686
library as "already provided"; `REFUSE_RE` refused the rest; `download_rpms`
never asked for the 32-bit closure; rpm's 32-bit content at `/lib` never
reaches a sysext, which merges `/usr` and `/opt` only; and `install` died on
fontconfig's dangling relative `/etc` symlinks.

Fixing those exposed a sixth: `extract_rpms` unpacked both architectures through
one `rpm -Uvh --replacefiles`, so the i686 `/usr/bin/fc-list` shadowed the
image's. **53 of katana's `/usr/bin` binaries are currently 32-bit**, including
`ldconfig`, `pipewire`, `dconf`, `gio`, `gsettings` and every `fc-*` tool. That
is why `fc-list` returns zero fonts and Steam draws no text.

Audio survives only because pipewire started at 09:09, before the extension
merged at 12:41. A pipewire restart, a logout or a suspend will exec the 32-bit
binary. **Do not suspend katana until it is repaired.**

Repair script ready at `scratchpad/katana-repair.sh`. It unmerges the extension,
which removes Steam, so it waits until nothing is being played.

### Recovering from the 12:50 usage limit

Eight agents were killed mid-work. The push-after-every-commit rule held for
most of them:

| Task | Outcome |
|---|---|
| P0-004, P0-018, P0-022, EACCES sweep | pushed before dying, all replayed and landed |
| P0-017, P0-002 | **lost entirely**, never pushed. Re-dispatched. |
| greeter names, disposable aliases | lost, re-dispatch pending |

The lesson is in every new agent prompt: push after each self-contained piece,
not once at the end.

### P1-044, done by the orchestrator between waves

**APEX had no firewall.** `firewalld` was not installed, `nftables` shipped with
an empty ruleset, and nothing in any Containerfile referenced either — so every
port a program bound was reachable from the network. Seventeen were listening on
the L16, including a model server on 11434 and a Steam remote-play socket.

Landed on `roadmap/v2.2` as `432b83b`: a default-drop policy, an `apex firewall`
helper that manages exceptions by service name into named nftables sets rather
than by rewriting the ruleset, a unit that loads before `network-pre.target` so
there is no unfiltered window at boot, and 25 assertions. `ssh` is open on
purpose: `apex host run` and remote agents are ssh, and a policy without it
strands the user on the machine they were driving from.

Criterion 2 (exceptions in the APEX UI) still wants a shell page. The CLI it
would drive exists.

### Integration order is now the constraint, not capacity

Four tasks have landed on `roadmap/v2.2` since the last wave and two needed a
rebase handed back to their own agent. That is working as intended — the agent
that wrote a change resolves its conflict — but it means branches should be
folded in as they report rather than batched, and a branch left unmerged for an
hour costs a rebase.

**P0-002 is the interesting one.** Criterion 2 says the normal API cannot return
a raw secret. It landed that as a type-system property rather than a policy:
`SecretValue` implements neither `Serialize` nor `Deserialize`, and `Response`
derives `Serialize`, so **a reply carrying a credential does not compile**.
Three tests keep it true as the API grows, including one that pins the response
variant list so a new variant fails a test rather than quietly opening a path.

The end-to-end proof is a real git smart-HTTP server on loopback that 401s
without auth: it asserts the server *received* the credential while no reply,
no output and no audit line contained it. That is the difference between a
brokered operation and an exit-zero no-op.

It also moved grants and the audit trail behind the root boundary. Both were
user-writable before, so a session could grant itself a capability and then
rewrite its own audit.

### The 17:00 usage limit, and why it cost nothing

Six agents were killed. Four had pushed and lost nothing. Two — P0-009 and
P0-011 — had not, and I reported their work as lost.

It was not. The snapshot timer, installed immediately afterwards, found both
worktrees still on disk and rescued **2691 insertions** from P0-009 (including
an entire untracked 2368-line `profile.rs`) and **930** from P0-011. Both are
now on `refs/wip/` and both tasks have been redispatched to build on them.

The first version of that tool would have rescued almost none of it. It used
`git stash create --include-untracked`, which snapshots tracked modifications
only: it accepts the flag, ignores it, and prints nothing at all when the only
change is a new file. Measured on the same worktrees, stash captured 323
insertions where the temp-index version captured 2691. **Test a safety net
against the case it exists for.**

### Landed while the limit was in force

A parallel OpenCode session repaired Steam on katana and left a handoff in
`CLAUDE.md`. It found two more defects downstream of the multilib work, both now
fixed on `roadmap/v2.2`:

- **The rebuild deleted 26 image-owned `/etc` files**, among them
  `/etc/fonts/fonts.conf`, `/etc/ld.so.conf`, `/etc/krb5.conf`,
  `/etc/pki/tls/openssl.cnf` and both nssdb databases. A 32-bit rpm carries the
  same `/etc` paths as its 64-bit sibling byte for byte, so a run that installed
  the i686 set recorded them as ours, the next run stopped shipping them, and
  the has-the-user-touched-it test passed *because* the file was pristine. The
  removal pass now asks the rpmdb who owns a path and fails closed.
- **A 32-bit application twin cannot share a transaction.** steam ships
  1.0.0.85 for i686 and 1.0.0.87 for x86_64; rpm refuses the older as a
  downgrade and `--oldpackage` then hits file conflicts. A non-host-arch rpm
  whose name appears at a different EVR for the host arch is now dropped as an
  application fork rather than a multilib library.

### `producer | grep -qE` under `pipefail` is a flake, and eleven suites have it

`grep -q` exits the moment it matches. The producer then takes SIGPIPE and exits
141, and `set -o pipefail` reports that as a failure. Measured rather than
argued: **16 spurious failures in 200 runs** on a 415-line input, and **0 in 400
runs** with process substitution instead.

Two suites are fixed. Eleven more carry the pattern, listed on
`state/agents/p1-020.md`. This is very likely the mechanism behind several
"pre-existing flakes" that got shrugged at over the last two days.

### Two gates are red on the tip, and neither failure is a real defect

Confirmed by stashing every local change, so they belong to the tip:

- **"Agent Center invariants"** greps for `\bsudo\b` and `control\.sock` in
  `src/services/agents/*.qml`. P0-022's `AgentHelpContent.qml` quotes both **as
  help text**. The invariant was written when nothing in that directory could
  mention them; a help panel that explains what `sudo` means now does.
- **`run-agent-center-smoke.sh` counts any ERROR**, and the restarted shell logs
  `pipewire … Errno: 112` under a private `XDG_RUNTIME_DIR` — an artefact of the
  harness, not the shell.

A red gate everyone knows to ignore is worse than no gate, because the next real
failure hides behind it. Both are dispatched.

### A fixed scratch path two daemons fight over

`/tmp/apex-agent/<id>` is a fixed path and session ids are per-daemon, so two
daemons belonging to one user collide and one deletes the other's scratch under
a running session — the symptom is `bwrap: Can't open source`. Now movable by
`$APEX_AGENT_SCRATCH`. This affects containers and a second user session, not
only tests.

### A security hole that only existed once two branches met

P1-018 shipped `apex secret grant <svc> <op> --everywhere`, gated in the daemon
on `OperationSpec::names_nothing()` — no resource **and** no parameters. The
reasoning was sound on the tree it was written against: an operation that names
nothing can only ever reach the endpoint pinned when its credential was stored,
so granting it everywhere widens *where* it may be asked for and not *what* it
reaches. `git.push` resolves a remote out of the caller's repository, so it was
correctly refused.

P1-002 landed the first counterexample. **`cloudflare.account.read` declares no
resource and no params, and still resolves its account from the project's own
`apex.toml`** — bound it is `GET /accounts/{id}`, unbound it is `GET /accounts`.
So `*` would have let an agent in a project the owner never approved read that
project's account.

Neither branch was wrong on its own. The hole existed only in their
composition, and **P1-018's own test caught it during integration** — which is
the argument for writing the test that states the intent rather than the one
that matches the code. The integrator fixed P1-018 against its stated intent
rather than relaxing the test: an explicit allow-list shared by the gate and the
CLI hint, so the two cannot disagree, plus a runtime test. Mutation pair 109/2
against 111/0.

That fix is fail-closed but not final: the registry test will pass silently for
the next such operation. The real fix is `same_everywhere` as a declared fact on
`OperationSpec`, and it is queued.

### The clippy wrapper paid for itself on its first real run

P1-002's author recorded that clippy could not be run. On the first run over
that code it found two genuine lints — `type_complexity` and `useless_format` —
both fixed, bytes identical before and after, 13/0 either way.

### An integrator pushed a tip that did not compile, and said so

`cherry-pick -n` then `git apply` without `--index` then `commit -C` commits the
**index**, so the fix was not in the commit — while cargo, reading the working
tree, reported 1750 passing. The tip was wrong for about four minutes. Caught on
one line of `git status`, re-picked with the fix staged, force-pushed, and a
dirty-tree guard (exit 3) added to the runner scripts and mutation-proved, plus
a per-commit build loop over all fifteen commits.

Worth recording for the same reason the other self-reports are: the failure mode
is that **the working tree and the commit disagree, and every test reads the
working tree.**

### It was twelve runners, not four, and the fix had two bugs of its own

The survey said four test runners reached Andre's session. A proper sweep found
**nineteen scripts that launch a graphical client, twelve of them on the
inherited `WAYLAND_DISPLAY`.** All twelve now bring their own headless wlroots
compositor, private runtime dir and private HOME; the other seven were already
headless inline and pass the guard on their own merits.

Verifying by *running* rather than parsing found two defects in the fix itself:

- **`set -e` exempts only the LEFT operand of `&&`.** `[ -n "$pid" ] && kill -9
  "$pid"` aborted the cleanup function whenever the pid had already been reaped
  — the normal case, three tenths of a second after the first kill. The function
  never reached its `rm -rf` or its `return 0`, and an EXIT trap ending on a
  failure hands that status to the script. **A suite printing "5 passed, 0
  failed" exited 1**, and every run leaked its sandbox; five were sitting in
  `/tmp`.
- The guard itself read `command -p X` as a lookup. `-v` and `-V` print a path;
  **`-p` runs the command.** The author's own mutants missed it because, in their
  words, they were written from the same wrong model as the code. That is the
  sharpest statement of the mutation-testing limit anyone has made in this
  program: a mutant tests whether the code matches your model, not whether your
  model is right.

The guard has four rules because a hand mutation defeated the first three:
`export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-1}"` placed after
`headless_begin` satisfies A, B and C and still lands the run on the desk. And
it was unreachable until wired into `ci.yml`, which lists every check by name —
a guard nothing runs is a comment.

**Fact worth keeping:** Hyprland comes up nested inside a *headless* labwc and
`configerrors`, `reload` and `version` all answer — but the host labwc must not
be pixman. Aquamarine wants a dmabuf, and the only symptom is a silent
`CBackend::create()` failure.

### Five firewall defects that only a real machine could show

The policy had been read, parsed and asserted for a day. Loading it into a
kernel on katana found five things reading never would:

- **SELinux.** `/usr/sbin/nft` is `iptables_exec_t`, so systemd transitions it
  to `iptables_t`, which cannot read `var_run_t` — a policy staged under `/run`
  failed to load as root. The shipped `/usr/share` path works for the mirror
  reason, `usr_t` being readable by the confined `nft`. Nobody had exercised
  either.
- **katana's sshd banned the driver.** `Match exec` runs `~/.ssh/lan-up` on
  every invocation, multiplexing does not skip it, each one is a bare
  unauthenticated connect to port 22, and `PerSourcePenalties` locked the driver
  out for about 50 seconds **while the policy was loaded** — which is
  indistinguishable from the firewall stranding its operator. Measured: six
  calls through the config open seven such connections; six over the control
  socket open none.
- **`%h` in the ControlPath expands differently with and without the config**, so
  every multiplexed call silently failed. And "no table is loaded" is a non-zero
  exit, so the absence of a connection was being read as a fact about the target.
- `--target` was an ssh destination and a TCP address at once, so the suite could
  not run against any machine reached by an alias — which is every machine it is
  for.
- The shell page rendered one reassuring sentence for an empty exception list.
  Three machines produce that list and it is true of one: on katana, whose image
  predates `apex firewall`, the page told the user their ports were reachable
  only locally, three lines under its own headline saying nothing was filtering
  them.

The dead-man's switch was proven to **fire**, not merely to be scheduled, and
`established,related` was shown doing the work rather than assumed: port 27036
answered from the L16 before the load and timed out after it, while the ssh that
loaded the policy kept carrying commands and a brand-new ssh still got in on the
same address. One mutation was deliberately **not** run — of the ssh accept rule
against katana — because that is an outage on Andre's machine and the namespace
suite already measured that half. Refusing a test is a legitimate result when you
say which one and why.

### The clippy wrapper I wrote had the defect it was written to prevent

`tests/run-clippy.sh` could not run on katana at all. Its `podman run` had no
`--network=host`, `docker.io/library/rust:1` does not ship clippy but downloads
it, and the container therefore had no DNS. The `>/dev/null 2>&1` on
`rustup component add` then threw away `Temporary failure in name resolution`
and printed "could not add the clippy component" — which reads like a broken
image rather than a network problem.

That is **a failure to look reported as an absence**, the exact defect class
this codebase already swept for in about fourteen places, written into the fix
for a different instance of it. It passed on the L16 only because the L16's
podman has working DNS, so the wrapper looked correct on the one machine it was
tested on. Found by the P1-018 agent when it tried to use it on the other.

Corrected on `task/p1-018-mcp-auth` at `16fc8ae`, and the integration agent was
told to land that version rather than the one I pushed.

### A test count read through a pipe is not a test count

`cargo test --locked 2>&1 | grep -E '^test result' | tail -12` reported **810
passed, 0 failed, exit 0** on a tree whose baseline is 1512. Both halves of that
are wrong in the same way: the exit code belongs to `tail`, not to cargo, and
the log holds only what `grep` let through, so a run that stopped early looks
identical to one that finished.

Under a shell without `pipefail`, **every "N tests passed" measured through a
pipe is unverified**, and this program has taken a lot of those numbers on
trust. Set `set -o pipefail`, or redirect to a file and grep the file afterwards
— which also leaves the failures readable instead of filtered away.

Same family as the three earlier ones: `git stash create --include-untracked`
accepting a flag it ignores, a suite reporting phantom passes with blank counts,
and an integration test skipping itself in CI while reporting success. **A check
that cannot fail is not a check**, and the cheapest way to find out which kind
you have is to break the thing it guards and watch.

### The 01:11 limit — what the resume system was built for, measured

All six agents died together at 01:11 on the same session limit, resetting at
04:10. Both resume crons fired. The recovery, in full:

1. One `resume.sh -f`. It named every agent, said each was dead by how long, and
   printed each one's `NEXT` line.
2. One card read (`integrate-2.md`, ~2 KB) to find out what the integration
   agent had left undone: apex-shell finished and pushed at `0fd12ee`, apex-os
   P0-005 finished and pushed at `5ae4350`, six P1-030 commits cherry-picked
   locally and unpushed.
3. Five fresh agents dispatched, each pointed at its own card rather than handed
   a transcript.

Compare with 2026-09-06, when four agents were revived by replaying four
transcripts of 2.1–2.4 MB and it cost about 10% of a usage window. Nothing was
lost either time; what changed is the price of picking the work back up.

Two details worth keeping:

- **`p1-048` had 14 uncommitted files** when it died. They were in
  `refs/wip/wt-p1-048-` and still in the worktree. The replacement agent was
  told to look at them before anything else, because a card describes intent and
  the tree holds the work.
- **The cards were 191–208 minutes old** — written before the limit, not after —
  which is exactly the property the "write it as you go, never at the end" rule
  exists to produce. A card written at the end would not have existed.

### A flag the script never had erased a third of the roadmap's evidence

`set-status.py` takes evidence **positionally**. Some orchestrator ran it as
`set-status.py <id> <status> --evidence "<text>"`, and `sys.argv[3]` is then the
string `--evidence`; the real text sat in `argv[4]` and was never read. The
script did what it promised — it wrote the evidence it was given, re-parsed the
file, and reported success — so nothing looked wrong at any point.

Found on 2026-09-07: **33 of 127 tasks held the literal string `--evidence`**
where their acceptance record belonged. Every one of them had reported cleanly.

It is the same shape as the other findings on this page. A check that cannot
fail is not a check, and a tool that cannot tell a flag from a value will
cheerfully record the flag. Three things came out of it:

1. `set-status.py` accepts `--evidence TEXT` and `--evidence=TEXT` now, and
   **refuses** to store anything that starts with `--`, or an empty string. Both
   spellings work, so an orchestrator reading either the docstring or a stale
   prompt gets the same result.
2. Eight were recovered exactly, from `refs/wip/roadmap-state` — the snapshot
   timer's 62 commits are a real history, not just a backup of the latest state.
   That is worth knowing the next time something is lost.
3. Twenty-four were reconstructed from `state/agents/*.md`, which is what the
   cards are for: they are written by the agent doing the work, contemporaneously,
   and they held the test counts and mutation pairs the evidence field had lost.
   Each reconstructed line says so and names the card it came from.

**`P0-024` is the one that could not be recovered.** No snapshot ever held its
evidence and no card covers it, so its line records what is verifiable today and
says plainly that the acceptance record is gone. Anyone re-qualifying P0-024
should re-derive its criteria rather than trust that line.

### A stale queue offers finished work, and `resume.sh` was offering it

The READY list was computed only from each unit's `after` field, so a unit stayed
on offer no matter what had happened to it. On 2026-09-07 it was still offering
`p1-018` and `p1-020` — both landed and `done` — and it listed every unit already
in a dispatched agent's hands. Fixed in `resume.sh`, both derived rather than
maintained by hand:

- **IN HAND** — a unit an agent owns is not offered. The agent's slug and the
  queue id may differ (`followups-integrate-3` was dispatched as
  `followups-int3`), so an agent may name its unit with a `queue_id` field.
- **COMPLETE** — a unit whose every roadmap item is `done` is not offered. Read
  from `roadmap.yaml`, so it cannot go stale.

One more trap, in `dispatch.json` rather than the queue: `resume.sh` does
`os.path.isdir(worktree)` and `git rev-list origin/<branch>..HEAD`, so both
fields must hold **one** value. An agent working two repos writes the second in
`second_worktree`/`second_branch`. Writing "a and b" there makes the next report
say "no worktree" for a worktree that is perfectly fine — an absence that is
really a parse failure, which is the fourth time this program has hit that shape.

### The landed-check was wrong, and it cost an agent run

`ROADMAP/state/unlanded.py` sampled the lines a commit added and asked whether
they were on the integration tip. That reads "landed in August, then four lines
rewritten in September for a better reason" as "still unlanded", and on that
basis this file claimed three apex-shell branches held 17 commits of orphaned
work. They did not. All three were squash-merged as PRs #5, #6 and #7, and
`git diff <branch-tip> <squash>` is empty for each — byte-identical trees. The
merge-bases were the tell: each branch forked from the *previous* branch's
squash commit.

The check now runs an exact test first, before any heuristic: **if any commit on
the integration branch has the same content as the branch tip across the files
that branch touched, the branch landed**, whatever its patch-ids say. Bounded by
the branch's own footprint rather than by history, so it is fast. It labels
those branches "squash-merged, safe to delete", and it immediately caught a
fourth — `task/p0-016-agent-settings` — that the old check would also have sent
someone after. The per-commit heuristic still runs for what survives, and the
report now says in the output that it is a heuristic and gives the command that
settles it.

Fixed in the shipped skill too (`~/.claude/skills/resume-guard/`), because the
same wrong check would have misled every project that installed it.

Fixing it introduced a second bug of the classic shape: the script's argument
list changed and its caller did not, so `resume.sh` passed a repository path
where an integration ref belonged. It did not crash. It printed a blank tip and
**"nothing — every task branch's content is on the integration tip"** for a
repository it had never looked at, which is the most dangerous sentence a status
page can contain. The script now refuses a bad invocation by name, and
`resume.sh` prints a warning where the section would have been rather than
letting a missing section read as a clean one.

The run was not wasted. Inverting the question — *did the last 82 commits break
any of that August work?* — found a defect nobody was looking for: Caffeine
gated the logind idle inhibitor on `Compositor.isLabwc`, on the belief that the
Wayland surface inhibitor covered Hyprland. Hyprland ignores idle inhibitors on
layer-shell surfaces, and the bar is one. **Caffeine did nothing at all on the
primary compositor**, while the labwc work aimed the one working mechanism at
the compositor that did not need it.

### Four shipped test runners open windows on Andre's desktop

`run-service-tier-test.sh`, `run-popup-smoke.sh`, `run-compositor-facade-test.sh`
and `run-nested-labwc.sh` all use the inherited `WAYLAND_DISPLAY` with no
headless backend — the last one says "nested inside the current Wayland session"
in its own first line — and `service-tier-test.qml` instantiates PopupWindows.
The constraint Andre complained about is violated by the repository's own test
scripts, not only by careless agents. `run-nav-geometry-test.sh` is the model to
copy.

### Nobody could have been running clippy, and three reports said they were

`clippy` is installed on neither the L16 nor katana, and neither machine has
`rustup`, so `cargo clippy` exits with "no such command". Checked on both, after
the P1-002 agent said so. Every unqualified "clippy clean" in this file before
2026-09-07 is therefore unverified as written.

Two agents did run it honestly, in a container — `docker.io/library/rust:1` plus
`rustup component add clippy` — and that is now the only accepted method, wrapped
as `apex-os/tests/run-clippy.sh [<ref>]` so nobody has to rediscover it. What is
**not** accepted is `RUSTFLAGS="-D warnings" cargo build --all-targets` reported
as clippy: it is a strictly weaker check that misses every clippy-specific lint,
and one report substituted it without saying so until asked.

The result itself holds, which is worth saying plainly rather than leaving the
correction sounding worse than it is: `9a24d2b` was run through the container and
came back **exit 0, zero warnings across the workspace**. What was wrong was the
evidence, not the code.

### Findings against P1-001 that its own evidence could not have seen

The first provider written on top of the framework found two gaps, and both are
worth more than the provider:

- **`Bound` carries no provider payload**, so `bind` cannot hand what it resolved
  to `perform`. Cloudflare works around it by re-resolving and refusing on
  mismatch. **git does not** — which means git's host pin, the invariant P1-001's
  evidence called "an invariant for every provider written later", is a TOCTOU
  rather than a guarantee.
- **`mint` cannot reach a companion service**, which is why Cloudflare's OAuth
  refresh could not be expressed as `mint`. P1-011 (temporary task credentials)
  will hit the same wall.

Also found and still live on the integration tip when reported: a refusal path
out of a credential use that scrubs nothing.

### The resume system, rebuilt (23:20–23:50)

Andre: *"stop losing work when we hit usage limits"* and then, after watching
the recovery, *"it cost 10% of my usage just for you to bring back the agents,
that is not good at all."* He was right. The snapshot timer had protected every
file, but reviving four agents meant replaying four transcripts of 2.1–2.4 MB
through the model, and working out the state beforehand took fifteen git
commands. Both halves are now shell work.

- **`ROADMAP/resume.sh`** prints the whole program's state on one page for no
  model usage: roadmap counts, every agent with whether it is alive and what its
  next action was, branches carrying unlanded work, the timer's health, and the
  ready queue. The cron prompts are now four sentences that point at it.
- **`ROADMAP/state/`** holds `dispatch.json` (who owns what), `queue.json` (28
  ordered dispatch units covering all 83 remaining items, with `after` and
  `conflicts` per unit) and `agents/<slug>.md`, one card per agent, **written by
  that agent as it works**. `NEXT` is the load-bearing field: everything else can
  be re-derived from git, the next action cannot.
- **A dead agent is never revived by messaging it.** A fresh agent gets the
  card. 2 KB against 2.4 MB.
- **`apex-roadmap-resume.timer`** replaces the in-session cron, which died with
  the session and so did nothing about a shutdown. `Persistent=true`, so a
  firing missed while the machine was off happens once on the next boot. It
  always refreshes the report for free and continues the run only while
  `state/AUTORESUME` exists.
- The snapshot timer now also pushes `ROADMAP/` itself to
  `refs/wip/roadmap-state` on apex-os, so the plan survives losing the disk.
  Recovery proven, not assumed.

**It launched a second orchestrator on its first firing**, alongside this live
one. Two orchestrators dispatching from the same queue would have fought over
the same branches. It now refuses to start while the orchestrator's own process
is alive or any agent has written output in the last twenty minutes — both arms
checked against a live and a dead pid.

Packaged as a skill for every project: **`~/.claude/skills/resume-guard/`**.
`install.sh <project> [--auto-resume]` sets it up; `selftest.sh` proves the net
catches an untracked file, leaves HEAD and the stash alone, and can recover the
plan from the remote. 8/0, and mutation-proved twice: a snapshot that only takes
tracked files goes to 6/1, one that commits in the real tree goes to 5/3.

### The 19:53 usage limit, and the four agents it stopped

All four in-flight agents died at 19:53 on the same session limit. The
`task-notification` for each said `failed`, and a later reminder said each was
still running with a plausible progress string — both cannot be true, and the
tie-breaker was `stat` on their output files: last write 19:53:38–19:54:00, so
they were dead and the "still running" reminders were stale. **Check the mtime,
not the reminder.**

Nothing was lost. Every worktree was intact, and the two with uncommitted edits
still had them (`wt-p0-005`: main.rs + registry.rs; `wt-p0-018b`: six files).
All four were resumed with `SendMessage` to their original task-id, which
restores the agent's own transcript — so each picked up mid-verification rather
than re-deriving its approach. That is the cheap recovery path and it should be
the first thing tried after any limit; fresh dispatch throws the context away.

### `fix/locked-hint` was never pushed, and P0-015 needs it

P0-015's evidence said the work was "dispatched as fix/locked-hint in
apex-shell". No such branch existed on the remote — the agent died before it
pushed. The only copy was `refs/wip/wt-lockhint-`, written by the snapshot
timer, and its worktree was already gone from disk. Recovered at `87c84f3`:
`src/services/system/LockedHintService.qml` (170 lines), a `qmldir` entry and
ten lines of `Lockscreen.qml` wiring, now pushed as `fix/locked-hint`. Without
the snapshot timer this work would simply not exist. That is the first time the
net has actually caught something.

### A landed-check that is worth trusting

`git merge-base --is-ancestor` is useless here, because the integration flow
rebases task branches onto `roadmap/v2.2` — every landed branch reads as
unmerged. `git cherry origin/roadmap/v2.2 <branch>` compares patch-ids and is
close, but a commit whose conflicts were resolved during the rebase also reads
as unlanded. Both false alarms fired today: `task/p0-022-agents-help` and
`task/p0-004-policy-split` each showed a full commit unlanded, and their content
was on the tip. The check that settles it is a two-dot diff scoped to the
commit's own files:

```
git diff origin/roadmap/v2.2 origin/<branch> -- $(git show --name-only --format= origin/<branch>)
```

Empty means identical. Three-dot (`A...B`) does not answer this question — it
shows everything the branch changed since the merge base, landed or not.

### Genuinely orphaned work found by the sweep

Three apex-shell branches from 21–23 August were never merged to `main` and are
not in `roadmap/v2.2`: `fix/labwc-desktop-parity` (10 commits, a compact app
dock, VPN/caffeine indicator truthfulness, pointer input on the power menu),
`fix/popup-first-open-and-media-keys` (5, first-open popup delivery, notification
toasts, CTRL+SUPER media keys) and `fix/screenshot-off-hyprland` (2, screenshot
on labwc and niri). They predate the roadmap program and branched around PR #4,
since when P0-016..025 have rewritten most of Bar, status, popups and labwc.
They must be ported **per commit, asking whether each bug still exists on the
tip**, not rebased as a block.

### In flight — six agents

| Task | Branch | State |
|---|---|---|
| P0-003 credentials into the broker | `task/p0-003-cred-broker` | resumed 23:24, was writing vault notes |
| P0-005/006/007 native mode, break-glass, session grants | `task/p0-005-permission-modes` | resumed 23:24, owed a SIGTERM-escalation test |
| P0-018/021/025 shell-side finishing | `task/p0-018-display-finish` | resumed 23:24, owed a mutant on the "exact" arm |
| P1-030..034 fish, nushell, tmux, zellij, layouts | `task/p1-030-shell-integrations` | resumed 23:24, owed a review pass |
| P1-045/046/047 state migration, channels, trust status | `task/p1-045-updates-trust` | dispatched 23:26 |
| P1-002/003/004 Cloudflare provider, identity, Workers | `task/p1-002-cloudflare` | dispatched 23:26 |

Six is the ceiling. Four agents plus the orchestrator hit the session limit
twice today; eight does not get more work done per limit-window, because the cap
is usage rather than wall-clock — it only gets more agents killed mid-task, and
the snapshot timer saves files, not plans.

### Held back deliberately

- **P1-040 mixed-DPI shell scaling** — same files as the in-flight P0-018 shell
  agent. Dispatch after it lands.
- **P1-039/041/042 Night Light, colour management, creator input** — also touch
  AppearancePage and the display pages. Same reason.
- **P0-014/015** — depend on P0-007, in flight. P0-015 additionally needs
  `fix/locked-hint` integrated first.
- **P0-001** hardware qualification — gated on a real reboot of the L16 onto the
  integrated image, which is the last step before main.

### Autonomous resume crons

| id | schedule | role |
|---|---|---|
| `a64bb3d6` | `49 12,17,22,3,8 * * *` | primary five-hourly resume |
| `35dc07f3` | `23 1,6,11,16,21 * * *` | offset backstop, halves worst-case dead time after a limit resets |

Both are session-only and expire after 7 days. They read this file first.

### Where the whole roadmap stands

| | done | partial | todo | blocked |
|---|---|---|---|---|
| P0 (25) | 15 | 8 | 2 | — |
| P1 (62) | 1 | 7 | 54 | — |
| P2 (19) | — | — | 19 | — |
| BASE (18) | 10 | 8 | — | — |
| Later (3) | — | 1 | — | 2 |

28 of 127 done. P1 and P2 are the bulk of what remains, 73 of the 99
outstanding. The P1 blocks worth dispatching as units: the Cloudflare suite
(16 tasks, all gated on P1-001, which is now done), MCP auth and sandboxing
(P1-018/019), agent UI and notifications (P1-020..029), desktop and creator
(P1-038..043), remote agents (P1-050..052) and the Android chain (P1-053..060,
eight tasks and the largest single piece of work in the roadmap).

### Constraints that changed

- **Katana is off-limits while Andre games on it.** Everything builds on the
  L16 under `nice -n 15`. Load there is ~0.5, so it copes.
- **Never run `qs -p <path>`.** It launches a shell instance rather than
  parse-checking one. I did this by accident; it exited on its own and his
  session was untouched, but the next one might not.
- **Always set `XDG_STATE_HOME` to a temp dir before a test run.** A suite
  writing to the real one truncated three of his PTY transcripts today.
- **Build output goes on disk, not in the scratchpad.** `/tmp` is tmpfs, sized
  at 50% of RAM, so its ceiling is about 15 GB on this machine and no quota can
  raise it past RAM plus swap. A cargo `target/` is 2-3 GB and six agents
  building at once filled it. Export
  `CARGO_TARGET_DIR=/var/tmp/apex-build-cache/<task>` — `/var` is a 1.5 TB disk
  with 1.2 TB free — and delete `target/` from a worktree as soon as its agent
  reports.

  The per-user quota was raised from 12 GB to the full 15 GB on 2026-09-06.
  Diagnose with `quota -s`, never `df`: when this filled, `df` still read 63%
  while writes were already returning `EDQUOT`, and the symptom was that `echo
  hello` failed while `true` succeeded, because the tool captures output to a
  file it could no longer write.

## Log

- **2026-09-06 09:20** — Roadmap read, 126 tasks parsed, dependency waves built.
- **2026-09-06 09:35** — Katana pruned (235.6 GB reclaimed), confirmed booted on
  308127d9. It doubles as P0-001 hardware evidence.
- **2026-09-06 09:50** — `roadmap/v2.2` created and pushed in both repos.
- **2026-09-06 10:05** — `P0-025` added to `roadmap.yaml`; the file was missing a
  task its own companion document called a release blocker.
- **2026-09-06 10:10** — Wave 0 dispatched: 5 BASE verification agents, 5 P0 UI
  implementation agents.
- **2026-09-07 10:05** — Round 4 dispatched. All six of the 05:05–06:00 agents
  were dead (session ended, not failure); none was revived by message. Fresh
  agents carry the cards for `p0-014`, `p1-023`, `p2-010` and `p1-039`;
  `integrate-4` lands seven finished branches; `followups-int3` takes the two
  security holes integration round three routed onward. `p1-050` and `p1-035`
  are held under the ceiling with their finished work pushed — see
  `state/dispatch.json:_deliberately_held`, which exists so the next resume does
  not read the hold as an oversight.
- **2026-09-07 10:20** — Roadmap evidence repaired: 33 tasks whose evidence was
  the literal string `--evidence`, 8 recovered from the snapshot ref and 24
  reconstructed from the cards. `set-status.py` now refuses flag-shaped
  evidence. `resume.sh` stopped offering work that is finished or already in an
  agent's hands. Counts moved 45/25/55/2 → 46/30/49/2 done/partial/todo/blocked,
  entirely from recording work that was already pushed.

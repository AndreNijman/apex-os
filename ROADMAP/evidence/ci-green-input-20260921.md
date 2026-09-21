# ci-green-input — one regression and two flakes in `Package engine`

**2026-09-21/22, round 40, unit `ci-green-input`, branch `task/ci-green-input`
at `cd3c06b6` / `762c5204` / `9c389baf`.**

`PR validation` has been red on `roadmap/v2.2` since well before this unit was
dispatched. Per `ROADMAP/ANDRE-TODO.md` A.4 the only thing left before the
merge to `main` is qualification, and merging an integration branch whose own
validation is red is what forty rounds have not done.

## What was actually red, as against what the brief said

The round-39 brief named `test-apex-input.sh` and `test-mux-layouts.sh` and
called the second one a runner-environment defect. The round-40 continuation
named `input-settings` and `secret-broker`. Neither list is the whole picture.
Measured over the **eight most recent `roadmap/v2.2` runs whose `Package
engine` job actually ran** — the `success` runs in between are docs-only
commits where the path selector skipped the job, 3-4 minutes against 13m54s:

| suite | red | shape |
| --- | --- | --- |
| `apex-input` | **8 / 8** | 93 passed, 7 failed, 8 skipped — every run |
| `mux-layouts` | **4 / 8** | 44 passed, 3 failed vs 47 passed, 0 failed |
| `secret-broker` | **2 / 8** | 83 passed, 1 failed vs 93 passed, 0 failed |

So one is a regression and two are flakes, and they must not be treated alike.

**The flakes are flakes, checked and not assumed.** Across
`1156daa4 69253336 4e7aa3fa 11c45d36 10b2f449 9eef37a2 89036ec7 44c9a5cb
e02561fb` — reds and greens interleaved on a branch that only moves forward —

```
tests/test-secret-broker.sh   blob 8668547a35   at every one
apexd/                        tree fd247b5193   at every one
tests/test-mux-layouts.sh     blob d8658e648e   at every one
files/system/libexec/apex-mux blob afe43acead   at every one
```

Identical bytes, different outcomes. There is nothing to bisect.
(`git rev-parse <ref>:<path>` must be run from `bash`; zsh's `:s` history
modifier eats the path and the empty result reads as a real answer.)

## 1. `apex-input` — a real regression, and one cause behind seven failures

The CI log names it on the line above the first FAIL, and nothing asserted on
it:

```
/tmp/tmp.i2Ei0idF5c/niri-include-block.sh: line 30: NIRI_BIN: unbound variable
FAIL  the generated input config is included
FAIL  the generated keybind config is included
FAIL  exactly six lines were added (8 -> 8)
FAIL  a backup was taken before the edit
FAIL  re-running adds nothing
FAIL  it refuses and says so
FAIL  it restores and says so
```

`(8 -> 8)` — and `(635 -> 635)` on the L16 against a real niri default config —
says the append never happened.

The suite drives the SHIPPED provisioner instead of restating it: it extracts
`/^    # ── 6a\./,/^    fi$/` out of `files/system/libexec/apex-shell-firstrun`
and sources that block under `set -euo pipefail`.

* `2605db27` (2026-09-04) put the `NIRI_BIN=` resolution **inside** block 6a on
  purpose. Its message: *"defined inside 6a so the block stays self-contained
  for the suite that extracts and runs it."*
* `37497975` (2026-09-19, "Safe Graphics… niri ran two bars") added block
  **6a-pre**, which disables niri's stock `spawn-at-startup "waybar"` and needs
  the same binary, and moved the resolution up to share it.

That is correct for the provisioner — at runtime `NIRI_BIN` is assigned at the
top of step 6's if-body and is in scope at every use, so **the shipped script
is not broken and is not touched by this unit** — and it silently moved the
line out of the extracted range. The block then died on its first
`"${NIRI_BIN}" validate`, before the append.

The proof that this is the single cause, rather than seven bugs: the one branch
that still PASSED is *"an include is never written for a file that is not
there"* — the only branch that returns before touching `${NIRI_BIN}`.

**Fixed in the suite** (`cd3c06b6`). The harness sources the shipped resolution
line rather than restating the resolution, and three assertions replace a
convention that was invisible and got broken:

* the provisioner resolves `NIRI_BIN` exactly once
* `NIRI_BIN` is resolved **before its first use** — the runtime invariant, of
  which "inside block 6a" was only ever a proxy
* the block ran with everything it needs — asserted on the block's own output,
  FIRST, so the next variable that moves out of range costs one named line
  rather than seven downstream reds

A fourth assertion was passing for the wrong reason and is closed: *"a setting
changed in Settings reaches niri through the include"* checked `niri validate`
plus a grep of the **generated** file, neither of which needs the include to
exist. It passed on every run of the broken tree.

**Mutation-tested, not assumed** (L16, niri 26.04, suite run ALONE):

| tree | result |
| --- | --- |
| `roadmap/v2.2` as found | 101 passed, **8 failed**, 0 skipped |
| with `cd3c06b6` | **112 passed, 0 failed, 0 skipped** |
| `NIRI_BIN=` line deleted | 100 passed, **12 failed** — all three new assertions, and the strengthened one flips to FAIL |
| `NIRI_BIN=` moved after its first use | 111 passed, **1 failed** — exactly the ordering assertion |

## 2. `secret-broker` — the gate blamed a sysctl the same job had cleared

One assertion, twice in eight runs (`35616795800`, `35625128495`), identical
both times:

```
PASS  a confined session started (id 1)
      | --- can the session read the credential file directly? ---
FAIL  the session's script actually ran
      the sandbox did not come up, so nothing below was tested
      (a "uid map: Permission denied" here means unprivileged
       user namespaces are blocked — see the CI sysctl)
```

**That diagnosis is false, and the same job's own log refutes it.** The
`Install bubblewrap` step printed

```
bubblewrap 0.9.0
kernel.apparmor_restrict_unprivileged_userns = 0
bubblewrap works: a confined session can be built
dev.tty.legacy_tiocsti = 0
```

the session started, and the **first line of `inside.sh` reached the
transcript**. The sandbox came up. The hint was printed unconditionally, so it
was never evidence of anything.

What the two reds do agree on is the shape: **exactly one transcript line, the
full 25 s (100 × 0.25 s), nothing further.** `Session::write_log` in
`apexd/apex-agentd/src/registry.rs` holds `log: Option<File>` — unbuffered,
`write_all` straight to the fd — so that one line is all the session ever
produced. `echo; cat; echo` contains nothing that blocks. The state that fits,
and that the old loop had no way to see, is a session that **went away** after
its first command. The loop asked one question (`does the transcript say
DONE`), so it spent 25 s on a corpse and then guessed.

Reproduction was attempted and failed, which is itself the finding: run ALONE
on the L16 it is `93 passed, 0 failed`, and again under `CPUQuota=20%`. CPU
starvation is not the mechanism. The runner is a second environment (bwrap
0.9.0 on Ubuntu 24.04 against 0.12.0 on Fedora 43) and the mechanism is not
named here.

**What `762c5204` does about it.** The wait ends on one of three things and
says which — `DONE`, the session **left** (stop waiting at once and report its
exit status, the answer the old loop threw away), or the deadline. Either
failure now prints the transcript's byte count, where it stops, `apex agent
status <id>`, the stderr of `apex agent run`, and the tail of the runtime's own
log. The uid-map hint survives only behind a check that `run.err` actually says
it.

The ceiling goes 25 s → 90 s, and that is defensible **only** because "gone" no
longer waits: a dead session now fails in well under a second, so the longer
ceiling is spent exclusively on a session that is alive. It is not claimed to
fix the flake. It is what makes the next red answerable.

**Mutation-tested, because a diagnostic nobody has seen fire is not one.** With
`exit 7` spliced into `inside.sh` before its `echo "DONE"` — a session that dies
after doing its work, which is the state the CI reds are consistent with — the
gate now reports:

```
FAIL  the session's script actually ran
      nothing below was tested. the wait ended because: the session left before
      printing DONE (exited 7)
      waited 0s; the transcript is 1086 bytes and stops at:
        | apex: brokered git.fetch against https://127.0.0.1 exited 128
      apex agent status 1:
        state        failed
        command      /bin/sh /tmp/tmp.3njbO0xeh2/demo/inside.sh
        pid          979987
        outcome      exited 7
      stderr of `apex agent run`:
      the agent runtime's own log, last 20 lines:
        apex-agentd: listening on …/control.sock (6 adapters)
```

`waited 0s` against the old 25 s, and the exit status named. The false uid-map
hint did not print, because `run.err` did not say it.

## 3. `mux-layouts` — a drop with its evidence on `/dev/null`

```
apex-mux: the zellij layout never became a tab in session 'apex-demo-zellij'
FAIL  build creates the zellij session
FAIL  the layout landed as a tab zellij can describe
FAIL  the layout landed exactly once (0 apex tabs)
```

The cause is the drop `zellij_build`'s own comment already documents and
measures on zellij 0.45.1: for a window after a session is created, the server
answers `dump-layout`, accepts a tab request, returns 0 — and discards it. That
comment also records that the two environments **disagree about which of the
two send forms is the reliable one**, which is why the function alternates them.

The problem is that all four reds said exactly one thing. Both sends had
stdout AND stderr on `/dev/null`, so the single fact that would settle the
question — which form was tried, whether it exited 0, what it printed — was
being discarded twelve times a run, on a machine nobody can log in to.

**`9c389baf`** keeps each send's form, exit code and stderr, and on failure
prints them, `zellij list-sessions`, and the **tab names** the session does
have (names rather than the whole `dump-layout`, which is ~300 lines of swap
layouts with the answer buried in it). Attempts go 6 → 12, for the reason
measured in that same comment — a discarded request is cured by an identical
one a moment later — and because run `35624291380` exhausted all six. The
ordinary case lands on the first send in under 300 ms and pays nothing.

Verified by **mutation**, not by reading: with `zellij_landed` forced false,
`apex-mux build zellij` fails with 32 legible lines naming all twelve sends,
the session list, and the thirteen tabs that did exist.

### 3b. A third flake, found on the L16 and not in CI

`tests/test-mux-layouts.sh` line 128 was

```sh
"$MUX" backends | grep -qE '^(tmux|zellij)$'
```

under that file's own `set -uo pipefail` (line 24). `apex-mux backends` is a
**loop of printfs in another process**; `grep -q` exits on the first line it
matches, the second printf takes SIGPIPE, and pipefail turns the match into
141 — a pass reported as a failure. **Measured at 2 in 60** runs of that exact
pipeline on the L16, and it is what made the assertion red in the first local
run of this unit. `grep -c` reads to EOF, so there is no early exit to race
with.

## Measured results

Every suite run ALONE, because this repo's suites interfere in a sequential
loop.

| suite | before | after |
| --- | --- | --- |
| `test-apex-input.sh` | 101 passed, 8 failed | **112 passed, 0 failed, 0 skipped** |
| `test-secret-broker.sh` | 93 passed, 0 failed (green here) | **93 passed, 0 failed** |
| `test-mux-layouts.sh` | 46 passed, 1 failed (the SIGPIPE race) | **47 passed, 0 failed, 0 skipped** |

## CI

Four `workflow_dispatch` runs on `task/ci-green-input`, because a 25-50% flake
can go green once on nothing:

<!-- CI-RESULTS -->

## What is NOT claimed

* The mechanism behind the `secret-broker` stall is **not** named. What is
  claimed is that it is a flake on identical bytes, that the previously printed
  cause was false, and that the gate now records enough to name it next time.
* The `mux-layouts` drop is a zellij 0.45.1 behaviour this repository has
  already measured and cannot fix from here. What is claimed is that a fifth
  red will say which send form failed and what it printed.
* Nothing is skipped. No assertion in any of the three suites was weakened;
  four were added and one was strengthened.

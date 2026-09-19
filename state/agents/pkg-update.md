# pkg-update — a fix baked into an image now reaches a machine that updates

item: P0-001 (queue unit `pkg-update`, items `["P0-001"]`)
repo: apex-os
worktree: `/var/tmp/apex-work/wt-pkg-update`, branch `task/pkg-update`
  (cut 2026-09-19 off `roadmap/v2.2` @ `f8b8184d`, pushed, **not landed**)
commits: `178cc10c` fix, `627315a9` suite + CI, `b512cf12` the bump,
  `6f28505b` docs
evidence read: `ROADMAP/evidence/katana-image-qual-20260919.md` §0.2 and §1.2
  (landed as merge `c13ac17a`). Nothing was added to that file — it is the
  hardware record of another unit's run and this unit touched no machine.
scratch: `/tmp/claude-1000/.../scratchpad/pkg-update/` (session-local, gone)

## What was wrong

`rebuild_extension`'s short-circuit compared the resolved rpm set and
`os_version_id` and never `pkg_compat_level`. `cmd_rebuild --if-needed`
compared the level and never the set. Neither guard alone can see an APEX image
build: `os_version()` is `VERSION_ID` out of `/usr/lib/os-release` and reads
`43` before and after, and the resolved set comes from Fedora's repositories,
which know nothing about what APEX baked. So a machine took the image carrying
the pkg-share fix, `apex-sysext-rebuild.service` started and finished in the
same second reporting success, and re-merged a byte-identical old extension.

## What was done, and why in that order

1. `178cc10c` — the decision moved into `ext_up_to_date <state> <new_set>` and
   gained the level comparison. The state path is an argument, not `$STATE`,
   because `PKG_ROOT` is a `readonly` `/var/lib` constant and a test otherwise
   cannot reach the decision without root.
2. `627315a9` — `tests/test-apex-pkg-update.sh`, wired into the `engine` job of
   `pr-validation.yml` with an assertion floor and a step that fails if podman
   is absent, on any SKIP, and on a missing summary line.
3. `b512cf12` — `PKG_COMPAT_LEVEL` 2 → 3, **separately**, because a bump
   without step 1 is worse than no change: `--if-needed` announces a rebuild,
   re-downloads every rpm, stops at "already up to date" on the unchanged set,
   and `write_state` stamps the new level anyway. Marker consumed, nothing
   built, every later boot a match.
4. `6f28505b` — `docs/packages.md` said the level triggers a rebuild, which was
   not true until step 1, and gave a reader no way to check. It now names the
   two commands that separate a real rebuild from a unit that finished in the
   second it started.

## Why the suite is shaped the way it is

"The extension rebuilt after a bump" is worthless as an assertion: it passes
against the **broken** engine on any machine whose repositories have drifted,
which is exactly how katana's rebuild eventually happened (§2.1 — "luck, not
design"). So every case **holds the resolved rpm set constant** and varies only
the level, and leg B prints the stored set against the computed one so that is
measured rather than claimed.

Leg A (no root, no network, no container) calls the real `ext_up_to_date`
against state files that differ in one field each, with a control that a
matching state **is** up to date so no "must rebuild" can pass vacuously. Leg B
runs `apex-sysext-rebuild.service`'s exact `ExecStart` — `cmd_rebuild
--if-needed` — inside `podman run --rm` as root, with only the download and the
build stubbed; `state.json`, the requested list, the merge marker, the payload
sha256 and `write_state` are all real, because "the marker was consumed without
a rebuild" is a claim about what `write_state` put on disk.

Both legs carry a mutant that deletes the level clause from the live function
body and **requires the defect back**. The container one reproduces §0.2 in one
row: `announced=yes uptodate=yes extracted=no payload_changed=no`, new level
stamped.

Measured: 17 passed, 0 failed, 0 skipped. Mutation-tested against the engine
file itself, not only in-suite — deleting the clause gives 12/4, and deleting
it **and** bumping to 3 (the remedy that looks like a fix) is also 12/4.
Restored with plain `cp`, verified byte-identical with `cmp`. Neighbours on the
same tree: `test-apex-pkg.sh` 82/0, `test-apex-multilib.sh` 3/0,
`test-apex-multilib-extract.sh` 11/0, `test-apex-resolve.sh` 91/0.
`check-suites-run-in-ci.sh`, `check-shellcheck-coverage.sh` (170 scripts, 0
failing, and clean under shellcheck **0.9.0** in a container — the version the
runner has, which disagrees with a Fedora workstation's 0.11.0),
`check-doc-verbs.sh` and `check-containerfile-assertions.sh` (195 checked, 0
failed) all pass.

## NEXT

**There is no more code on this unit.** Two things are left and the first is
not an agent's judgement call:

1. **Land `task/pkg-update` by MERGE, not rebase.** Round 10 changed the
   mechanism; a cherry-pick of this branch conflicts. Four commits, three files
   (`files/system/libexec/apex-pkg`, `tests/test-apex-pkg-update.sh`,
   `.github/workflows/pr-validation.yml`) plus `docs/packages.md`. Nothing else
   this round owns `apex-pkg`, so a merge into `roadmap/v2.2` should be clean.
2. **One machine must confirm it, after the branch reaches an image build.**
   Nothing below can be done in a container and nothing below was done here.

### The hardware checklist, in the order a machine answers it

On a machine that already carries an extension, after it takes an image built
from a tip carrying `b512cf12`:

```
journalctl -u apex-sysext-rebuild -b
    -> "extension compatibility changed (OS 43, level 2 -> OS 43, level 3) — rebuilding"
    -> a duration in MINUTES, not "started and finished" in the same second
sudo sha256sum /var/lib/extensions/apex-user.raw
    -> not 99749240e8762e2b4eaf0c3840a07beba249961e1cad86f798805c22c58282ed
sudo jq -r .pkg_compat_level /var/lib/apex/pkg/state.json          -> 3
ls /usr/share/vulkan/icd.d/ | grep -c i686                         -> 13, not 0
comm -12 <extension file list> <rpm -qal>                          -> empty
```

and the offline case, which the service is written for: boot with no network,
the unit warns and leaves `state.json` at 2, and the next boot or the next
`apex update`/`apex install` retries.

**The one thing no container can answer** is whether `systemd-sysext refresh`
re-merging `/usr` under a live desktop disturbs running applications. That is
the engine's own stated worry — "re-merge /usr underneath a running desktop for
no reason" is the comment on the short-circuit this unit just made stricter —
and it is now reachable on a normal update rather than only on a Fedora bump.

### Two things a stranger reading a journal will misread

* **Katana will rebuild once redundantly.** Its current extension was already
  built by the *fixed* engine (§2.1 shows the `multilib: carrying` line) but
  stamped level 2, because that engine's constant was still 2. A
  `level 2 -> level 3 — rebuilding` line there is the mechanism working, not
  the defect returning.
* **The L16 is not exempt.** It carries a `clippy` extension, so it rebuilds on
  the first boot after this lands, under Andre's live desktop. `Nice=10` and
  `IOSchedulingClass=idle` on the unit bound the cost; the `/usr` re-merge does
  not care about either.

### Deliberately NOT done, and it is a real gap

* **No `apex pkg rebuild --force`.** The evidence offered it as the alternative
  fix ("or give bare `rebuild` an explicit force path"); it is not the fix, it
  needs a documented verb that `check-doc-verbs.sh` will demand, and there is
  now no way for a user or a support session to force a rebuild when the three
  signals agree but the extension is wrong anyway. Small, self-contained, worth
  a follow-up.
* **The level is a MANUAL marker.** `PKG_COMPAT_LEVEL` only moves when a human
  remembers to move it. pkg-share's own rule asks the running image's rpmdb
  what it owns, so strictly the extension is stale after *every* image change
  that adds or removes a package. Recording an image identity in `state.json`
  (the bootc digest, or the ostree deployment checksum) is the automatic
  version and would make this class of defect impossible — at the cost of a
  multi-minute rebuild and a `/usr` re-merge on every OS update. That is a
  product decision about update cost (`docs/update-cost.md` records the same
  tension for the AI apps), not something to decide inside this unit.

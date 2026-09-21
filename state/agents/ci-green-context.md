# ci-green-context — `**/target/` deletes real source from the build context

items: none (no roadmap id — the `base` tier cannot build, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-context (create it)
branch: task/ci-green-context, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-context

Dispatched round 39, 2026-09-21 ~18:20 AWST, by the orchestrator.

## THE DEFECT — diagnosed, but VERIFY it rather than trusting it

CI run **35582968952** built `core` green and then died in `base`:

```
error[E0583]: file not found for module `target`
error: could not compile `apex-backup-core` (lib) due to 1 previous error
Error: building at STEP "RUN set -eux; cd /build/apexd; cargo build --release --locked; …"
##[error]base build failed with 101 and produced no image
```

The module is not missing from git. `apexd/apex-backup-core/src/target/` has
eight tracked files — `mod.rs`, `fs.rs`, `bucket.rs`, `ssh.rs` and their
`tests.rs` — and `git merge-base --is-ancestor` puts the commit that added them
(`990ea554`, *"fix(backup): the whole target module was invisible to git"*) on
both `roadmap/v2.2` and the branch that failed.

**`.containerignore` line 11 is `**/target/`.** Line 10 is already the correct,
anchored `apexd/target/`. The unanchored pattern matches a directory named
`target` at ANY depth, so `apexd/apex-backup-core/src/target/` is stripped out
of the build context before `podman build` ever starts, and `cargo` inside the
container then cannot find the module.

The whole `.containerignore` file arrived on **2026-09-20** in `aa9e582b`
("feat(core): install APEX's own kernel…"), which is why `base` built fine
before that and has been broken since. The file's own comment explains the
real intent — keeping a developer's ~440 MB of host `apexd/target/` artefacts
out of the context — and `apexd/target/` alone achieves it.

There are exactly two cargo roots: `apexd/` (a workspace, so one target dir)
and `windows-installer/`.

**Verify before fixing.** The diagnosis above was read off the file and the log
in fifteen minutes; it has not been demonstrated. Show the exclusion actually
happening — build the context and list it, or use `podman build` with a trivial
Containerfile that `ls`es the path — before and after.

## WHAT DONE LOOKS LIKE

1. The exclusion is DEMONSTRATED, not argued.
2. Fixed so the intent survives: host build artefacts still excluded, source
   never excluded. Anchor per cargo root.
3. **A gate for the class, because this will happen again.** The failure mode
   is "a tracked source file is invisible to the build context", and it costs a
   40-minute build to discover. A test that compares the set of tracked files
   against what `.containerignore` excludes, and refuses when any tracked
   `*.rs`/`*.py`/`*.sh` under a source directory is excluded, catches every
   future instance for about a second. Prove it BOTH ways: it must go red if
   somebody re-adds `**/target/`.
4. `base` demonstrably builds past the `cargo build --release --locked` RUN.
   A full `base` build is ~40 min; if you cannot afford one, build just far
   enough to prove the compile succeeds and say exactly what you proved.
5. Evidence file `ROADMAP/evidence/ci-green-context-20260921.md`.

## BOUNDS

- `.containerignore`, the new gate, its CI wiring, and the evidence file.
- Do NOT restructure the Containerfiles. Do NOT rename the source module to
  dodge the pattern — the pattern is the defect.
- A `core` image built green locally at 18:06 today as `localhost/apex-os-core:latest`
  (`afe03dcdf173`); reuse it rather than rebuilding the world.

## NEXT

- Reproduce the exclusion: show that `apexd/apex-backup-core/src/target/mod.rs`
  is absent from the build context under the current `.containerignore`.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing

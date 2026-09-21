# pkg-alternatives — extraction never runs %post, so every `alternatives` package installs broken

items: P0-001 (the package engine), feeds P1-038
repo: apex-os
worktree: /var/tmp/apex-work/wt-pkg-alternatives
branch: task/pkg-alternatives, cut from origin/roadmap/v2.2 @ 95c0a798
lab: /var/lab-scratch/pkg-alternatives

Dispatched round 40, 2026-09-22 ~05:20 AWST, by the autoresume orchestrator.
Diagnosis by the peer session (landed `95c0a798`); blast radius measured by the
orchestrator.

## LANDABLE 77e51d27

`task/pkg-alternatives`, three commits on `origin/roadmap/v2.2` @ `6fa9ddbc`.
Touches `files/system/libexec/apex-pkg`, `tests/test-apex-pkg-alternatives.sh`
(new) and `.github/workflows/pr-validation.yml`.

## NEXT

- Running one last mutation: delete the `image_owns "$link"` refusal from
  `alternatives_place` and confirm the refusal leg goes red
  (`/var/lab-scratch/pkg-alternatives/suite-mut.log`, unit
  `apex-alt-suite-mut`). Restore
  `/var/lab-scratch/pkg-alternatives/apex-pkg.good` over the engine afterwards
  and check `git status` is clean before believing anything.

## DONE

- `9607c5bc` the engine fix: `alternatives_record`, `payload_link_resolves`,
  `dangling_links`, `alternatives_replay`, `alternatives_candidates`,
  `alternatives_place`, `apply_alternatives`, called from `rebuild_extension`
  between `extract_rpms` and `fix_caches`.
- `c6098180` `tests/test-apex-pkg-alternatives.sh`. **15 passed, 0 failed** on
  the L16, 56 s: RED 33 declared links absent and 12 payload symlinks dangling
  on their account — the katana twelve, reproduced independently — GREEN 0, 0
  and 0. Shellcheck clean; `check-suites-run-in-ci.sh` and
  `check-shellcheck-coverage.sh` both pass.
- `77e51d27` CI wiring (floor 15) + `ROADMAP/evidence/pkg-alternatives-20260922.md`.
- Mutation-tested the call-site assertion both ways: with the
  `apply_alternatives` line deleted the suite prints `14 passed, 1 failed`, and
  with podman hidden it still runs that one assertion and exits 1.
- `tests/test-apex-pkg.sh` still `86 passed, 0 failed`.

## IN PROGRESS

- nothing half-written. The branch is pushed and complete.

## FOUND

- The scriptlet is `%posttrans`, not `%post` (wine-core). `--noscripts` skips
  both, so the diagnosis holds, but a parser must read POSTIN **and** POSTTRANS.
- wine-core's `%posttrans` also declares alternatives links that are NOT
  dangling symlinks but **absent files**: `/usr/lib64/wine-wow64/wine/{x86_64,
  i386}-windows/{dxgi,d3d8,d3d9,d3d10,d3d10core,d3d11}.dll`, several with
  `--slave` followers. The card's property ("no dangling symlink an installed
  package owns") does not see those, so the gate needs a second assertion.
- Scriptlet text uses shell line continuations, single-quoted names with
  parentheses (`'wine-dxgi(x86-64)'`), `--slave` (the old spelling of
  `--follower`) and trailing `|| :`. A parser must tokenize, not regex.
- **CORRECTION — alternatives IS fully functional on APEX.** I first claimed the
  admin database was missing because `rpm -ql alternatives` lists
  `/etc/alternatives.admindir` (a **dot**) and `/var/lib/alternatives`, and
  neither exists on the booted image. `strace` says the binary actually reads
  `/etc/alternatives-admindir` (a **hyphen**), which does exist and is
  populated; `alternatives --display java` and `--list` both work, and
  `find /etc/alternatives -xtype l` is 0. The rpm file list and the binary
  disagree about the spelling. Read the syscall, not the file list.
- `alternatives --altdir/--admindir` CANNOT be pointed at a build root: with
  `--altdir /r/etc/alternatives` it writes `<link> -> /r/etc/alternatives/wine`,
  the build-time prefix baked into the runtime symlink, and records the
  prefixed link path in the admin file too. Measured, not assumed.
- `iptables-nft`'s scriptlet builds its `--install` arguments from **shell
  variables** (`$pfx`, `$pfx6`). A text parser cannot read it. Executing the
  scriptlet with a recording `alternatives` on PATH recovers all three
  invocations and every `--follower` in full.
- Interception by PATH alone is not enough: `wine-core`, `nmap-ncat` and
  `java-latest-openjdk` all invoke `/usr/bin/alternatives` by **absolute path**.
  Rewriting `(/usr)?/s?bin/(update-)?alternatives` in the scriptlet body to the
  recorder's own absolute path catches both forms and keeps `[ -x … ]` tests
  true.
- `setpriv` and `runuser` are **absent from `registry.fedoraproject.org/fedora:43`**
  (present on the L16). A suite that drops privileges must install util-linux.

## BLOCKED ON

- nothing

# pkg-alternatives — extraction never runs %post, so every `alternatives` package installs broken

items: P0-001 (the package engine), feeds P1-038
repo: apex-os
worktree: /var/tmp/apex-work/wt-pkg-alternatives
branch: task/pkg-alternatives, cut from origin/roadmap/v2.2 @ 95c0a798
lab: /var/lab-scratch/pkg-alternatives

Dispatched round 40, 2026-09-22 ~05:20 AWST, by the autoresume orchestrator.
Diagnosis by the peer session (landed `95c0a798`); blast radius measured by the
orchestrator.

## NEXT

- Create the worktree/branch, then build the RED half: a suite that runs the
  shipped `extract_rpms` over a real wine-core rpm in a Fedora 43 container and
  counts dangling symlinks in the payload. Expect 12.

## DONE

## IN PROGRESS

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
- **The image's own alternatives state is half-dead already.** `alternatives`
  rpm owns `/var/lib/alternatives` and `/etc/alternatives.admindir`; on this
  booted APEX image **neither exists**. `/etc/alternatives` has real symlinks
  (java etc.) but there is no admin database behind them, so
  `alternatives --display/--config/--remove` cannot work on APEX today. Not
  this unit's bug, but it is why reproducing the full alternatives triangle in
  the extension would buy nothing.

## BLOCKED ON

- nothing

# pkg-alternatives — extraction never runs %post, so every `alternatives` package installs broken

items: P0-001 (the package engine), feeds P1-038
repo: apex-os
worktree: /var/tmp/apex-work/wt-pkg-alternatives (create it)
branch: task/pkg-alternatives, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/pkg-alternatives

Dispatched round 40, 2026-09-22 ~05:20 AWST, by the autoresume orchestrator.
Diagnosis by the peer session (landed `95c0a798`); blast radius measured by the
orchestrator. **Read `ROADMAP/evidence/` for `95c0a798`'s file before starting.**

## THE DEFECT

`apex-pkg` builds the system extension by **extracting RPMs**. Extraction does
not run `%post`. Fedora's `wine-core` owns `/usr/bin/wine` and
`/usr/bin/wineserver` and creates them **in its scriptlets**, through
`alternatives`:

```
/usr/bin/alternatives --install /usr/bin/wine wine /usr/bin/wine64 …
```

So those two targets are never created, and everything pointing at them dangles.

**Measured on katana 2026-09-22 05:15, independently of the diagnosis:**

- **exactly 12 dangling symlinks in `/usr/bin`**, all wine's, all `-> wine`,
  each **4 bytes** (the length of the string `wine`): `msidb`, `msiexec`,
  `notepad`, `regedit`, `regsvr32`, `wineboot`, `winecfg`, `wineconsole`,
  `winedbg`, `winefile`, `winemine`, `winepath`
- `wine32`, `wine64`, `wineserver32`, `wineserver64` are **real files** — which
  is why `wine64 --version` prints `wine-11.0 (Staging)` and only *running*
  something fails
- **0 dangling under `/etc/alternatives`**, and java's entries there resolve

That last line is the calibration and it matters: **the image's own alternatives
are fine**, because the image installs by dnf and runs scriptlets. Only
**extension-installed** packages are hit. So the class is real and currently
unrealised beyond wine on this machine — do not write this up as "java is
broken", because on katana it is not.

## WHY IT IS WORTH A UNIT ANYWAY

It is invisible to every cheap check. `command -v` answers. `--version` prints.
`ls` lists the symlink happily. The failure only appears when something is
actually *run*. Any future `apex install` of a package that publishes through
`alternatives` — editors, JDKs, the iptables/nftables compat wrappers, `vi`,
`cc` — installs "successfully" and does not work.

## WHAT DONE LOOKS LIKE

1. **Decide the mechanism, and argue it rather than defaulting.** At least these
   are on the table and each has a real cost:
   - run `alternatives --install` for the extracted set at build time, derived
     from the rpm's own scriptlet rather than from a hand-written list
     (hand-written lists rot silently — this repo has been bitten by that);
   - run the scriptlets properly in the extraction chroot (biggest blast radius,
     and `%post` can do arbitrary things, so say what you are willing to run);
   - post-process: find symlinks the extraction left dangling and resolve them
     against the `alternatives` metadata the rpm ships.
   Say why the others were rejected.
2. **A GATE, proved BOTH ways.** The property is "the extension contains no
   dangling symlink that an installed package owns." It must go red on an
   extension built from today's `apex-pkg` with wine in the set, and green
   after the fix. A gate that cannot be shown red is this repo's dominant
   defect family and will be treated as not done.
3. `ROADMAP/evidence/pkg-alternatives-20260922.md`.

## BOUNDS AND WARNINGS

- **Do not "fix" this by special-casing wine.** The package is the example, not
  the bug.
- `apex-pkg` is the most load-bearing script in this repo and has a long history
  of defects that only appear on a real machine. Read
  `ROADMAP/evidence/` for `pkg-etc-label` and `pkg-update` first: `install_etc`
  once deleted 26 image-owned `/etc` files, and the removal pass must never
  delete a path an image package owns (ask the system rpmdb, `rpm -qf`).
- **`apex-pkg` decides image ownership by asking the rpmdb.** Anything you add
  must not blind that guard.
- **Do NOT run `apex install` or rebuild the extension on katana.** Its root
  filesystem is 96% full (907 G of 954 G) and `precheck` passes root-space with
  ZERO margin. Build and test in a container on the L16.
- Never push `main`, never open a PR, never land on `roadmap/v2.2`. Push your
  branch and mark `## LANDABLE <sha>` on this card.
- Headless only. `/var/lab-scratch`, never `/tmp` (15 GB tmpfs on 29 GB RAM).
- Long jobs under `systemd-run --user`, never `nohup &`.

## A METHOD NOTE THAT COST TWO SESSIONS TONIGHT

Three checks disagreed about these files and **each was right about a different
question**: `ls` does not follow symlinks and listed them as present; `test -e`
does follow and called them missing; `stat -c %s` said **4 bytes**, which is the
length of the target string and is what actually gave it away. Earlier the same
night the mirror-image error was made in the other direction — concluding wine
was absent from `command -v wine`, when the shipped binary is `wine64`. When a
file's existence is the question, say which existence you mean.

## NEXT

- Reproduce the dangling set in a container from today's `apex-pkg` with wine in
  the set — that is the red half of the gate — before designing the fix.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing

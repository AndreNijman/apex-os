# P4: one image, and progress after the P1+P2+P3 merge

## Where the tree stands

Everything the roadmap describes is merged. `main` is at `7c88f27` (179
commits): P1, P2 and P3, plus the UI polish pass and the §24 audit. apex-shell
`main` is at `9141ea7` (PR #16).

P4 sits outside the roadmap's sections. It is the one structural change Andre
asked for after reading the merged result, plus the loose ends around it.

## The change: three images become one

Today the build publishes three flavor tags off a shared base:

| tag | who it was for |
| --- | --- |
| `daily` | laptops that never game |
| `gaming-mesa` | AMD/Intel graphics |
| `gaming-nvidia` | NVIDIA graphics |

The objection that ended this design: a person with a gaming laptop who games
*once in a while* had to make a choice at install time that they cannot revisit
without reinstalling. Choosing "daily" on capable hardware is the wrong default,
and choosing "gaming" pays for Steam and Proton on a machine that may never
launch either.

The replacement is **one image for every laptop.** Every machine installs the
same bytes and starts with the daily set. Switching to Gaming Mode asks to
install what gaming needs, and from then on the machine has it.

Andre set two constraints on that:

1. **NVIDIA drivers stay in the image by default.** They are not part of the
   on-demand gaming set. A machine with NVIDIA hardware must have working
   graphics from first boot whether or not it ever games. The akmod has to
   match the shipped kernel, which is a build-time property that no later
   `apex install` can fix.
2. **Everything else gaming-specific is on demand:** Steam, gamescope,
   MangoHud, Proton, Sunshine, OBS.

## The constraint that outlives the refactor

`installer/apex-install` derives, with an explicit "this MUST drive
TARGET_IMAGE, it is not cosmetic" comment:

    EDITION="${APEX_EDITION:-$(cat /usr/lib/apex-installer/edition ... || echo daily)}"
    TARGET_IMAGE="${APEX_TARGET_IMAGE:-ghcr.io/andrenijman/apex-os:${EDITION}}"

`TARGET_IMAGE` becomes `bootc install --target-imgref`, the ref the installed
machine tracks for every future `apex update`. Therefore **all three tags must
keep resolving forever**, `daily` included. A machine installed from an existing
ISO tracks `:daily`; machines installed from the gaming ISOs track the other
two.
Drop or rename any of them and those machines silently stop updating: they
report no error and never see another update.

The intended shape: `:daily` stays the real tag, and `:gaming-mesa` and
`:gaming-nvidia` become aliases onto the same manifest.

Given that, the installer needs **no functional change**:

* The CI `installer-iso` job is already single-edition: it pulls the
  already-published `:daily`. There is no ISO matrix to collapse.
* `installer/build-live-iso.sh`'s `EDITION` variable already defaults to
  `daily`; it becomes vestigial but stays correct.
* `installer/apex-installer-gui`'s `ACCENTS` and `ed_name` maps are keyed on
  the three editions and collapse to the `daily` branch on their own. They are
  cosmetic; leave them.

## What `apex gaming` has to say afterwards

`Readiness::blockers()` in `apexd-core/src/gaming.rs` currently treats a
missing greeter entry as meaning "this is not a Gaming edition image". Under
one image the greeter entry and the gamescope session script ship everywhere
(otherwise a daily machine that installs Steam and gamescope still could not
reach Gaming Mode), so that sentence stops being true and `session_desktop`
stops telling editions apart.

The blockers that remain are exactly the installable ones, and those are what
the switch-to-gaming flow acts on. `install_hint()` (branch `fix/gaming-remedy`)
already produces the single `sudo apex install gamescope steam` line that
clears them.

The test `a_non_gaming_image_gets_no_install_hint_because_no_package_fixes_it`
encodes today's semantics and asserts a state that can no longer occur once the
greeter entry is universal. It needs reconciling rather than blind deletion: if
some signal still distinguishes "cannot game here", the test should assert
*that* instead.

## What was wrong with CI

Read `docs/ci-release-tiers.md` before touching the base job. Four runs failed
here and three explanations were wrong. In short:

1. **`--cache-to` kills builds.** Two runs died mid-build on podman's
   `locating image with ID ...: image not known`, at step 18 and step 44. The
   push runs inside `podman build`, so it exits 125 and takes the build with it.
2. **Per-step commits made `base` unfinishable.** A commit costs a constant
   3m28s on this runner and base produces 102 of its own layers: 5h54m against a
   6h ceiling. Run 33866516076 was cancelled at 6h01m. The fix is
   `--layers=false` (one commit instead of 102), at a measured cost of 15 MiB
   per update, because base's own layers total 14.9 MiB.

The wrong turns, recorded so nobody repeats them:

* "A cache-push failure is survivable if the image exists." That is true and
  does not help: the failure lands mid-build, when there is no image yet.
* "`--cache-to` is too SLOW — 4m13s/layer x 120 > 6h." The arithmetic was right
  and the target wrong: removing both cache flags gave the worst run of all
  (5h44m, never finished).
* "It must be the storage driver copying the whole rootfs." The base job now
  prints the driver: the runner reports `overlay`, not `vfs`.

The 3m28s commit cost is STILL not explained: 22s against the old core, 3m28s
against a new one only 11% larger, from different runners hours apart. Leave it
unexplained rather than adding a fifth theory; the fix does not depend on it.

## Landing order

The two branches are now one: `p4/merge-candidate` is the one-image work plus
the gaming reconciliation, rebased onto `main` and fast-forward-clean.

Still ungated on evidence, in this order:

1. The image artifact must be **inspected** as well as built: NVIDIA akmod
   present and loadable against the shipped kernel, Mesa intact for a
   non-NVIDIA laptop.
2. Merge (fast-forward: this repository takes neither merge commits nor a
   rebase of `main`). The push triggers `build-image` automatically, because
   `Containerfile*`, `apexd/**` and `.github/**` are all in its path filter.
3. **Only after that run is green**, dispatch `build-image` again with
   `build_iso: true`. The `installer-iso` job is `workflow_dispatch`-only, has
   no `needs:`, and pulls `$IMAGE:apex`, a tag that does not exist until the
   merge build publishes it. Dispatching earlier fails on a missing tag.

`build_qcow2` is a separate input, needed only for a VM disk.

## What is NOT verified yet, and must not be claimed

`ci: publish one image, and prove every existing tag still resolves to it` adds
a step that reads each tag's digest back from the registry. **It has never
run.** Nothing local can exercise it, because it needs a publish. The
closest available proxy is `build-local.sh` applying all four names and
asserting they resolve to one image ID, which is the same property one layer
below a manifest digest. Treat "all four tags resolve to one digest" as
unverified until you have read that CI step's output. A tag assertion that
silently skips looks exactly like one that passed, and this repository has
shipped that failure before.

## Standing rules that bit during this work

* Never run a test that opens a window on Andre's desktop.
  `APEX_LABWC_SESSION_TESTS` stays unset; compositor validation runs headless
  (`WLR_BACKENDS=headless`) and asserts on the Wayland socket count.
* Never cause a polkit or keyring prompt.
* `build-image.yml` triggers on push to `main` only. Pushing a feature branch
  does not disturb a running build.
* `cargo clippy --all-targets --locked -- -D warnings` is a CI gate. Neither
  this laptop nor the Katana has clippy installed; run it in a
  `docker.io/library/rust:latest` container, and mount the **repo root** rather
  than `apexd/`, because `apexd-core/src/profile.rs` `include_str!`s
  `config/sysprofiles/` from above the workspace.
* There is deliberately no `cargo fmt` gate: rustfmt has never run over the
  workspace, and 24 files differ.

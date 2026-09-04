# P4 — one image, and progress after the P1+P2+P3 merge

## Where the tree stands

Everything the roadmap describes is merged. `main` is at `7c88f27` (179
commits): P1, P2 and P3, plus the UI polish pass and the §24 audit. apex-shell
`main` is at `9141ea7` (PR #16).

P4 is not a roadmap section. It is the one structural change Andre asked for
after reading the merged result, plus the loose ends around it.

## The change: three images become one

Today the build publishes three flavor tags off a shared base:

| tag | who it was for |
| --- | --- |
| `daily` | laptops that never game |
| `gaming-mesa` | AMD/Intel graphics |
| `gaming-nvidia` | NVIDIA graphics |

The objection that killed it: a person with a gaming laptop who games *once in
a while* had to pick, at install time, a decision they cannot revisit without
reinstalling. Choosing "daily" on capable hardware is the wrong default, and
choosing "gaming" pays for Steam and Proton on a machine that may never launch
either.

So: **one image for every laptop.** Every machine installs the same bytes and
starts with the daily set. Switching to Gaming Mode asks to install what gaming
needs, and from then on the machine has it.

Two constraints on that, both from Andre directly:

1. **NVIDIA drivers stay in the image by default.** They are not part of the
   on-demand gaming set. A machine with NVIDIA hardware must have working
   graphics from first boot whether or not it ever games — the akmod has to
   match the shipped kernel, which is a build-time property, not something an
   `apex install` can fix later.
2. **Everything else gaming-specific is on demand** — Steam, gamescope,
   MangoHud, Proton, Sunshine, OBS.

## The constraint that outlives the refactor

`installer/apex-install` derives, with an explicit "this MUST drive
TARGET_IMAGE, it is not cosmetic" comment:

    EDITION="${APEX_EDITION:-$(cat /usr/lib/apex-installer/edition ... || echo daily)}"
    TARGET_IMAGE="${APEX_TARGET_IMAGE:-ghcr.io/andrenijman/apex-os:${EDITION}}"

`TARGET_IMAGE` becomes `bootc install --target-imgref` — the ref the installed
machine tracks for every future `apex update`. Therefore **all three tags must
keep resolving forever**, `daily` included. A machine installed from an
existing ISO tracks `:daily`; machines installed from the gaming ISOs track the
other two. Drop or rename any of them and those machines silently stop
updating — no error, they simply never see another update.

The intended shape: `:daily` stays the real tag, `:gaming-mesa` and
`:gaming-nvidia` become aliases onto the same manifest.

Given that, the installer needs **no functional change**:

* The CI `installer-iso` job is already single-edition — it pulls the
  already-published `:daily`. There is no ISO matrix to collapse.
* `installer/build-live-iso.sh`'s `EDITION` variable already defaults to
  `daily`; it becomes vestigial, not wrong.
* `installer/apex-installer-gui`'s `ACCENTS` and `ed_name` maps are keyed on
  the three editions and collapse to the `daily` branch on their own. Cosmetic;
  leave them.

## What `apex gaming` has to say afterwards

`Readiness::blockers()` in `apexd-core/src/gaming.rs` currently treats a
missing greeter entry as meaning "this is not a Gaming edition image". Under
one image the greeter entry and the gamescope session script ship everywhere —
otherwise a daily machine that installs Steam and gamescope still could not
reach Gaming Mode — so that sentence stops being true and `session_desktop`
stops discriminating editions.

The blockers that remain are exactly the installable ones, which is what the
switch-to-gaming flow acts on. `install_hint()` (branch `fix/gaming-remedy`)
already produces the single `sudo apex install gamescope steam` line that
clears them.

The test `a_non_gaming_image_gets_no_install_hint_because_no_package_fixes_it`
encodes today's semantics and asserts a state that can no longer occur once the
greeter entry is universal. It needs reconciling — not deleting blindly; if
some signal still distinguishes "cannot game here", the test should assert
*that* instead.

## The build cache that made every build impossible

Worth reading `docs/ci-release-tiers.md` and the comment in the base job before
touching CI. Short version: the base tier's registry build cache cost a
dead-constant 4m13s per layer on layers whose own commands ran in one to two
seconds. 120 layers is 8h26m against a 6h ceiling, so **no build using it could
ever have finished**, and the three red runs that looked like three separate
accidents were one defect. Removed. Do not re-add without measuring on a runner.

## Landing order

Both branches are now one branch: `p4/merge-candidate` is the one-image work
plus the gaming reconciliation, rebased onto `main` and fast-forward-clean.

Still ungated on evidence, in this order:

1. The image artifact must be **inspected**, not merely built: NVIDIA akmod
   present and loadable against the shipped kernel, Mesa intact for a
   non-NVIDIA laptop.
2. Merge (fast-forward — this repository takes neither merge commits nor a
   rebase of `main`). The push triggers `build-image` automatically, because
   `Containerfile*`, `apexd/**` and `.github/**` are all in its path filter.
3. **Only after that run is green**, dispatch `build-image` again with
   `build_iso: true`. The `installer-iso` job is `workflow_dispatch`-only, has
   no `needs:`, and pulls `$IMAGE:apex` — a tag that does not exist until the
   merge build publishes it. Dispatching earlier fails on a missing tag.

`build_qcow2` is a separate input and only needed for a VM disk.

## What is NOT verified yet, and must not be claimed

`ci: publish one image, and prove every existing tag still resolves to it` adds
a step that reads each tag's digest back from the registry. **It has never
run.** Nothing local can exercise it — publishing is required — so the closest
available proxy is `build-local.sh` applying all four names and asserting they
resolve to one image ID, which is the same property one layer below a manifest
digest. Treat "all four tags resolve to one digest" as unverified until that CI
step's output has been read. A tag assertion that silently skips looks exactly
like one that passed, and this repository has shipped that failure before.

## Standing rules that bit during this work

* Never run a test that opens a window on Andre's desktop.
  `APEX_LABWC_SESSION_TESTS` stays unset; compositor validation runs headless
  (`WLR_BACKENDS=headless`) and asserts on the Wayland socket count.
* Never cause a polkit or keyring prompt.
* `build-image.yml` triggers on push to `main` only. Feature branches are free
  to push and do not disturb a running build.
* `cargo clippy --all-targets --locked -- -D warnings` is a CI gate. Neither
  this laptop nor the Katana has clippy installed; run it in a
  `docker.io/library/rust:latest` container, and mount the **repo root**, not
  `apexd/` — `apexd-core/src/profile.rs` `include_str!`s `config/sysprofiles/`
  from above the workspace.
* There is no `cargo fmt` gate, deliberately: the workspace has never been
  rustfmt'd and 24 files differ.

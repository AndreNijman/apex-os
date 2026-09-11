# base-partials
items: BASE-002, BASE-005, BASE-009, BASE-010, BASE-013, BASE-014, BASE-016, BASE-018
repo: apex-os + apex-shell
worktree: /var/tmp/apex-work/wt-base-os (apex-os), /var/tmp/apex-work/wt-base-shell (apex-shell)
branch: task/base-partials (apex-os), task/base-partials-shell (apex-shell)
base: apex-os e67fab9, apex-shell a90cef6 (both origin/roadmap/v2.2)

## NEXT
Round 12 (fresh agent; round 11's agent died on a usage limit at step 0).

**BASE-013 is CLOSED, both halves.** The shell WIP the previous agent left
dirty is committed and pushed as `ea83d21` on task/base-partials-shell, after
being verified in the engine and mutation-proved seven ways (see RESULTS
round 12). apex-os half was already `836df42f`.

**Exact next action:** BASE-014's last assertion — the rebound key that fires
after `labwc --reconfigure`. Home is apex-OS `tests/test-labwc-session.sh`,
extending the reconfigure section at :224-255. `wtype` is at /usr/bin/wtype on
this machine. Then BASE-009's safe piece (the `gamingmode` DISPATCH branch in
apex-shell check-compositor-backends.sh:340-343 under APEX_COMPOSITOR_DRY_RUN=1)
and BASE-010's two real defects (the vacuous aiprobe resolve tests, and
ai.rs:1327's `layers.max(1)`).

Round 11 in progress. BASE-018, BASE-002, BASE-016, BASE-005 CLOSED (rounds
9/10). Both branches MERGED (not rebased) with origin/roadmap/v2.2 and pushed;
both merges were fast-forwards.

**Exact next action:** BASE-013's apex-SHELL half. Edit
`src/services/config_tab/pages/MiscPage.qml`: the compositor segmented control
at :213-224 labels its options with raw lowercase ids (`hyprland`, `niri`) and
OMITS labwc entirely even though `Compositor.isValidName` (src/state/
Compositor.qml:68-70) accepts "labwc" — so a labwc user cannot pin their own
compositor from Settings. Add the map as `presentedName(id)` + a `modeName`
property on `src/state/Compositor.qml` (NOT on the backends: `displayName` is
the adapter's own name and is pinned by check-compositor-backends.sh:112 and
compositor-facade-test.qml:279-285 — leave it alone), relabel the control
Auto/Tiling/Scrolling/Floating with the VALUES unchanged, add the labwc option,
and fix the help text at :200 which names only niri as the degrading target.
Then a new `tests/check-compositor-naming.sh` in apex-shell house style
(ok/bad/want, two-space PASS/FAIL, `passed=/failed=` footer).
Then BASE-014 (wtype), BASE-010, BASE-009.

### Round-10 correction to this card's own work order
The previous NEXT said BASE-016 was still to do. It was already DONE at
`cf7df30`, committed by the predecessor minutes before it died — the five
named holes were filled in `tests/test-apex-recover.sh` (which is the right
file: BASE-016's evidence claims "there is no tests/test-apex-disposable.sh"
and concludes wrongly from it — `test-apex-recover.sh` already drives the
shipped `apex-disposable` through a stub capsule engine, and `pr-validation.yml`
runs it in the `static` job, the one with no path filter). Counts re-verified
this round, see RESULTS.

## RESULTS (round 12) — per item

### BASE-014 — CLOSED. apex-os `eb26c072` on task/base-partials.
The last open assertion ("the rebound key fires after --reconfigure") now
exists and runs. `tests/test-labwc-keybind-reload.sh` **18 passed, 0 failed,
0 skipped**.
- **A NEW FILE, not a section in either labwc suite, and that was a safety
  decision.** `test-labwc-session.sh` nests inside the PARENT display — this
  agent's env carries Andre's `WAYLAND_DISPLAY=wayland-1` and `DISPLAY=:0`, so
  its `APEX_LABWC_SESSION_TESTS=1` opt-in would have opened windows on his
  desktop. It is also in NO workflow, so anything added there runs nowhere.
  The new suite runs under `WLR_BACKENDS=headless` (note: plural — `WLR_BACKEND`
  silently falls through to DRM and fails on seat access) in a private
  XDG_RUNTIME_DIR with WAYLAND_DISPLAY/DISPLAY unset.
- **`labwc --reconfigure` SIGHUPs `$LABWC_PID`** (confirmed in labwc(1) and in
  the binary's strings), which is INHERITED. The suite unsets it on entry and
  sets it explicitly to the pid it started. Isolation is asserted as four hard
  failures before the first keypress; if any fails the suite refuses to press
  anything.
- Keypresses are observable because generated commands are PATH-resolved names:
  the model binds SUPER+T to `alacritty`, and a stub of that name on a private
  PATH records each run.
- Five phases: control (shipped binding fires), negative (unbound key, waited
  the SAME 5s), **the bug reproduced** (`--no-reload` → new key dead, old key
  alive), the criterion (after `--reconfigure` → new fires, old dead), and the
  shipped tool's OWN reload (`apply` with no `--no-reload`, covering
  `reload_labwc()`'s call site).
- MUTATION-PROVED four ways, TWO against the product:
  `reload_labwc()` made a no-op → phase E reddens (16/2) while D stays green,
  which is exactly the split those phases draw; the explicit `--reconfigure`
  deleted → D reddens (15/3); LABWC_PID aimed at a bogus pid → the isolation
  guard fires and the suite REFUSES to press anything (5/2); the shipped
  rc.xml losing its SUPER+T binding → the control fails and the suite STOPS
  rather than banking vacuous passes (6/2).
- Wired into pr-validation.yml after the keybind-generator step (which already
  vendors the apex-shell tree this needs) with labwc+wtype installed, and into
  that job's ShellCheck gate. shellcheck/bash -n clean, check-ignore exit 1,
  YAML re-parsed.
- **NAMED GAP:** whether the GitHub runner can start labwc headless is NOT
  verified — pr-validation.yml runs on push to `roadmap/v2.2`, which this
  branch must not push, and this branch's own copy of the workflow has only
  `pull_request: [main]`. The suite skips at status 0 where labwc or wtype will
  not run, so the untested outcome is a skip, never a false red.


### BASE-013 — CLOSED. apex-shell `ea83d21` on task/base-partials-shell.
The predecessor's dirty WIP (MiscPage.qml, Compositor.qml,
check-compositor-naming.sh) was NOT committed as found. Its step 0 was "verify
the QML actually loads and mutation-prove before committing", and both steps
changed the shipped result.
- The control offered raw ids as labels and OMITTED labwc entirely, so a
  Floating user could not pin their own compositor at all and a hand-set
  override left the control with nothing highlighted. Fixed with
  `presentedName(id)` + a `modeName` property on `src/state/Compositor.qml`;
  `displayName` deliberately untouched (it is the adapter's own name and two
  suites pin that contract).
- `tests/check-compositor-naming.sh` **42 passed, 0 failed** — source level.
- **`tests/compositor-facade-test.qml` 60 passed, 0 failed, 8 NEW** — engine
  level, under real headless labwc. The grep checker is a reading of the code,
  which is exactly the standard that left these items partial, so the map is
  now also asked of the running engine.
- MUTATION-PROVED SEVEN ways, restored with `cp` after each (never `mv`, never
  `git checkout --`: the WIP was uncommitted and checkout would have destroyed
  it — a safety copy was taken first).
  Label back to `"hyprland"` reddens 2; deleting the labwc option reddens 3;
  `Compositor.name` on the CONTINUATION line of the Active row reddens 1
  (confirming the predecessor's whole-file rewrite of that assertion is real);
  LabwcBackend.displayName collapsed to "Floating" reddens 1; NiriBackend
  gaining `gaps` reddens the help text's claim; `presentedName` deleted
  wholesale reddens 6 rather than dropping the count silently.
- **The seventh mutation is why the second suite exists.** Typo the ARGUMENT,
  not the function — `presentedName(root.nam)` — and the source still says
  "presentedName" everywhere the checker greps: it stays **42/0 and blind**
  while the facade goes **58/2**. That also proves the harness genuinely
  resolves `labwc`; a `""` name would leave expected and actual both empty and
  pass.
- MiscPage's bindings proven to construct by `run-settings-pages-test.sh`
  **17 passed, 0 failed**.
- Gates: shellcheck -S warning clean (shellcheck IS present on the L16 now, at
  ~/.local/bin/shellcheck — the round-11 card recorded it absent); bash -n
  clean; `check-headless-runners.sh` **25/0** with the new file inside its
  no-allowlist scan; `check-no-conflict-markers.sh` clean; check-ignore exit 1.
  Wired into ci.yml beside check-compositor-backends.sh and into its
  required-files gate; ci.yml re-parsed as YAML after editing.
- **STILL NEEDS ANDRE'S RATIFICATION** (unchanged): "Floating" is ratified at
  ROADMAP.md:1031; "Scrolling" and "Tiling" are a reading of ROADMAP.md:91 and
  are new user-visible words. They live in one map + one option list in
  apex-shell and two .desktop files + one Containerfile sed in apex-os.

## RESULTS (round 11) — per item

### BASE-013 — apex-os half DONE. apex-os 836df42 on task/base-partials.
Criterion 3 was recorded FAILING as a real defect. It was: the greeter renders
each wayland-session's `Name=` verbatim, so the login carousel read
`labwc (APEX) · niri · Hyprland` — three vendored project names, one of them
the default session.
- The fix is DATA, not code. The greeter stays a faithful desktop-entry
  consumer (what the spec asks of it) and the entries carry product names:
  apex-labwc -> **APEX Floating** (ROADMAP.md:1031 ratifies the word),
  niri -> **APEX Scrolling**, hyprland -> **APEX Tiling** (both from
  ROADMAP.md:91's "scrolling workstation" / "dynamic tiling workstation").
  apex-gaming already read "APEX Gaming Mode" and is unchanged.
  **PRODUCT DECISION NEEDING ANDRE'S RATIFICATION:** "Floating" is ratified in
  the roadmap; "Scrolling" and "Tiling" are my reading of ROADMAP.md:91 and are
  new user-visible words. Easy to change — they live in two .desktop files and
  one Containerfile sed.
- `hyprland.desktop` belongs to the Hyprland PACKAGE, so it is the one entry
  this repo cannot word in a file it owns: Containerfile.base renames it after
  the package lands, and Containerfile.apex sweeps the FINISHED directory (the
  only point where all four exist) and refuses a build whose picker names a
  compositor — which also catches the next package that drops an entry in.
- Nothing that keys off these files keys off `Name`: the greeter's default is
  the literal id "hyprland", last-session stores the file stem,
  apex-session-select validates the stem, niri-portals.conf reads
  `DesktopNames`. All asserted unchanged in §6 of the new suite.
- `tests/test-apex-greet-sessions.sh` **34 passed, 0 failed, 0 skipped**. It
  EXTRACTS AND RUNS rather than restates: the `sh -c` enumeration is lifted out
  of GreetContext.qml, the rename out of Containerfile.base, the sweep out of
  Containerfile.apex (reusing `files/scripts/check-containerfile-order`'s own
  `logical_lines` so comment-stripping matches the Dockerfile parser). A gate
  that otherwise only runs inside a multi-hour image build now runs per-PR in
  under a second.
- **The plausibility checks on each extraction earned their place immediately.**
  The first extractor stopped at the `]` inside `[ -r "$f" ]` and returned a
  50-char fragment; the sweep extractor first matched the OTHER loop in the same
  RUN (the greeter enumeration replayed inside a single-quoted `ENUM='...'`,
  whose `done'` defeats a `.*?done;` span). Both surfaced as FAILs, not as a
  green suite over an empty script.
- The build sweep counts what it swept and refuses `< 4`, because a sweep of a
  directory that turned out empty would print nothing, exit 0, and be a build
  step that cannot fail.
- MUTATION-PROVED four ways, tree restored clean after each: restoring
  `Name=labwc (APEX)` reddens 6 including the criterion assertion itself;
  deleting the Containerfile.base rename reddens 6; and the suite carries two
  in-file mutations that require the extracted BUILD GATE to refuse (a re-named
  entry, and an entry with no `Name=`) before it will believe the gate works.
- `check-containerfile-order` clean on both Containerfiles. `bash -n` clean.
  `git check-ignore` on the new file: not ignored. stop_slop run on the two
  prose files; on the ADDED lines only it is down to a single em-dash, which is
  this file's own house style. shellcheck is ABSENT on the L16 (no package on
  the atomic image) — the suite is wired into pr-validation.yml's ShellCheck
  gate at :1299 so CI runs it.

## RESULTS (round 10) — per item

### BASE-005 — DONE. apex-os 2c77182 on task/base-partials.
The one outstanding qualification was "the AMD and hw device profiles are
argv-pinned but not hardware-verified". Closed on the L16, which HAS the
hardware (Radeon 780M, `/dev/kfd` present, engine reads the machine as `amd`).
New suite `tests/test-apex-env-devices-live.sh` (265 lines) sources the SHIPPED
`files/system/libexec/apex-env` and calls `gpu_flags`, so a profile change
changes the test, then hands exactly those flags to real rootless podman on an
already-local image and reads back whether each node is present and OPENABLE.
- `tests/test-apex-env-devices-live.sh` **14 passed, 0 failed, 2 skipped**.
  /dev/kfd and /dev/dri/renderD128 both OPEN inside a real rootless container
  with the amd profile's six arguments, and both ABSENT without them (negative
  control on every device assertion). /dev/bus/usb arrives for `hw`, absent
  without it. `none` adds nothing. No container left behind.
- The two SKIPs are honest and named: `--group-add keep-groups` is
  unverifiable here (amdgpu leaves /dev/kfd mode 0666, so no supplementary
  group is what grants access — host mode printed in the skip line), and
  nvidia has no device on this machine so `--nvidia` stays argv-pinned only.
- Never calls `apex env create`, never pulls, every container `--rm`, one
  read-only probe (`dd … count=0`). No window, no password.
- MUTATION-PROVED four ways, engine restored clean after each: dropping
  `--device /dev/kfd` from `amd` reddens the live /dev/kfd assertion;
  collapsing `gpu_flags`' two `printf` lines into one **FAILS the shape
  assertion rather than skipping past it** (that is the trap this suite was
  built to avoid — a shape change would otherwise leave every device
  assertion with nothing to pass); dropping `--device /dev/bus/usb` from `hw`
  reddens two; making `none` emit a device reddens the default-holds-nothing
  assertion.
- **A DEFECT IN MY OWN FIRST COMMIT, found and fixed at `8a74816`.** The
  suite's last assertion ("no container was left behind by this suite")
  filtered `podman ps -a` on `name=^disp-` — the DISPOSABLE engine's prefix,
  lifted from the wrong neighbour — while every container this file starts took
  podman's own random name. It could not fail. PROVEN vacuous rather than
  argued: with the old filter and `--rm` removed from all three `podman run`
  calls, FIVE exited containers sat in `podman ps -a` and the suite still
  printed `PASS  no container was left behind` and a clean 14/0/2. Now every
  probe is `--name "apexdev-probe-$$-$RANDOM"`, the filter looks for that, and
  a `started_containers` counter (incremented in the current shell, never in a
  `$(…)` subshell where it would be discarded) makes the assertion SKIP rather
  than bank a pass on a machine where every hardware section skipped. Two more
  mutations: dropping `--rm` reddens it and names the five leaks (13/1/2);
  restoring the `^disp-` filter goes green again at 14/0/2 with five containers
  demonstrably present. Leftovers removed and `podman ps -a` re-checked empty
  after each.
- Wired into `pr-validation.yml`'s capsule job AND its ShellCheck gate (:1134).
  `shellcheck -S warning` clean (needed an explicit `SC1090` directive with a
  reason, matching `apex-env:170`'s convention). `bash -n` clean.
  `git check-ignore -v` on the new file: not ignored (exit 1).

### BASE-016 — DONE. apex-os cf7df30 on task/base-partials.
Committed by the predecessor; counts RE-RUN and confirmed this round, both
halves matching the commit message exactly:
- `tests/test-apex-recover.sh` **98 passed, 0 failed** (structural only).
- `tests/test-apex-recover.sh --with-binary` **220 passed, 0 failed**.
See NEXT for why this card previously listed the item as outstanding.

## RESULTS (round 9) — per item

### BASE-018 — DONE. apex-os d385c96 on task/base-partials.
Surface.bootloader now carries the caveat. `probe()` consulted
`chain.bootloader_unavailable` for the ROUTE and then dropped it, so `Surface`
could not carry it: both renderings of `apex recover status` printed a bare
"grub" while `apex boot status` printed the caveat for the same machine.
Fixed by carrying the field onto `Surface`, a caveat line under the human
label, and a `bootloaderUnavailable` sibling key in the JSON (the VALUE is
untouched — apex-shell's RecoveryService.qml reads it and two shell suites pin
the bare strings). The reason string stays out of the human line: it is an
efivarfs path plus an OS error, one token longer than the whole 96-column
budget the rendered report is measured against at test-apex-recover.sh:638.
- `cargo test -p apex` **412 passed, 0 failed** — 4 new:
  `recover::tests::the_bootloader_label_says_so_when_the_identity_could_not_be_read`,
  `recover::tests::a_readable_efivarfs_leaves_the_bootloader_label_uncaveated`,
  `boot::tests::an_unreadable_loaderinfo_is_not_a_measurement_of_grub`,
  `boot::tests::a_genuinely_absent_loaderinfo_is_a_reading_rather_than_a_refusal`.
- `tests/test-apex-recover.sh --with-binary` **201 passed, 0 failed** (194
  before; 7 new, in a new section "a bootloader identity that could not be
  read"). Includes a CHECKED seal — root/CAP_DAC_OVERRIDE walk through mode
  000, so the suite proves the refusal is real before asserting through it.
- `tests/test-boot-v2.sh --with-binary` **85 passed, 0 failed**, unchanged.
- MUTATION-PROVED both ways. Dropping the field back on the floor in `probe()`
  reddens the Rust label test. Printing the caveat unconditionally is INVISIBLE
  to the Rust tests and reddens the new shell assertion — and that is how it
  was actually caught here: the shell suite ran against a stale mutant binary
  left in target/debug and went red on exactly that assertion.
- shellcheck -S warning on the modified suite: clean (CI gates it at :1107).

### BASE-002 — DONE, no code needed. The evidence was stale.
Both named defects are ALREADY FIXED on this lineage; the roadmap's line
numbers (`adapter.rs:50-56`, `registry.rs:173-178`) are `origin/main`'s, not
roadmap/v2.2's. `4b85f24` "let opencode and codex start in a confined sandbox"
and `48a3872` "stop a daemon restart from overwriting the last sessions" are
both contained in `task/base-partials`, and both carry named regression tests.
- `cargo test -p apex-agent-core -p apex-agentd` **671 passed, 0 failed**,
  0 skipped, including
  `adapter::tests::a_symlinked_agent_reaches_the_package_its_bin_entry_points_at`
  (defect A) and
  `registry::tests::a_restart_neither_reuses_an_id_nor_overwrites_the_record_at_it`
  (defect B).
- MUTATION-PROVED both. Removing `.local/lib/node_modules` from `TOOLCHAIN_RO`
  reddens A's test. Restoring main's allocator (no `highest_used_id` disk scan,
  no `create_new` guard, no transcript check) reddens FIVE registry tests
  including B's. Worktree restored clean after each.
- NOTE for the orchestrator: the fix is `.local/lib/node_modules`, NOT
  `.local/lib` — `adapter.rs:639` asserts the wider path is absent by design.
  Do not "complete" this by widening it.
- The live daemon was never touched: these are unit tests over temp stores, and
  the three integration suites that do spawn a daemon kill by pid, never by
  name, each with its own XDG_RUNTIME_DIR.

## DONE
- Worktrees created and both branches pushed with -u before any work.
- Read existing roadmap evidence for all eight items (see WHAT EVIDENCE SETTLES).

## WHAT EXISTING EVIDENCE ALREADY SETTLES (per item)
- BASE-002: attach/detach/persistence PROVEN live (isolated daemon, marker
  replayed after daemon kill). Direct binaries proven upstream, not shims.
  Settled. Open: two named code defects (adapter.rs:50-56 TOOLCHAIN_RO lacks
  .local/lib; registry.rs:173-178 next_id resets to 1 on restart).
- BASE-005: capsule isolation MEASURED — read-only OS layer only, shares real
  HOME, /run/host read-write as the user, apex-env refuses root. Not a security
  sandbox; that is recorded fact, not a gap. 253/253 + 44/44 green. Open: AMD
  and hw device profiles argv-pinned, never hardware-verified.
- BASE-009: perf metrics accuracy PROVEN live (vram matched nvidia-smi; frame
  time honestly unmeasurable). Open: controller-first path unproven (gamescope
  and steam were absent at measurement time); Desktop->Gaming live switch not
  performed.
- BASE-010: RTX 3070 detection, accel list, VRAM budget, per-user socket, TCP
  refusal all PROVEN live. 44/0. Open: no runtime or model installed anywhere,
  so placement and idle-unload have never run against a live model.
- BASE-013: shared model PROVEN (48/0 facade under real labwc, 36/0 adapter
  confinement with negative control); no config drift PROVEN. Criterion 3
  FAILS as a real defect, not a missing test: greeter prints raw compositor
  names (GreetContext.qml:332-335). Hyprland/niri backends load+schema checked
  only.
- BASE-014: theme wiring, cornerRadius/shadow config, matugen pipeline, real
  screenshot with accent border, emergency fallback all PROVEN (18/0). Open:
  rounded corners not visible at headless size, shadow indistinguishable on
  black, menu/switcher need a click, rebound key after --reconfigure untested.
- BASE-016: live run with --copy-in/--copy-out, complete teardown, explicit
  plan boundary all PROVEN. Not-a-security-boundary and the /run/host write
  reaching the host are recorded facts. Open: no shell-level suite for the
  shipped script's teardown fencing and image resolution.
- BASE-018: 60/0 structural, 85/85 --with-binary, green CI booting signed /
  unsigned / foreign / tampered UKIs under SB-enforcing OVMF + swtpm. Route
  fix landed. Open: Surface.bootloader still prints grub with no caveat when
  efivarfs is refused wholesale.

## IN PROGRESS
- nothing yet

## FOUND
- **BASE-009, a lockout-class greeter defect (NOT YET FIXED).**
  `GreetContext.qml:83-86` `_selectWanted()`: when `/var/lib/apex-greet/
  last-session` names a session the TryExec filter removed — exactly what
  happens after a Gaming Mode switch if gamescope is later removed or the
  sysext has not refreshed — it returns WITHOUT selecting anything, leaving
  `sessionIndex` at its default 0, i.e. the first surviving sorted entry
  (`apex-labwc`), not `defaultSession` ("hyprland"). The named-default
  protection at :66-79 covers only the empty-`_wantSession` path. This is the
  same class of defect as the hyprland-uwsm lockout the file's own comment
  describes.
- **BASE-009, second gap (NOT YET FIXED).** apex-shell `PowerMenu.qml:141-142`
  gates the Gaming Mode row on helper + desktop file only, never on gamescope.
  Where the entry exists but gamescope does not, the menu offers a switch that
  logs the user out into a greeter that HIDES the session, landing them back on
  the desktop with `last-session=apex-gaming` recorded — which then trips the
  defect above. The build asserts the greeter hides it
  (Containerfile.apex:129-138); nothing asserts the power menu agrees.
- **BASE-009: `apex-session-select --switch` IS `loginctl terminate-user`**
  (`files/system/libexec/apex-session-select:107`). It ends EVERY logind session
  for the uid and tears down `user-<uid>.slice` — Andre's 5-day Steam client,
  his seat0 desktop, and any agent's own ssh process tree. There is NO partial
  form: `:95` exits 0 before it unless `--switch` is passed. DO NOT RUN IT. The
  non-destructive half (validate + write + chown + sync, `:69-92`) runs without
  `--switch` and is the most that can be exercised live.
- **BASE-009 deliverable located:** apex-shell
  `check-compositor-backends.sh:340-343`'s DISPATCH covers `desktopmode` but
  NOT `gamingmode`, and `gamingmode` is also missing from that file's
  "Not listed, with reasons" block. Nothing in either repo asserts the
  `gamingmode` branch. `APEX_COMPOSITOR_DRY_RUN=1` makes it safe: `apex_run`
  (compositor.sh:46-50) prints argv and returns, never execs. Verified live by
  the recon agent: `gamingmode` prints
  `sudo -n /usr/libexec/apex-session-select apex-gaming --switch` rc=0.
- **BASE-010: an assertion that cannot fail, in the Rust tests.**
  `apexd/apexd-core/src/aiprobe.rs:978-1001` `fn resolvable()` writes a fake
  `llama-server` and returns its path — but every caller binds it as `_fake`
  and nothing sets `APEX_AI_RUNTIME`, so the three resolve tests (`:1111`,
  `:1141`, `:1163`) are written as `if let Ok(r) = resolve(...)` and assert
  NOTHING on a machine with no llama-server. The docstring at `:344-345` claims
  "`$APEX_AI_RUNTIME` … is what the shell suite points at a fake backend";
  repo-wide, nothing sets it. That is the mechanical reason placement has never
  run.
- **BASE-010: a real planner defect.** `ai.rs:1327` `let layers = layers.max(1)`
  means a manifest with `layers: 0` — which is exactly what `apex ai pull --url`
  writes (`apex/src/ai.rs:1047`) — plans `Placement::Gpu { layers: 1 }` and
  emits `--n-gpu-layers 1`, while the CLI simultaneously prints "cannot plan a
  partial offload for it" (`apex/src/ai.rs:1074`).
- **BASE-010 levers that make the item closable:** `APEX_AI_STORE` (a
  hand-built store needs no root; `pull` needs root, the store does not),
  `APEX_AI_RUNTIME` (must be a regular file), `idle_timeout` in
  `~/.config/apex/ai.toml` (valid down to small values; supervisor tick is 5s,
  `main.rs:81`). Unload is a real SIGTERM to the process group with a 5s grace
  then SIGKILL (`runtime.rs:438`); the log line to assert is
  `apex-aid: unloading {model} after {n}s idle` then `backend stopped in {ms} ms`
  — the SIGTERM-vs-SIGKILL distinction IS the "VRAM released cleanly" claim.
- katana (checked live this round): gamescope, steam, mangohud, wtype,
  nvidia-smi all PRESENT; llama-server and ollama ABSENT; GPU at 0%/22 MiB and
  no game running, but Andre is logged in on seat0 with a Steam client up 5
  days. The L16 has NEITHER gamescope nor steam (steam only as a Flatpak, which
  `apex-gaming-session:52`'s `command -v steam` does not satisfy).
- CLAUDE.md records that katana now HAS steam + gamescope + mangohud installed
  (apex-user.raw, 219 pkgs) as of 2026-09-06. BASE-009's "gamescope and steam
  are not installed" is stale. Does not by itself close the item.

## BLOCKED ON
- nothing

## RECON — apex-shell compositor + labwc (DO NOT RE-DERIVE)

Written into this card by the round-8 orchestrator. The recon subagent that
produced it finished and reported, then its parent was stopped, so the report
would otherwise have died with that context. All of it is static reading of
`wt-base-shell` @ `a90cef6` and `wt-base-os` @ `e67fab9` — no suite was run,
so the counts below are grep counts unless marked otherwise.

### The two harnesses (apex-shell)

`tests/lib/headless.sh`, 408 lines, 11 functions: `headless_require` (SKIP +
exit 0 on a missing tool), `headless_begin` (stubs + private HOME/XDG),
`headless_unstub`, `headless_sockets`, `headless_wait_socket`,
`headless_start [labwc|sway] [WxH]`, `headless_filler`,
`headless_assert_private`, `headless_assert_not_ambient_signature`,
`headless_require_nested_optin`, `headless_cleanup`.

- Compositor preference `labwc` then `sway`, both under `WLR_RENDERER=pixman`;
  labwc reads `tests/labwc-test-rc.xml`. Default mode `1920x1080`,
  `WLR_HEADLESS_OUTPUTS=1`. labwc has no output stanza, so a non-default mode
  is applied afterwards with the REAL `wlr-randr` (the stub is skipped).
- Stubbed on PATH: `apex hyprctl wlr-randr niri matugen xdg-open playerctl
  wpctl brightnessctl pkcheck notify-send swww systemctl loginctl`, plus a
  `git` stub answering `v0.0.0-test` to `*describe*`.
- Private `HOME`, `XDG_RUNTIME_DIR` (0700), `XDG_{CONFIG,STATE,CACHE,DATA}_HOME`;
  unsets `XDG_CURRENT_DESKTOP WAYLAND_DISPLAY DISPLAY HYPRLAND_INSTANCE_SIGNATURE
  NIRI_SOCKET SWAYSOCK`; fonts borrowed read-only by two symlinks into the
  ambient HOME.
- `headless_assert_private` refuses three ways: runtime dir must differ from the
  ambient one, `$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY` must be a socket and NOT a
  symlink, and the display name must differ from the ambient one.
- `headless_filler` starts a 360x240 toplevel and **fails rather than skips** if
  it died: "the filler toplevel did not stay up; window assertions would be
  vacuous".

`tests/check-headless-runners.sh`, 608 lines: a bash driver that heredocs a
Python scanner, discovers every `.sh` under `tests/` with no allowlist, and
scans comment-, quote- and heredoc-stripped source. Four rules — **A** a
compositor/quickshell command word before the neutralisation point, **B**
reading `$WAYLAND_DISPLAY` before it, **C** touching `HEADLESS_AMBIENT_*`
outside `tests/lib/`, **D** a `${WAYLAND_DISPLAY:-…}` fallback or a literal
`wayland-N`. The neutralisation point is the earliest `unset … WAYLAND_DISPLAY`,
`headless_begin`, or `headless_require_nested_optin`. Most of the file is
self-test: four mutants (one per rule), each asserted to fail with the right
rule tag, plus an unmutated control and eight named regression assertions.

**So a NEW runner is written by copying `tests/run-service-tier-test.sh` (55
lines — the canonical minimal shape, and the checker's own mutant victim):**
source `lib/headless.sh`, `headless_require quickshell`, set the `staged` path,
`cleanup() { rm -f "$staged"; headless_cleanup; }`, trap, `headless_begin`,
`headless_start || exit 0`.

### What is NOT where BASE-013/BASE-014 might expect it

- The labwc **capability matrix** suite reported as 18/0 (screencopy, clipboard,
  layer-shell, session-lock, idle-notify, output-management, primary selection)
  is **apex-OS** `tests/test-labwc-session.sh`, 306 lines.
- apex-shell's `tests/labwc-matrix-test.qml` + `run-labwc-matrix-test.sh` is a
  **different** suite — input regions, dismiss surfaces, bar masks, dock
  capacity, ~30 checks — and it **does not source `lib/headless.sh`**: it brings
  its own inline headless labwc and asks the backend for TWO outputs. Treat it
  as a separate harness.
- `check-labwc-keybinds*` does not exist in apex-shell. apex-OS has
  `files/scripts/check-labwc-keybinds` (60-line Python wrapper) and
  `tests/test-labwc-keybinds.sh` (395 lines).

### BASE-013 criterion 3 and BASE-014's "Floating": one shared root cause

**There is no name-translation map anywhere in either repo.** What exists:

- `displayName` is declared once per adapter and is the project's OWN
  capitalisation — `HyprlandBackend.qml:34` "Hyprland", `NiriBackend.qml:32`
  "niri", `LabwcBackend.qml:39` "labwc", `NullBackend.qml:24` "". The facade
  forwards it verbatim (`CompositorService.qml:86-93`) and the contract is
  stated in that comment. It is asserted by `check-compositor-backends.sh:112`
  and `compositor-facade-test.qml:279-285`, so **changing its meaning is a
  deliberate, tested edit, not a drop-in**.
- "Floating" appears ONLY in comments, docs and section headings — never as a
  user-visible value. `files/desktop/labwc/rc.xml:86-88` states the intent:
  "APEX Floating is presented as the traditional overlapping-window mode, not a
  fallback". `friendlyName` has zero hits in either repo.
- The greeter reads `Name=` raw: `GreetContext.qml:332`
  `name=$(sed -n 's/^Name=//p' "$f" | head -n1)`, carried untransformed through
  `_addSession` to `sessionName` and rendered at `GreetSurface.qml:408`. The
  carousel therefore reads **`labwc (APEX)`**, **`niri`**, **`Hyprland`**, and
  where gamescope is installed **`APEX Gaming Mode`**. `Hyprland` comes from the
  upstream package, not this repo. `Containerfile.apex:147` asserts the literal
  `defaultSession: "hyprland"` survives, so selection is by id, not glob order.
- `MiscPage.qml:213-224`'s compositor segmented control offers only
  `auto/hyprland/niri` with **raw lowercase ids as labels and labwc absent
  entirely**, and its help text at line 200 names only niri as the degrading
  target — although `Compositor.qml:75` defines `isLabwc` and
  `CompositorService` loads `LabwcBackend.qml`.

So "Floating/labwc" is a documentation convention that was never implemented in
code, in two places at once.

### labwc config and the reload gap (apex-OS)

`files/desktop/labwc/rc.xml` (489 lines) sets `<cornerRadius>10`,
`<dropShadows>yes`, `<keepBorder>no`, `<maximizedDecoration>titlebar`, a
thumbnail `<windowSwitcher preview="yes" outlines="yes">` scoped to the focused
output, and Noto Sans 11 for the four theme fonts. Also shipped:
`themerc-override` (75), `menu.xml` (41), the greeter's own
`apex-greet/labwc-greet/rc.xml` (55), and `labwc-portals.conf`.

`tests/test-labwc-session.sh:224-255` DOES test `labwc --reconfigure`: it
asserts the reconfigure is accepted, then corrupts `rc.xml`, reconfigures again
and asserts the session SURVIVES (`wlr-randr` still answers) — because labwc
falls back to defaults silently on a broken config.

**No "the rebound key fires after --reconfigure" assertion exists anywhere in
either repo.** `test-labwc-keybinds.sh` never starts labwc (every `apply` passes
`--no-reload`, deliberately, so it cannot touch `~/.config/labwc/rc.xml`);
`test-labwc-session.sh` reloads a live nested labwc but never presses a key; and
there is no `wtype`/`ydotool`/synthetic-input machinery in `tests/` in either
repo. The motivating bug is quoted at `test-labwc-keybinds.sh:5-9` — "rebind the
launcher, watch the UI confirm it, press the key, nothing happens" — and it is
still untested end to end. That is BASE-014's last open assertion, and it needs
new machinery rather than a new assertion in an existing suite.

### House counting style, per repo — do not mix

- apex-shell shell suites: `ok()`/`bad()`/`want()` printing two-space-indented
  `PASS`/`FAIL`, footer `passed=$pass failed=$fail` then `[ "$fail" -eq 0 ]`.
  `want` runs the command itself rather than reading `$?` afterwards (SC2319).
- apex-shell QML suites: `passed`/`failed` int properties, `check(name, cond)`,
  final `console.log("passed=… failed=…")` then `Qt.exit(failed === 0 ? 0 : 1)`.
  The `run-*.sh` wrapper grades on the extracted COUNT, never on whether a FAIL
  line survived a grep.
- apex-OS: `printf 'PASS  %s\n'` with NO leading indent, a third `skp`/skipped
  counter, `section()` for headings, and a named footer such as
  `labwc-session: %d passed, %d failed, %d skipped`. Both labwc OS suites run
  under `set +e` deliberately, because CI invokes `bash -e {0}` and under that
  an assignment from a failing command kills the run mid-way.

## FOUND (round 9, fresh agent)
- **`wtype` IS installed at `/usr/bin/wtype`** on the L16. virtual-keyboard-unstable-v1
  is exactly what labwc feeds into its own seat, so BASE-014's "the rebound key
  fires after --reconfigure" assertion IS deliverable headless. `ydotool`,
  `dotool`, `wlrctl` are all absent; `swtpm`, `gamescope`, `steam`, `mangohud`,
  `llama-server` absent on the L16 too (ollama exists at ~/.local/bin/ollama).
  No `/usr/share/wayland-protocols` or `wlr-protocols` tree, so a hand-compiled
  virtual-keyboard client is NOT the route — `wtype` is.

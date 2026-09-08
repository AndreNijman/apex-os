# base-partials
items: BASE-002, BASE-005, BASE-009, BASE-010, BASE-013, BASE-014, BASE-016, BASE-018
repo: apex-os + apex-shell
worktree: /var/tmp/apex-work/wt-base-os (apex-os), /var/tmp/apex-work/wt-base-shell (apex-shell)
branch: task/base-partials (apex-os), task/base-partials-shell (apex-shell)
base: apex-os e67fab9, apex-shell a90cef6 (both origin/roadmap/v2.2)

## NEXT
BASE-018 and BASE-002 are CLOSED (see RESULTS). Remaining work order:
BASE-016 (fill five named holes in tests/test-apex-recover.sh — the suite the
old evidence said did not exist), BASE-005 (AMD is verifiable ON THIS MACHINE),
BASE-013, BASE-014 (wtype works — proved), BASE-010, BASE-009.

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

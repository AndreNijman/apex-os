# Validation record — 2026-09-20

Branch `task/windows-installer`, created in a separate worktree from cached
`origin/roadmap/v2.2`, exactly `303221d547489ace414d7a543350ecf9cf2a3a8c`.
`git fetch origin roadmap/v2.2` was attempted first and failed with GitHub DNS
resolution failure. Remote freshness therefore could not be established; the
base does meet the requested minimum revision. The original worktree and other
agents' untracked files were left alone. No push or PR.

## Executed

- Linux Rust/Cargo 1.98, no external crate dependencies: offline locked debug and
  release builds passed.
- `cargo test --offline --locked`: 3 unit tests passed, covering extent boundaries,
  zero-length/overflow, short reads and injected I/O failure.
- `cargo clippy --offline --locked --all-targets -- -D warnings`: passed.
- `python3 tests/image_lab.py`: 8 test methods passed (with multiple cases).
  Invokes the compiled executable against private regular 8 MiB GPT `.img`
  fixtures. All-zero target passes and reports every byte read. NTFS/ext/btrfs,
  LUKS, nested GPT/MBR, interior data and final-byte sentinels fail. An actual
  `mkfs.ext4` filesystem, created only inside a temporary regular partition image
  without user files, also fails. Protected types, attributes, stale unused GPT
  entries, corruption, overlap and absent selection fail. Symlinks fail.
  Nonzero content outside the selected extent does not affect its content scan.
  Each inspection test compares the complete before/after image bytes.
- Existing workflow static checks: 180 shell syntax checks, GUI Python syntax
  (bytecode redirected to `/tmp`), TOML parsing and daemon-only host-command
  boundary passed. No installer engine or physical disk test was executed.
- `git diff --check`: passed.

## Existing/static limitations

The repository's `.github/workflows/pr-validation.yml` path selectors do not yet
include `windows-installer/`. The Rust and image checks above were run explicitly;
CI integration should be added in a later scoped change.

The workflow's Containerfile layer check passed for base/core but failed because
`Containerfile.daily` and `Containerfile.gaming` are absent on the selected base.
Input parity against the existing sibling apex-shell checkout failed for
`touchpad.click_method` (`clickfinger` vs `buttonAreas`) and `touchpad.drag_lock`
(True vs False). These are outside this new directory; no existing file was
changed to hide them. Network cloning of a fresh shell checkout was unavailable.

`cargo fmt` is unavailable (rustfmt is not installed). No Windows target/linker
or Windows VM is available here. A Windows `.exe` has **not** been built or tested;
the Windows-specific reparse-point code has not been compiled in this session.
No physical disk, VHDX attachment, UEFI variable or ESP has been accessed.

No Windows disk enumeration, volume ownership/lock validation, GUI, destructive
confirmation, payload deployment, additive bootloader transaction or undo is
implemented. This is content-check evidence for an image laboratory, not a
hardware safety certification or a completed installer.

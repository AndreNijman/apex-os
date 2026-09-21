# luks-installer-2 — continuation of luks-installer (L-002)

items: L-002
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-installer-2
branch: task/luks-installer-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

luks-installer's own branch already LANDED in round 35 (first encrypted
install end to end, live run 8: 41/0, nvram-guard verified) — do not redo its
work. Read ROADMAP/state/agents/luks-installer.md for the full history. Note
its item 7 ("bootc-install-lab needs --generic-image") is ALREADY RESOLVED by
efivars-guard-2, also landed — do not re-do that either, and read
ROADMAP/state/agents/efivars-guard-2.md if you touch that file.

## NEXT (dispatched with, fill in as you go)
1. **Boot a disk this installer actually produced — nothing has.** The recipe
   (predecessor card NEXT item 2): one OVMF boot in the apex-bootlab container
   against the live suite's target.img, QMP send-key the passphrase, watch
   serial for the pivot to /sysroot.
2. Ask luks-enroll-2 (dispatched this round too) for the hop this unit
   couldn't measure: does sd-boot actually read \loader\credentials\*.cred off
   the ESP and pass it through sd-stub? Their run-scenarios has the signed-UKI
   chain scenario for it.
3. The unlock-keymap fallback is announced too late (on the PROGRESS page,
   after the confirm step) — a GUI-side `--check-passphrase` mode in the
   engine, or moving the typeability check to the encrypt page, closes this.

Use /var/lab-scratch for anything large, never /tmp (15GB tmpfs). A grep
pattern containing `[` or `]` is a character class — three prior runs read as
failures until `serial_has` grew `-F`.

## FOUND
-

## BLOCKED ON
- nothing yet

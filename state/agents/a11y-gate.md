# a11y-gate — settle what actually gates quickshell's accessibility tree

items: P2-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-a11y-gate (create it)
branch: task/a11y-gate, cut from origin/roadmap/v2.2
lab on katana: /var/lab/scratch/a11y-gate  (katana uses /var/lab/scratch, NOT /var/lab-scratch)

Dispatched round 40, 2026-09-22 ~04:30 AWST, by the autoresume orchestrator.

## YOU HAVE THE SEAT. ONE AGENT AT A PHYSICAL SEAT, EVER.

Two orchestrators collided on katana's seat0 earlier tonight and it cost the
previous unit three P1-038 rows. A seat split is now agreed: **this session owns
katana and seat0**; the other session owns the L16 and will not `chvt` or start
a session there without saying so first. That agreement is worth more than any
row you could measure — if you find another session on seat0, STOP and write the
card rather than taking it.

## THE QUESTION, AND WHY IT IS WORTH ONE SESSION

Two runs tonight, on two different compositors, both measured quickshell
publishing **8** accessibility nodes on the new image with **no shim** — the
number round 30 could only reach with an `LD_PRELOAD`. They agree on that and
**disagree on the mechanism**, and both agents have now said in writing that
their own causal claim is not established:

- `katana-a11y-20260922.md` concluded *"Qt decides whether to publish when the
  application object is built. Turning the bridge on later does not retrofit
  it."* Its author has since **retracted the strong form**: its sequence changed
  TWO variables between the 0 and the 8 — it set the gsetting **and** restarted
  quickshell. Self-consistent, but not controlled; a restart alone produces the
  same pattern.
- `katana-final-qual-20260922.md` §5 is a dated counter-example: `andre` had **no
  dconf user database at all** until 03:51:08, three minutes *after* an
  already-running pid 9702 published 8 frames, and that process carried no
  `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`. Its hypothesis: the gate Qt watches at
  runtime is **`org.a11y.Status`** on the accessibility bus, and the gsetting is
  merely what `at-spi-bus-launcher` mirrors into it — one writer of the gate,
  not the gate.

Read **both files** before touching anything.

## THE EXPERIMENT — one session, two walks

1. `gsettings set org.gnome.desktop.interface toolkit-accessibility false`
   explicitly. **This is now a real change, not the default**: the other agent
   created `~/.config/dconf/user` at 03:51:08 and set it TRUE, so the pristine
   state no longer exists and you must set it back down yourself.
2. Start the shell with **no** `QT_LINUX_ACCESSIBILITY_ALWAYS_ON`, no other
   accessibility variable, and **no write to `org.a11y.Status`**. Confirm by
   reading `/proc/<pid>/environ`, not by trusting the launcher.
3. **Walk #1** — read `ChildCount`. Expect 0 under the hypothesis.
4. Now set **`ScreenReaderEnabled` alone** on `org.a11y.Status` over the session
   bus — nothing else, and specifically not the gsetting.
5. **Walk #2** — read `ChildCount` on the *same, still-running* process.

The verdicts, decided in advance so the result cannot be narrated after the fact:

| walk 1 | walk 2 | conclusion |
|---|---|---|
| 0 | 8 | **`org.a11y.Status` is the gate; the gsetting is not.** A late enable DOES retrofit. `katana-a11y`'s "does not retrofit" line is struck. |
| 8 | 8 | **Neither is the gate** — quickshell-git simply publishes where 0.3.1 did not. The gsetting is irrelevant and that line is struck for a different reason. |
| 0 | 0 | The hypothesis is wrong and the startup-order finding survives. Say what else differed from the run that got 8. |

## A RESIDUE THAT WILL BITE YOUR BASELINE

`katana-final-qual-20260922.md` records it: **Qt's bridge does not *un*register
when the flag goes back to false.** quickshell stays on the a11y bus until the
greeter next starts, so `--count` answers 1 rather than 0 on a machine where
this has already been done once. A pristine zero is only obtainable on a greeter
nobody has asked. Plan for that — start from a freshly restarted greeter and say
in the evidence which state your baseline was taken from.

## WHAT DONE LOOKS LIKE

- `ROADMAP/evidence/a11y-gate-20260922.md` — the two walks, the environment read
  out of `/proc`, the dconf state at each step with timestamps, and the verdict
  from the table above.
- **Say which of the two landed evidence files is wrong**, in one sentence. Both
  authors have asked for this and neither will be offended; the other session has
  offered to amend its own file once you report.
- Do NOT edit `katana-a11y-20260922.md` yourself — its author owns it and has
  asked to make the correction. Write your result; the orchestrator relays it.
- Branch pushed, `## LANDABLE <sha>` on this card. Do not land it yourself.

## RESTORE, AND PROVE THE RESTORE

The previous unit's restore is the standard to match: disarm your dead-man timer
FIRST, restore `/etc/greetd/config.toml` by `cmp` against your own `.orig` copy
**without restarting greetd**, assert 0 `initial_session` lines, terminate ONLY
your own session by id, then hand the seat back. Verify by reading it back — not
by having run the command. Note `trap … EXIT INT TERM` does NOT end a script on
a signal, so a restore-on-kill written that way works by accident.

Leave `toolkit-accessibility` as you found it at dispatch (**true**), and say so.

## CONSTRAINTS I AM UNDER

- Never run `qs -p`. No polkit or keyring prompts — Discord/Chromium/VS Code can
  raise one on a screen nobody is at; do not start them.
- Never touch katana's boot path, NVRAM or `bootc` state. Windows must keep booting.
- Do not interrupt gaming: re-read `apex game status` and `loginctl list-sessions`
  before every heavy step, not once at dispatch.
- Headless only with respect to the L16 — never open a window on Andre's desktop.
- Long jobs under `systemd-run --user`, never `nohup &`.
- Never push `main`, never open a PR, never land on `roadmap/v2.2`.

## NEXT

- Read both evidence files, confirm you hold seat0 alone, then take the baseline
  from a freshly restarted greeter.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing

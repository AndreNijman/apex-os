# P0-018: where the display watchdog lives, and the one hole left in it

Written against the shipped code, with every claim below either read out of the
source at a named line or produced by running it. The runs are on katana, in a
nested headless compositor, never on a real session.

## The short answer

The countdown has **two** owners, and the second one is not APEX Shell.

| Owner | File | Survives |
|---|---|---|
| the shell | `src/services/config_tab/DisplayService.qml` | nothing; it is the thing that dies |
| the guard | `src/scripts/apex-display-guard.sh` | the shell dying, by any signal |
| nothing yet | — | the **compositor** or the session dying |

`DisplayService._begin()` (DisplayService.qml:419) writes the transaction and
spawns the guard **before** the engine is asked to change anything:

```
'bash "$5" spawn "$1"\n' +
'exec "$6" apply --model "$1/target.json"'
```

One `bash -c`, in that order. So there is no window in which a layout is on
screen and nobody is holding its rollback — not even a window in which the
dialog could have failed to construct, because the dialog is built later, from
`DisplayService.pending`, and cannot influence a guard that is already detached.
`cmd_spawn` uses `setsid -f bash "$0" run "$dir"`, so the guard is out of the
shell's process group and out of its session.

**If the shell dies mid-countdown**, the guard notices nothing at all — it was
never watching the shell. It polls the transaction directory for a verdict, sees
none, and at the deadline restores `rollback.json` through the same engine and
the same fallbacks the Revert button uses. `tests/run-display-transaction-test.sh`
proves this by `SIGKILL`ing quickshell's own pid (asserted to be quickshell's,
not a wrapper's) with the countdown running, then reading the layout back off
the compositor. On the next start, `shell.qml` holds a reference to
DisplayService so `apex-display-guard.sh reconcile` runs whether or not anybody
opens the Display page, and re-attaches to a countdown still in flight rather
than reverting a layout the user may be about to keep.

## The hole: the compositor, not the shell

The guard is detached from the shell, not from the session. If the compositor
goes, the guard goes with it, and there is no compositor left to revert through
anyway. That would be fine if a temporary apply were temporary. It is not.

**Measured**, on katana, in a headless sway with `HOME` pointed at an empty
sandbox and the compositor detection pinned so nothing could reach a real
session: one `apex-display-apply apply --model …`, with no Keep and no second
command, left behind

```
~/.config/hypr/apex-display.conf
~/.config/kanshi/config          →  scale 1.5, the layout nobody confirmed
```

The engine persists on every apply — `apex-display-apply` line 505, "Persist on
both apply and save: a layout that is applied but not persisted…". kanshi
reapplies its profile at the next login and on every hotplug. So:

> session dies during the countdown → next login comes up on the unconfirmed
> layout, with no transaction left anywhere to say it was never confirmed.

The shell already does its half correctly: `display.json`, the model the shell
owns, is untouched until Keep, and the labwc suite asserts that
("nothing is persisted while the countdown runs"). The kanshi profile and the
Hyprland fragment are written by the engine, below the shell, and the shell
cannot stop it from where it stands.

## What apexd / apex-os needs, exactly

This is one flag in one program, in a different repository
(`apex-os/files/system/libexec/apex-display-apply`). It is deliberately not
written here.

1. **Add `--no-persist` to the `apply` verb.** `main()` currently calls
   `persist(model, live, args.dry_run)` for both `apply` and `save` (line ~505).
   With `--no-persist`, `apply` changes the live layout and writes neither
   `KANSHI_OUT` nor `HYPR_OUT`, and sends kanshi no `SIGHUP`. Nothing else about
   the verb changes.
2. **APEX Shell then passes it** from `DisplayService._begin()`, so a temporary
   apply touches only the running compositor.
3. **Keep already persists correctly**: `confirm()` runs the engine's `save`
   verb against the promoted model (DisplayService.qml:450-466), which is
   persistence with no hardware write. No change needed there.
4. **Revert must still persist.** `apex-display-guard.sh restore` puts the old
   model back with a plain `apply`; keeping it plain means the rollback also
   rewrites the kanshi profile, which is what you want after any state the
   machine may have been left in. Do not add `--no-persist` to the guard.

With that flag, a session that dies mid-countdown comes back on the last layout
the user actually confirmed, and no watchdog outside the session is needed at
all. **A user-level systemd watchdog is the wrong shape for this**: it would
have to hold the rollback across a boot, race kanshi at login, and reach a
compositor that may not be the one the transaction was made against. Making the
temporary apply genuinely temporary removes the requirement instead of adding a
process to satisfy it.

## Two defects this work found and fixed in the shell half

Both were found by `tests/run-display-unplug-test.sh`, which uses headless
sway's `swaymsg create_output` / `output <name> unplug` to add and destroy real
wlroots outputs. Neither is reachable from a harness that can only pretend an
output went away.

1. **The guard recorded a revert it had not performed.** `cmd_run` called
   `restore` and then wrote `state reverted` unconditionally. When the engine
   fails during the countdown — mid-upgrade, compositor not answering — the
   layout stayed wrong and the directory said it had been put back, so
   `reconcile` at the next shell start read "settled" and did nothing. The
   machine kept an unconfirmed layout permanently, with a record saying
   otherwise. Now `settle_revert` retries, writes `revert-failed` when it cannot
   succeed, `reconcile` treats that as work still to do, and the Display page
   tells the user the screen they are looking at was never confirmed.
2. **A rollback naming a destroyed output failed outright.** wlr-randr rejects
   the whole invocation over one unknown output. Unplug a monitor during the
   fifteen seconds — which is a likely fifteen seconds to unplug a monitor in,
   since a display that is behaving oddly is why the dialog is up — and the
   revert restored nothing at all, leaving every *other* output on the
   unconfirmed layout. `restore` now falls back through a rollback pruned
   against a fresh enumeration before it falls back to dropping modes.

## Criterion by criterion

| # | Criterion | Where |
|---|---|---|
| 5 | Revert or timeout restores the exact pre-apply configuration | `run-display-unplug-test.sh` "exact": four fields across two outputs, whole enumeration compared |
| 6 | shell / dialog / adapter / output transaction fails during the countdown | shell: `run-display-transaction-test.sh` restart phase. dialog: cannot affect it, the guard is spawned first (DisplayService.qml:419). adapter: `run-display-unplug-test.sh` "engine". output transaction: same file, "mid-flight". **Compositor death is not covered — see above.** |
| 7 | A failed apply does not discard staged values | `problemsWith` returns before `_preApply` is touched (DisplayService.qml:391); asserted in both harnesses |
| 8 | confirm, timeout, manual revert, invalid mode, disconnected output, shell restart | `run-display-transaction-test.sh` (41 assertions) plus `run-display-unplug-test.sh` (17), the disconnected output done twice — once hidden from `list`, once really destroyed |

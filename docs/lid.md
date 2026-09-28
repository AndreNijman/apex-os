# Closing the lid without stopping the work

> *"if i close my laptop lid without shutting down, most things pause to save
> battery, but all agents and whatever theyre using/doing or whatever can stay
> running, also staying with the vpn — like if i close my laptop at school and
> codex is running (which needs vpn to work) it keeps working."*

That request is the whole feature. This page describes what Rime does with it
on your machine. Roadmap P1-063.

## The short version

A laptop with live work in it keeps working when you shut the lid. A laptop
with nothing running suspends as it always has. You do not switch between the
two: the driver measures which case applies.

Three things can still take the machine down while the lid is shut, and each
one says that it fired: the machine got too hot, the battery hit the floor, or
you told it to. The first two checkpoint your work first.

```
rime lid status          # what the policy sees, and what it would do now
rime lid explain         # the same decision with every input that produced it
rime lid plan            # what would be powered down. Applies nothing
rime lid report          # what the last closed period actually did
rime lid pin             # print the current override
rime lid pin on          # keep working on a close, whatever is running
rime lid pin off         # always suspend on a close
rime lid pin auto        # hand the decision back to the measurement
```

`rime lid` on its own runs `rime lid status`.

Each of the reading verbs takes `--json`, which prints the full measurement the
text output is built from.

## What "live work" means

An agent session. `rime lid status` counts them, and that count decides an
automatic close:

* **One or more sessions:** there is work, and the machine stays awake with the
  lid shut.
* **Zero sessions:** a real measurement of nothing running. The machine
  suspends.
* **Unreadable:** the driver could not ask the agent runtime. The runtime is
  not enabled, its socket is absent, or the query failed.

The policy keeps the third case apart from the second, and the direction it
falls matters. A machine that cannot tell whether anything is running
suspends, because a laptop that stays awake on an unanswered question cooks in
a bag. `rime lid status` prints the reason, and `rime lid pin on` overrides it
on a machine where you know better.

Codex runs as an agent session, so the case in the quote at the top needs
nothing from you.

## The primitive, and why it is not a settings change

The driver holds a logind `handle-lid-switch` **block inhibitor** for as long
as the policy says to, and drops it as soon as the policy stops saying so.

Nothing edits `HandleLidSwitch=`. A static `ignore` applies to a machine with
nothing running as much as to one mid-build, and it survives a crash of
whatever set it, which is how a laptop bag becomes an oven. The inhibitor is a
child of `rime-lid.service` and dies with it, so a machine whose driver crashed
suspends on a lid close as a stock install does. That failure direction does
not cook a laptop.

You can see who holds an inhibitor at any time:

```
systemd-inhibit --list
```

## The guards

The policy does not decide once, at the moment the lid closes. A bag heats up
and a battery drains, so the driver re-checks the guards for the whole closed
period, every 30 seconds by default.

| Guard | Fires when |
|---|---|
| `thermal` | the hottest sensor came within the headroom of its own firmware `critical` trip |
| `thermal-unreadable` | a sensor is there and would not say |
| `thermal-no-sensor` | the machine reports no temperature at all |
| `battery` | charge reached the floor |
| `battery-unreadable` | the battery is there and would not say how full it is |

All five checkpoint live sessions before the machine suspends. A laptop that
kept working until it died is a worse outcome than a suspend, and the
checkpoint is what separates this feature from a crash: the driver saves the
work before the machine goes down.

The thermal rule uses the firmware's own number instead of inventing one. A
machine already publishes the temperature that is dangerous for its silicon,
and `thermal_headroom_c` sets how close to it a sensor may get. The driver
consults the absolute `thermal_ceiling_c` only for a sensor that declares no
critical trip.

## What gets powered down, and what does not

A keep-working close does not keep everything on. The driver turns off what is
useless with the lid shut and turns it back on when you reopen:

* the panel;
* the keyboard backlight, restored to the value it had;
* Bluetooth, soft-blocked and restored **only if this policy blocked it**, so a
  machine that already had it off keeps it off;
* periodic maintenance timers that have no deadline and would otherwise wake
  the machine: update and cache-refresh timers, `raid-check`, `updatedb`.

The driver stops only units that were running when the lid shut, so the
restore is exact. It touches nothing interactive, and never the shell or the
agent runtime.

`rime lid plan` prints that list for your machine and applies nothing. It also
prints what it *cannot* do and why; read that once before you rely on any of
it.

### Wi-Fi power saving goes OFF, and that costs you

This item spends power instead of saving it, and it is on by default.

The VPN is the load-bearing half of the request. If the machine never
suspends, NetworkManager never gets the sleep signal and the tunnel stays up.
Aggressive 802.11 power saving can still drop a long-lived tunnel with no
suspend involved at all, so the driver turns power save off for the closed
period.

`rime lid report` records what that costs.

## After you reopen

```
rime lid report
```

The report shows how long the machine stayed up and why, which guard ended the
period if one did, the peak temperature, what was powered down, what could not
be done and why, and the VPN timeline sampled across the whole period. Whether
the tunnel held comes from those samples.

The driver also writes the summary to the journal under `rime-lid`, so the
report is not the only copy:

```
journalctl -t rime-lid
```

### Three answers, and "nothing yet" is only one of them

`rime lid report` tells a machine whose lid has never been shut apart from a
record it could not read, and the exit status says which:

| what happened | `--json` | exit |
| --- | --- | --- |
| nothing has ever been recorded | `{"period":null}` | 0 |
| a real period | `{"period":{…},"summary":…,"vpn_held":…}` | 0 |
| the record is there and could not be read | `{"period":null,"error":"…"}` | 1 |

The third case used to print *"no lid-closed period has been recorded on this
machine yet"*, the same sentence as the first, for an unreadable file and for
one a crash had truncated mid-write. Anything that reads this output (the
shell's Closing the Lid page does) may say "nothing yet" only for exit 0 with
no `error` key.

You can reach that third case in practice. `rime-lid.service` sets
`StateDirectory=rime/lid` with no `StateDirectoryMode` and no `UMask`, so
`/var/lib/rime/lid/last.json` is 0644 and an ordinary user can read it.
**Adding `UMask=0077` to that unit would make every unprivileged `rime lid
report` answer "could not be read".** That answer is at least honest now;
before the fix it would have said "nothing has happened".

The driver writes the record to a sibling file and renames it into place, so an
interrupted write loses the new record and leaves the last good one intact.

## Configuration

`~/.config/rime/lid.toml` is yours and `/etc/rime/lid.toml` is the machine's.
Yours wins where both exist, and `rime lid status` names the file it used.

The pin is **not** root-owned, by design. Taking the inhibitor needs no
privilege, so a root-owned pin would buy nothing but a password prompt each
time you toggled the shell tile.

```toml
enabled            = true      # false makes the feature inert; stock behaviour
pin                = "auto"    # "auto", "on" or "off"
battery_floor_pct  = 20        # checkpoint and suspend with enough left to finish
thermal_headroom_c = 15.0      # degrees below the firmware's own critical trip
thermal_ceiling_c  = 95.0      # only for a sensor that declares no trip
require_thermal    = true      # a machine with no sensor may not stay awake
poll_secs          = 30        # how often the guards are re-asked

[powerdown]
display             = true
keyboard_backlight  = true
bluetooth           = true
wifi_powersave_off  = true     # see above: this one costs power
stop_user_units     = ["claude-desktop-update.timer"]
stop_system_units   = ["fwupd-refresh.timer", "dnf-makecache.timer"]
```

The example shortens `stop_system_units`. The shipped default stops seven
system timers: `rime-storage-notice.timer`, `fwupd-refresh.timer`,
`dnf-makecache.timer`, `flatpak-system-update.timer`,
`podman-auto-update.timer`, `raid-check.timer` and `mlocate-updatedb.timer`.

`battery_floor_pct` is 20 rather than 5 on purpose. A floor is there to
checkpoint and suspend with enough charge left to resume and finish, instead of
squeezing out the last watt and handing you a dead laptop and a half-written
file.

The driver reports a config file that exists and cannot be read on every run
and never skips it: a pin ignored in silence is a machine doing the opposite of
what you asked.

## The driver

`rime-lid.service` runs `rime lid watch`. The image enables it, and you should
not need to touch it.

```
systemctl status rime-lid.service
journalctl -u rime-lid.service
```

To see what it would decide without letting it act:

```
rime lid watch --once --dry-run
```

`--once` evaluates and acts once, then exits; the test suite drives that mode.
`--interval` overrides `poll_secs` for one run.

A desktop has no lid. `rime lid watch` says so on stderr and exits 0, and the
unit uses `Restart=on-failure` so that systemd does not poll the same answer
every 30 seconds for the life of the machine.

## Checking it without a lid

`tests/test-rime-lid.sh` drives the whole driver against a fixture tree. With
`RIME_LID_ROOT` set, the driver re-roots every path into that tree and
**executes no external program at all**: it appends each `systemctl`,
`rfkill`, `iw`, `nmcli`, `systemd-inhibit` and `runuser` call to a command log,
argv by argv. The suite asserts the exact argv the driver would have run, and
that the command log is the only thing that changed.

That is stronger than putting fakes first on `$PATH`, which an absolute-path
invocation walks straight past.

## What has NOT been measured: that it draws less

The acceptance criterion says the machine *"draws measurably less than it does
with the lid open"*, and nobody has that number yet. No fixture can produce
it: the draw of a panel, a keyboard backlight and a Bluetooth radio is a
physical measurement, and the only way to take it is to apply the real
power-down to a real machine somebody is using.

There are two preconditions, and the second is the one people forget:

1. **Undocked.** With an external display connected, logind consults
   `HandleLidSwitchDocked` (default `ignore`) before any inhibitor, so the lid
   close you measure is not one this feature controls.
2. **On battery.** `/sys/class/power_supply/BAT*/power_now` reads CHARGING power
   while the machine is on mains, so a run on AC measures the charger.

The procedure that would close it, on a machine nobody is using:

```
# lid open, on battery, idle, pinned so the driver is the only variable
sudo rime lid pin on
a=$(cat /sys/class/power_supply/BAT0/energy_now); sleep 600
b=$(cat /sys/class/power_supply/BAT0/energy_now)   # open baseline: a - b

# same window with the lid shut, which runs the power-down
c=$(cat /sys/class/power_supply/BAT0/energy_now); sleep 600
d=$(cat /sys/class/power_supply/BAT0/energy_now)   # closed: c - d
```

Use `energy_now` deltas over a fixed window, not an instantaneous `power_now`
reading: `power_now` swings with whatever the CPU was doing in the second you
sampled it. `rime lid report` already records the charge at close and the last
charge seen, so the driver takes the closed half of the measurement itself.
The open-lid baseline is the missing half.

## See also

* `docs/agent-runtime.md`: what an agent session is, and `rime agent list`
* `docs/update-cost.md`: the update timers this feature stops for a close

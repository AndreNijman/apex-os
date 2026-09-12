# Closing the lid without stopping the work

> *"if i close my laptop lid without shutting down, most things pause to save
> battery, but all agents and whatever theyre using/doing or whatever can stay
> running, also staying with the vpn — like if i close my laptop at school and
> codex is running (which needs vpn to work) it keeps working."*

That is the whole feature, and this page is what it actually does on your
machine. Roadmap P1-063.

## The short version

A laptop with live work in it keeps working when you shut the lid. A laptop
with nothing running suspends exactly as it always did. You do not switch
between those — the machine measures which one it is.

Three things can still take the machine down while the lid is shut, and each of
them says which one fired: it got too hot, the battery hit the floor, or you
told it to. The first two checkpoint your work first.

```
apex lid status          # what the policy sees, and what it would do now
apex lid explain         # the same decision with every input that produced it
apex lid plan            # what would be powered down. Applies nothing
apex lid report          # what the last closed period actually did
apex lid pin             # print the current override
apex lid pin on          # keep working on a close, whatever is running
apex lid pin off         # always suspend on a close
apex lid pin auto        # hand the decision back to the measurement
```

`apex lid` on its own is `apex lid status`.

Every one of those reading verbs takes `--json`, and the JSON is the same
measurement the text is rendered from, not a summary of it.

## What "live work" means

An agent session. `apex lid status` counts them, and that is the input that
decides an automatic close:

* **Sessions, one or more** — there is work; the lid stays awake.
* **Sessions, zero** — a real measurement of nothing running. The machine
  suspends.
* **Unreadable** — the agent runtime could not be asked. It is not enabled, its
  socket is absent, or the query failed.

The third is not folded into the second, and the direction it falls matters. A
machine that cannot tell whether anything is running suspends, because a laptop
that stays awake on an unanswered question is a laptop that cooks in a bag. The
reason is printed rather than swallowed, and `apex lid pin on` is the override
for a machine where you know better.

Codex is an agent session, so the case in the quote at the top is covered
without anyone having to remember anything.

## The primitive, and why it is not a settings change

The driver holds a logind `handle-lid-switch` **block inhibitor** for exactly
as long as the policy says to, and drops it the moment it does not.

Nothing edits `HandleLidSwitch=`. A static `ignore` applies to a machine with
nothing running as readily as to one mid-build, and it survives a crash of
whatever set it — which is how a laptop bag becomes an oven. The inhibitor is a
child of `apex-lid.service` and dies with it, so a machine whose driver crashed
suspends on a lid close exactly as a stock install does. That is the failure
direction that does not cook a laptop.

You can see who holds one at any time:

```
systemd-inhibit --list
```

## The guards

The decision is not made once at the lid edge. A bag heats up and a battery
drains, so the guards are re-asked for the whole closed period — every 30
seconds by default.

| Guard | Fires when |
|---|---|
| `thermal` | the hottest sensor came within the headroom of its own firmware `critical` trip |
| `thermal-unreadable` | a sensor is there and would not say |
| `thermal-no-sensor` | the machine reports no temperature at all |
| `battery` | charge reached the floor |
| `battery-unreadable` | the battery is there and would not say how full it is |

All five checkpoint live sessions before the machine suspends. "It kept working
until it died" is a worse outcome than suspending, and the difference between
this feature and a crash is that the work is written down first.

The thermal rule is built on the firmware's own number rather than an invented
one: a machine already publishes what temperature is dangerous for its silicon,
and `thermal_headroom_c` is how close to that it may get. The absolute
`thermal_ceiling_c` is only consulted for a sensor that declares no critical
trip at all.

## What gets powered down, and what does not

A keep-working close is not "stay fully on". Everything that is useless with
the lid shut goes off and comes back on reopen:

* the panel;
* the keyboard backlight, restored to the value it had;
* Bluetooth, soft-blocked — and restored **only if this policy blocked it**, so
  a machine that had it off already gets it left off;
* periodic maintenance timers that have no deadline and would otherwise wake
  the machine — update and cache-refresh timers, `raid-check`, `updatedb`.

Only units that were actually running when the lid shut are stopped, so the
restore is exact. Nothing interactive is touched, and explicitly not the shell
or the agent runtime.

`apex lid plan` prints that list for your machine and applies nothing. It also
prints what it *cannot* do and why, which is worth reading once before you rely
on any of it.

### Wi-Fi power saving goes OFF, and that costs you

This is the one item that spends power rather than saving it, and it is on by
default.

The VPN is the load-bearing half of the request, not a detail. If the machine
never suspends, NetworkManager never gets the sleep signal and the tunnel
simply stays up — but aggressive 802.11 power saving is a well-known way to
lose a long-lived tunnel with no suspend involved at all. So power save is
turned off for the closed period.

The cost is recorded in `apex lid report` rather than hidden.

## After you reopen

```
apex lid report
```

How long it stayed up, why it stayed up, which guard ended it if one did, the
peak temperature, what was powered down, what could not be done and why, and
the VPN timeline sampled across the whole period — including whether the tunnel
actually held, which is asserted rather than assumed.

The same summary goes to the journal under `apex-lid`, so the report is not the
only place it exists:

```
journalctl -t apex-lid
```

## Configuration

`~/.config/apex/lid.toml` for you, `/etc/apex/lid.toml` for the machine. Yours
wins where both exist, and `apex lid status` names which file it used.

The pin is deliberately **not** root-owned. Taking the inhibitor needs no
privilege, so a root-owned pin would buy nothing except a password prompt every
time the shell tile was toggled.

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

`battery_floor_pct` is 20 rather than 5 on purpose. The point of a floor is to
checkpoint and suspend with enough charge left to resume and finish, not to
squeeze out the last watt and hand you a dead laptop and a half-written file.

A file that exists and cannot be read is reported every time, never skipped —
a silently ignored pin is a machine doing the opposite of what you asked.

## The driver

`apex-lid.service` runs `apex lid watch`. It is enabled with the image; you
should not need to touch it.

```
systemctl status apex-lid.service
journalctl -u apex-lid.service
```

To see what it would decide without letting it act:

```
apex lid watch --once --dry-run
```

`--once` evaluates and acts exactly once and exits, which is what the test
suite drives. `--interval` overrides `poll_secs` for one run.

A desktop has no lid. `apex lid watch` says so on stderr and exits 0, and the
unit is `Restart=on-failure` precisely so that answer is not polled every 30
seconds for the life of the machine.

## Checking it without a lid

`tests/test-apex-lid.sh` drives the whole driver against a fixture tree. With
`APEX_LID_ROOT` set, every path is re-rooted into that tree and **no external
program is executed at all** — `systemctl`, `rfkill`, `iw`, `nmcli`,
`systemd-inhibit` and `runuser` are appended to a command log argv by argv
instead. The suite asserts the exact argv the driver would have run, and that
the command log is the only thing that moved.

That is stronger than putting fakes first on `$PATH`, which an absolute-path
invocation walks straight past.

## See also

* `docs/agent-runtime.md` — what an agent session is, and `apex agent list`
* `docs/update-cost.md` — the update timers this feature stops for a close

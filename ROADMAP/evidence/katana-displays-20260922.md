# katana already has the mixed-DPI desk three rows were waiting for

Measured 2026-09-22 from DRM directly, not from a compositor:

| output | card | mode | physical | density |
|---|---|---|---|---|
| `eDP-1` | card1 | 1920x1080 | 38 × 22 cm | **~128 DPI** |
| `HDMI-A-1` | card2 | 1920x1080 | 54 × 30 cm | **~90 DPI** |

Both `connected`. card2 is the discrete GPU, which matches
`katana-qualification-20260919.md` §5.1 — "the external monitor is driven by
the discrete GPU in all three desktop sessions".

## What this corrects

`ROADMAP/ANDRE-TODO.md` asked Andre to supply "a second monitor, ideally a
different DPI to the laptop panel" for **P0-001**, **P1-038** and
**P1-040-per-output-scaling**. He already has one, on katana, and has given
standing permission to use that machine. The ask was wrong.

The two panels are the same resolution, which is why this is easy to miss and
worth writing down: identical modes, **1.4× apart in density**. That is exactly
P1-040's case — "two outputs at compositor scale 1 with different densities each
get their own size" — and it cannot be simulated honestly, which is why the row
has stayed open.

## What is still genuinely blocked, and stays in the TODO

- **VRR.** Confirmed again here: no output on katana exposes `vrr_capable` at
  all — the attribute does not exist under `/sys/class/drm/card*-*/`. Not a
  configuration problem; that connector cannot answer the question.
- **Physical output hotplug.** Plugging and unplugging needs a person.
- **A third density**, if any row wants one. Two is what exists.

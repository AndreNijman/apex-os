# P0-023: unify settings staging, apply, save and revert

Survey done 2026-09-06 against apex-shell `roadmap/v2.2`. Hand this to the
implementing agent as a starting point rather than a conclusion. Each count
below came from grepping, so confirm the pages by reading them.

## What is there today

Nine settings surfaces, and they use three different mental models.

| Page | apply | save | revert | reset | staged/pending | live |
|---|---|---|---|---|---|---|
| DisplayPage | 1 | 1 | 2 | 0 | 7 | 1 |
| KeybindsPage | 0 | 1 | 0 | 0 | **47** | 13 |
| BlueprintPage | 2 | 2 | 1 | 0 | 11 | 0 |
| AppearancePage | 0 | 0 | 0 | 1 | 0 | 3 |
| DataPage | 0 | 0 | 0 | 0 | 0 | 1 |
| InputPage | 0 | 0 | 0 | 1 | 0 | 0 |
| LayoutPage | 0 | 0 | 0 | 1 | 0 | 1 |
| MiscPage | 0 | 0 | 0 | 3 | 0 | 2 |
| RecoveryPage | 0 | 0 | 0 | 1 | 0 | 2 |

Three models:

1. **Transactional.** DisplayPage stages changes, applies them for a countdown,
   then asks the user to keep or revert. This is the only page where a change can
   blank a screen, which is why it has the machinery. P0-018 is fixing it.
2. **Staged, then saved.** KeybindsPage holds pending edits and writes on save.
   47 references to pending state and no revert control.
3. **Live.** Appearance, Data, Input, Layout, Misc and Recovery write as you
   touch them. Their only escape is Reset, which means something different on
   each page.

BlueprintPage is its own thing: apply, save and revert against a declarative
document rather than against the machine.

## What the acceptance criteria demand

1. Each page distinguishes live, staged, persisted, and needs-relogin-or-reboot.
2. Apply, Save and Revert mean the same thing everywhere.
3. Staged state survives navigation, or the user is told it did not.
4. A backend failure shows an error and keeps the user's intent recoverable.
5. Dangerous changes get transactional confirmation.
6. Pages read back effective state after applying.

Criterion 3 is the one with teeth against the current code. Six of nine pages
hold no staged state, so they pass it for free. KeybindsPage has 47 pending
references and no revert, so it is where the user can lose work by clicking a
different tab. Check that first, and check it by doing it.

## The judgement this task turns on

Do not convert the six live pages into staged pages. Making a brightness slider
require a Save click would be a regression dressed as consistency, and the
roadmap asks for shared semantics, not a single behaviour.

The **vocabulary and the promise** are what must become consistent:

- A live control says so, and the user can see that the write landed.
- A staged control shows that something is held and unsaved, and offers the same
  way out wherever it appears.
- Apply means "make this real now, subject to confirmation". Save means "persist
  what is already real". Revert means "put back what was there before". Reset
  means "return to the shipped default", which is a different thing again and is
  currently the only word six pages use.
- A control needing a relogin or a reboot says which, where the user changes it.

Write that down as a small shared component or state contract that pages adopt,
rather than nine pages each re-deciding. `src/components/config/` already holds
the shared controls (`CfgSlider`, `CfgSwitch`, `CfgRow`, `CfgSection`,
`CfgScroll`), so the vocabulary belongs beside them.

## Sequencing

This task touches every settings page, so it collides with anything else in
`src/services/config_tab/`. Land these first:

- P0-017, which changes the dashboard and settings chrome
- P0-018, which rewrites DisplayPage's transactional flow and defines what
  Apply, Keep and Revert mean there
- P0-020, done, which changed every `CfgSlider` consumer
- P0-025, which changes what KeybindService and DisplayService write

P0-018 is the reference implementation for criterion 5. Take its vocabulary and
generalise it rather than inventing a second one.

## Verification

`tests/` has `run-nested-labwc.sh`, `run-popup-smoke.sh`, `run-scaling-test.sh`
and the `check-*.sh` source-gate pattern. P0-024 asks for a settings regression
harness covering exactly this, and it depends on P0-017, P0-018, P0-019 and
P0-020, so build the navigation-away-with-pending-edits test here in a shape
P0-024 can adopt.

The test that matters: stage a change, navigate to another page, come back, and
assert the change is either still staged or explicitly discarded with the user
told. Losing it without a word is the defect.

# Vendored from apex-shell

`agentstate.js` is a **verbatim copy**. It is the desktop's single source of
the agent state -> colour-token mapping, and `AgentStateAgreementTest` parses
this copy and asserts the Kotlin table agrees with it in both directions.

| | |
|---|---|
| source repo | apex-shell |
| path | `src/services/agentstate.js` |
| branch | `roadmap/v2.2` |
| commit | `6e74101` |
| sha256 | `49e0774880dfce5f4e632256cb6ab673f426cfaeea015c71c35021e40d5fb1a6` |
| copied | 2026-09-12 |

## What this does and does not prove

It proves the Kotlin agrees with **this snapshot**. It cannot prove the Kotlin
agrees with apex-shell's current file, because the two live in different
repositories and neither CI checks out the other.

`android/tools/check-agent-state.sh` closes that gap when a checkout is
available: it diffs both copies against their live counterparts under
`${APEX_SHELL_DIR:-/var/tmp/apex-work/int-shell}` and fails on a difference.
With no checkout it prints **NOT CHECKED** and exits 0 — a script that reported
success because it could not look would be the "permission denied is not
absence" mistake in a shell script.

# Colors.qml

Also a **verbatim copy**, from the same commit. It is the desktop's single
source of the *other* half of the mapping — token -> colour — and `ToneColoursTest`
parses it and asserts `Tones.kt` carries the same ten hexes, the same two fixed
foregrounds and the same `darkSurface` threshold.

| | |
|---|---|
| source repo | apex-shell |
| path | `src/theme/Colors.qml` |
| branch | `roadmap/v2.2` |
| commit | `6e74101` |
| sha256 | `73c6d4a4ee69a57c70a0673f0ce1ffd2f513061b6e09b922e311ab805f11a26e` |
| copied | 2026-09-12 |

Why a second file rather than ten numbers typed into Kotlin: `agentstate.js`
holds no colour at all, by design — it says so — so the hexes genuinely are
somewhere else, and "somewhere else" is a file that moves when the design
moves. `AgentStateAgreementTest` without this one would prove the phone agrees
about which token a state gets and prove nothing about what the token looks
like.

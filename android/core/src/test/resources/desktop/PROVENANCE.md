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
available: it diffs this copy against
`${APEX_SHELL_DIR:-/var/tmp/apex-work/int-shell}/src/services/agentstate.js`
and fails on a difference. With no checkout it prints **NOT CHECKED** and exits
0 — a script that reported success because it could not look would be the
"permission denied is not absence" mistake in a shell script.

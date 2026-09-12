// ─── agentstate.js ───────────────────────────────────────────────────────────
// What an agent session's state LOOKS like: one tone, one theme token, one
// weight, for every state the runtime can publish.
//
// Kept out of the QML for the reason src/services/remoteagents.js is: this is
// the part with the edge cases, and tests/agent-state-test.js exercises THIS
// file — the one the shell loads — rather than a copy of it. Nothing here does
// I/O and nothing here holds a colour; the functions are string → string, so a
// node process can drive all of them.
//
// ── WHY THIS FILE EXISTS (roadmap P0-021) ───────────────────────────────────
//
// The Agent Center used to decide a state's colour inline, twice, once in
// SessionRow.qml and once in RemoteSessionRow.qml, as the same three-branch
// ternary:
//
//     color: needsYou ? Theme.active
//          : state === "failed" ? Theme.wsUrgent
//          : live ? Theme.text : Theme.subtext
//
// Read that against a matugen dark scheme, where `text` is on_surface at
// #e3e2e5 and `subtext` is on_surface_variant at #c4c6ce. `starting`,
// `working`, `complete` and `exited` — four of the seven states, and the four a
// person sees nearly all the time — all render as near-white, and the two
// finished ones are near-white too. That is the bug as it was reported:
// "APEX agents display only in white."
//
// It is not a missing colour. It is a mapping that collapses seven states onto
// three values, two of which are the palette's foreground.
//
// ── THE SEVEN, AND THE FIVE ─────────────────────────────────────────────────
//
// apex-agent-core publishes seven states (AgentState, apex-agent-core/src/
// protocol.rs). The roadmap names five that must be told apart. They are not
// the same list, so the mapping is explicit rather than clever:
//
//   starting            live, nothing observed yet   → working  (glyph differs)
//   working             producing output             → working
//   waiting_for_user    quiet, probably on you       → waiting
//   permission_request  asked for a decision         → blocked
//   complete            exit 0                       → done
//   failed              non-zero exit                → failed
//   exited              killed by a signal           → idle
//
// `exited` is deliberately NOT failure-toned. apexd maps a signal death to
// Exited rather than Failed on the stated grounds that "a user stopping their
// own agent has not suffered a failure" (apex-agent-core/src/session.rs), and
// painting it red would contradict the runtime in the one place a user looks.
//
// `starting` shares the working tone rather than earning a sixth. It lasts
// until the first byte of output, its glyph and its label already differ, and a
// colour nobody sees for more than a moment is a colour that only makes the
// other five harder to learn.
//
// ── WHY A WEIGHT, AND NOT JUST A HUE ────────────────────────────────────────
//
// Around one man in twelve cannot use hue to separate red from green. A status
// list encoded in hue alone tells them nothing, and the two states it fails to
// separate here are `failed` and `done` — precisely the pair where being wrong
// costs the most.
//
// So every tone carries a WEIGHT as well, and the weight is the redundant
// channel: it changes the badge's shape and its ink, not its colour.
//
//   solid     filled chip, foreground picked for contrast   blocked, failed
//   tint      chip at low alpha, glyph in the tone          working
//   outline   no fill, 1px ring in the tone                 waiting
//   plain     no chip at all                                done, idle
//
// The weights are assigned by measurement, not by taste. Simulating protanopia,
// deuteranopia and tritanopia over the five (Viénot 1999, in
// tests/agent-state-test.js) puts the worst pair at ΔE 4.9 — `waiting` against
// `failed` for a deuteranope on a light palette — so a scheme where any two of
// the five shared a shape would genuinely lose one of them. The invariant the
// test enforces is therefore a disjunction: every pair either differs in weight
// or stays 20 ΔE apart under all three simulations.
//
// `blocked` and `failed` are the one pair that shares a shape, and they are the
// pair that can afford to: the closest those two ever come is ΔE 51. `done` is
// plain rather than tinted for the same arithmetic — under tritanopia on a
// light palette it sits ΔE 6.0 from `working`, which a shape has to separate.
//
// On top of all of that the glyph and the written label already differ per
// state, so the encoding is fourfold: shape, colour, glyph, word.

"use strict"

// state → tone. Anything unknown is `idle`, never a guess: a state this file
// has not been taught is a runtime newer than the shell, and drawing it as
// "failed" would report a fault the runtime never claimed.
var STATE_TONES = {
    starting:           "working",
    working:            "working",
    waiting_for_user:   "waiting",
    permission_request: "blocked",
    complete:           "done",
    failed:             "failed",
    exited:             "idle"
}

// tone → the Theme property that carries it, and the badge weight.
//
// The token is a NAME, not a colour. This file never sees a hex, so the palette
// can move underneath it and a light scheme costs nothing here.
var TONES = {
    working: { token: "info",      weight: "tint"    },
    waiting: { token: "warning",   weight: "outline" },
    blocked: { token: "attention", weight: "solid"   },
    failed:  { token: "danger",    weight: "solid"   },
    done:    { token: "success",   weight: "plain"   },
    idle:    { token: "subtext",   weight: "plain"   }
}

// The five the roadmap requires to be distinguishable. `idle` is excluded on
// purpose: it means "finished, nothing to say", and the palette's own muted
// foreground is the honest colour for that.
var DISTINCT_TONES = ["working", "waiting", "blocked", "failed", "done"]

function tone(state) {
    return STATE_TONES[state] || "idle"
}

function token(state) {
    return TONES[tone(state)].token
}

function weight(state) {
    return TONES[tone(state)].weight
}

// True while the session is asking for something. Both of these states mean a
// human is the blocker, and the page groups them together — but they are drawn
// differently, because "it went quiet" and "it asked for root" are not the same
// news.
function needsYou(state) {
    return state === "waiting_for_user" || state === "permission_request"
}

// ── Agent names ─────────────────────────────────────────────────────────────
// The runtime records the adapter id it was started with, lowercase. Rendering
// that with a capitalised first letter produced "Opencode", which is not a
// product's name, and left the six adapters looking inconsistent in a list
// whose whole job is to be scanned quickly.
//
// The ids are apex-agent-core's adapter set; `generic` is any other command run
// under the runtime, and it is titled "Agent" because "Generic" describes our
// plumbing rather than the thing the user started.
var AGENT_NAMES = {
    claude:   "Claude",
    opencode: "OpenCode",
    codex:    "Codex",
    gemini:   "Gemini",
    kimi:     "Kimi",
    generic:  "Agent"
}

function agentName(agent) {
    if (!agent) return AGENT_NAMES.generic
    var key = String(agent).toLowerCase()
    if (AGENT_NAMES[key]) return AGENT_NAMES[key]
    // An adapter this build has not heard of. Title it rather than drop it —
    // the id is the only thing that identifies it.
    return String(agent).charAt(0).toUpperCase() + String(agent).slice(1)
}

if (typeof module !== "undefined" && module.exports)
    module.exports = {
        STATE_TONES: STATE_TONES,
        TONES: TONES,
        DISTINCT_TONES: DISTINCT_TONES,
        AGENT_NAMES: AGENT_NAMES,
        tone: tone,
        token: token,
        weight: weight,
        needsYou: needsYou,
        agentName: agentName
    }

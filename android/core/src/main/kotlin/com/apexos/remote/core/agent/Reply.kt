package com.apexos.remote.core.agent

/**
 * Answering an agent that is waiting for you, without attaching a terminal.
 *
 * ## The verb exists and a phone reaches it
 *
 * `Request::Input { id, data }` (`apex-agent-core/src/protocol.rs`) writes raw
 * bytes into a live session's PTY master. `apex-agentd/src/main.rs:710`
 * dispatches it; `apex-remoted`'s `control()` is a denylist of exactly one
 * verb, `attach`, so it is forwarded for a paired device.
 *
 * The gate is `privilege::refuse_input` (`privilege.rs:326`), and it refuses
 * exactly two callers: a connection that IS a managed session, and one whose
 * origin could not be classified. A phone is neither. `apex-remoted` is not a
 * session and, in the daemon's own words, "never will be" — it declares an
 * origin on a connection instead. **So unlike `decide`, this is not a verb a
 * phone is refused**, and the asymmetry is the daemon's reasoning, not an
 * oversight: what §7 reserves for a human at the machine is *authorising root*,
 * and typing a sentence at an agent is not that. What the gate prevents is an
 * agent typing at another agent, which is a different threat and one a phone
 * is not part of.
 *
 * This is the whole of P1-058's "input-needed workflow" that this runtime can
 * support: a notification says an agent is waiting, and the answer goes back
 * over the same connection without opening a PTY, downloading a screen, or
 * putting a terminal in front of somebody standing at a bus stop.
 *
 * ## What the daemon does NOT do for you
 *
 * `session::write_input` writes the bytes and nothing else — no newline is
 * appended, no encoding is applied. A reply sent without a terminator sits on
 * the agent's input line unsubmitted, which looks exactly like nothing having
 * happened. [bytes] appends the terminator, and it appends **CR**: a terminal
 * delivers `\r` for the return key and the line discipline turns it into a
 * newline, which is why `Keys.kt` encodes [com.apexos.remote.core.term.Key.ENTER]
 * as `"\r"` and `KeysTest` asserts it.
 *
 * ## Why a reply is checked against the live session before it is sent
 *
 * `SessionInfo.id` is a **per-daemon counter that is reused** — `Remove` and
 * `Prune` free an id and `registry.rs:441` hands it out again. A reply composed
 * against session 7 while looking at a notification, sent after 7 was pruned
 * and the number reissued, types the user's sentence into a **different
 * agent's terminal**, in a different project, with no indication to either
 * party that it came from somewhere else. The id alone is not an identity.
 *
 * [Target] therefore carries what makes the session itself: its machine, its
 * id, and `started`, which the daemon sets once at spawn and never rewrites.
 * [check] compares them and refuses rather than guessing. This is the same
 * hazard `AlertWatcher` handles by dropping a pruned session's remembered
 * state, stated once more here because the consequence is worse: an alert
 * about the wrong session is noise, and a sentence typed into the wrong
 * session is an instruction.
 */
object Reply {

    /**
     * The session a reply was composed for.
     *
     * Deliberately not an [AgentSession]: holding the whole record would mean
     * comparing a four-second-old copy against a fresh one field by field, and
     * every field that legitimately moves — `state`, `detail`, `lastActivity`,
     * `attached` — would have to be excluded by hand. Three values that never
     * change for the life of a session are the identity.
     */
    data class Target(
        val machine: String,
        val session: Int,
        /** Unix seconds the daemon spawned it. Written once, never rewritten. */
        val started: Long,
    )

    /** Why a composed reply must not be delivered. */
    enum class Refusal(val message: String) {
        /**
         * The phone is looking at a different machine than the one the reply
         * was composed against.
         */
        OTHER_MACHINE(
            "This reply was written for a session on another machine. Connect to that " +
                "machine and answer there.",
        ),

        /**
         * Nothing on this machine has that id any more.
         *
         * `Request::Input` against it would answer `no_such_session`, but this
         * is caught here so the user is told before their sentence is sent
         * anywhere rather than after.
         */
        GONE(
            "That session is no longer on the machine. It was ended or pruned while this " +
                "reply was being written.",
        ),

        /**
         * The id is live but belongs to a different session than the one the
         * reply was written for.
         *
         * The dangerous case, and the reason [Target] carries `started`: the
         * daemon reuses an id after a prune, so this would have typed the
         * reply into somebody else's terminal.
         */
        RECYCLED(
            "That session number now belongs to a different agent. APEX reuses a session " +
                "number once the old one is pruned, so this reply has not been sent — it " +
                "would have gone to the wrong agent.",
        ),

        /**
         * The session is still listed but its process has gone.
         *
         * `write_input` answers `SessionExited` for this, and the record
         * survives the process so that its exit code can be read. Refused here
         * so the reply is not lost to an error the user cannot act on.
         */
        EXITED(
            "That agent has exited. There is no terminal left to type into.",
        ),

        /**
         * The session is stopped.
         *
         * `SIGSTOP`ped, by this app's own pause button or by anything else.
         * The bytes would be accepted by the PTY and sit in its buffer unread
         * until somebody resumed the session — which reads to the user as a
         * reply that vanished. `Signal`'s own handler records `paused` only
         * after the kill succeeds, so this flag is not a guess.
         */
        PAUSED(
            "That agent is paused. Resume it first — otherwise the reply waits in the " +
                "terminal unread and it looks as though nothing was sent.",
        ),
    }

    /**
     * Whether [target]'s reply may be delivered, given what the machine says
     * now.
     *
     * @param machine the device id the phone is connected to at this moment.
     * @param live the sessions from the most recent `list`.
     * @return null when the reply may be sent, or why it may not.
     */
    fun check(target: Target, machine: String, live: List<AgentSession>): Refusal? {
        if (target.machine != machine) return Refusal.OTHER_MACHINE
        val found = live.firstOrNull { it.id == target.session } ?: return Refusal.GONE
        // Before the liveness checks, not after: an exited session that is
        // ALSO a different session must be reported as the recycled id, which
        // is the answer that says the reply was about to go somewhere wrong.
        if (found.started != target.started) return Refusal.RECYCLED
        if (found.isTerminal) return Refusal.EXITED
        if (found.paused) return Refusal.PAUSED
        return null
    }

    /**
     * The bytes for a typed reply, terminator included.
     *
     * Trailing whitespace a soft keyboard added is not part of what the user
     * meant to send, and a trailing newline in particular is: Android's IME
     * puts `\n` in the field when the action key is "send" on a multi-line
     * box, and forwarding it as well as the CR would submit the line twice —
     * once as the reply and once as an empty line, which many agents read as
     * "accept the default" for whatever they ask next.
     *
     * An **empty** reply is legal and is not the same as no reply: a bare
     * return is how a person accepts a default, and an agent asking "press
     * enter to continue" is exactly the case this workflow exists for. The
     * caller decides whether to offer it; this function does not refuse it.
     */
    fun bytes(text: String): String = text.trimEnd('\n', '\r', ' ', '\t') + "\r"

    /**
     * Whether a reply of this text would send anything but a bare return.
     *
     * For a screen that wants to label its button differently for the two, and
     * so that "send" on an empty box is a deliberate choice rather than a
     * mis-tap.
     */
    fun isBare(text: String): Boolean = text.isBlank()
}

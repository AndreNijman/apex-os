package com.apexos.remote.ui.term

import androidx.compose.runtime.withFrameNanos
import com.apexos.remote.core.term.Snapshot
import com.apexos.remote.core.term.Viewport
import kotlinx.coroutines.channels.Channel

/**
 * The repaint loop, lifted out of the composable so it can be run by a test.
 *
 * ## What it replaced, and why that was wrong twice over
 *
 * `TerminalScreen` used to hold this:
 *
 * ```
 * while (true) {
 *     withFrameNanos { }
 *     if (terminal.revision != last) snapshot = viewport.snapshot()
 * }
 * ```
 *
 * It coalesced correctly — a `cat` feeding the terminal three hundred times a
 * second still produced one snapshot per displayed frame — and that is the
 * part kept below. What it got wrong is what it did when **nothing** was
 * happening: `withFrameNanos` is a request for the next frame, so the loop
 * asked the Choreographer for a frame, looked, found nothing, and asked again,
 * for as long as the screen was open. An idle terminal ran the display
 * pipeline at sixty hertz — which `Terminal.revision`'s own documentation says
 * is the thing a revision counter exists to avoid.
 *
 * The second consequence is the one that made this feature untestable.
 * Compose's `Recomposer` is idle when nothing is awaiting a frame; a loop that
 * always awaits one is a `Recomposer` that is never idle, and every harness
 * built on `ComposeTestRule` waits for idleness before it returns from
 * `setContent`. Measured on a Pixel 7a: `IdlingResourceTimeoutException …
 * hadRecomposerChanges = true`, on every test that tried to compose this
 * screen. So "the terminal cannot be tested by the standard harness" was true,
 * and it was true because of a defect in the terminal rather than a limit of
 * the harness.
 *
 * ## What it does now
 *
 * It waits for a change, and only then asks for a frame. The signal comes from
 * two places, which is the whole of what can change what is painted:
 *
 * * `Terminal.onChanged`, for bytes arriving from the machine;
 * * [invalidate], for everything the user does to the *view* without changing
 *   the terminal — scrolling, searching, selecting, resizing.
 *
 * The channel is `CONFLATED`, so a hundred changes between two frames are one
 * wake-up rather than a hundred queued ones.
 *
 * ## The injectable clock
 *
 * [awaitFrame] defaults to `withFrameNanos`, which needs a
 * `MonotonicFrameClock` and therefore a composition. A test supplies its own
 * and drives it by hand, which is what lets every rule above be asserted on a
 * machine with no phone: that an idle terminal asks for **no** frames at all,
 * that a burst becomes one, that the first frame is painted unprompted.
 */
class TerminalFrames(
    private val viewport: Viewport,
    private val awaitFrame: suspend () -> Unit = { withFrameNanos { } },
) {
    /**
     * Conflated, and holding `Unit` rather than a counter: the question is
     * only ever "is anything different", and a queue of answers to that is a
     * queue of repaints nobody will see.
     */
    private val dirty = Channel<Unit>(Channel.CONFLATED)

    /**
     * How many frames this loop has actually asked for.
     *
     * Exposed because it is the measurable form of the defect above: the
     * assertion that matters is not that the screen repaints but that an idle
     * terminal stops asking. A counter is the only way to state that as a test.
     */
    @Volatile
    var framesRequested: Long = 0L
        private set

    /** Something changed that the next frame must show. Safe from any thread. */
    fun invalidate() {
        dirty.trySend(Unit)
    }

    /**
     * Paint until cancelled.
     *
     * The first snapshot is taken before anything has changed, because a
     * composition that has just been created has painted nothing and a
     * terminal attached to a session that is quiet would otherwise stay blank
     * until the user typed. `LaunchedEffect` restarts this on a text-size
     * change too, and that restart has the same first frame to paint.
     */
    suspend fun run(onFrame: (Snapshot) -> Unit): Nothing {
        onFrame(viewport.snapshot())
        while (true) {
            dirty.receive()
            awaitFrame()
            framesRequested++
            // Anything that arrived while the frame was pending is already in
            // the snapshot about to be taken, so its wake-up is spent here
            // rather than becoming one more frame that paints the same pixels.
            // One `tryReceive` and not a loop, because a conflated channel
            // holds at most one; and BEFORE the snapshot rather than after,
            // which is what makes it safe — a change landing between the two
            // sends its token after this line and is painted by the next frame.
            dirty.tryReceive()
            onFrame(viewport.snapshot())
        }
    }
}

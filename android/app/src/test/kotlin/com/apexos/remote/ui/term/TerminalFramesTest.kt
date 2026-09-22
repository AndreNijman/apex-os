package com.apexos.remote.ui.term

import androidx.compose.runtime.BroadcastFrameClock
import com.apexos.remote.core.term.Snapshot
import com.apexos.remote.core.term.Terminal
import com.apexos.remote.core.term.Viewport
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.coroutines.yield
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The repaint loop, on a machine with no phone.
 *
 * ## Why this test can exist at all, when the screen it belongs to cannot be
 * ## composed here
 *
 * Because the loop stopped being a `while (true)` inside a composable. What it
 * needs from Compose is a `MonotonicFrameClock`, and
 * `androidx.compose.runtime.BroadcastFrameClock` is one — pure Kotlin, no
 * Android, and driven by hand. So the real `withFrameNanos` path the app ships
 * runs here, one frame at a time, and every rule the loop claims can be
 * asserted rather than reasoned about:
 *
 * * an **idle** terminal asks for no frames at all — the property that makes
 *   the screen hostable by `ComposeTestRule`, and the one the old loop broke;
 * * a burst of output is **one** repaint;
 * * a change that is not the terminal's still repaints.
 *
 * `BroadcastFrameClock.hasAwaiters` is the measurement. It is exactly what the
 * `Recomposer` asks itself to decide whether it is idle, so a test asserting
 * that nothing is awaiting a frame is asserting the same thing the harness
 * does — not a proxy for it.
 */
class TerminalFramesTest {

    private val terminal = Terminal(cols = 20, rows = 6)
    private val viewport = Viewport(terminal, height = 6)

    /**
     * Run the loop under a clock this test advances.
     *
     * `runBlocking` and not a test dispatcher: nothing here is time-based, and
     * the only ordering that matters is "the loop has had its turn", which
     * [settle] provides by yielding on the single thread they share.
     */
    private fun withLoop(
        frames: TerminalFrames,
        body: suspend Harness.() -> Unit,
    ) = runBlocking {
        val clock = BroadcastFrameClock()
        withContext(clock) {
            val painted = mutableListOf<Snapshot>()
            val job = launch { frames.run { painted.add(it) } }
            val harness = Harness(clock, painted, job)
            harness.settle()
            harness.body()
            job.cancel()
        }
    }

    private class Harness(
        val clock: BroadcastFrameClock,
        val painted: MutableList<Snapshot>,
        val job: Job,
    ) {
        /** Let the loop run until it suspends again. */
        suspend fun settle() {
            repeat(SETTLE_TURNS) { yield() }
        }

        /** Give it the frame it asked for. */
        suspend fun frame() {
            assertTrue(clock.hasAwaiters, "nothing was waiting for a frame, so none could be sent")
            clock.sendFrame(0L)
            settle()
        }

        val lastText: String get() = painted.last().text()
    }

    // ---- the property that made the screen testable -----------------------

    @Test
    fun `an idle terminal asks for no frames at all`() {
        // The defect, as a test. The loop this replaced ran
        // `withFrameNanos { }` unconditionally, which is a standing request
        // for the next frame: an open terminal with nothing happening in it
        // kept the display pipeline running at sixty hertz, and kept Compose's
        // `Recomposer` permanently non-idle — which is why every attempt to
        // host this screen in `createAndroidComposeRule` timed out inside
        // `setContent` with `hadRecomposerChanges = true`.
        //
        // Remove `dirty.receive()` from `TerminalFrames.run` and this is the
        // assertion that goes red.
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            assertFalse(clock.hasAwaiters, "an idle terminal must not be waiting for a frame")
            assertEquals(0L, frames.framesRequested)
            // Twice over: not a race that happens to have settled early.
            settle()
            assertFalse(clock.hasAwaiters)
        }
    }

    @Test
    fun `a composition that has just started paints once, unprompted`() {
        // Without this the screen is blank until the first byte arrives — and
        // a session somebody attaches to while it is quiet produces no bytes
        // at all. It also matters on every text-size change, which restarts
        // the effect around a terminal that is already full.
        terminal.feed("already here")
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            assertEquals(1, painted.size)
            assertTrue(lastText.contains("already here"), lastText)
            assertEquals(0L, frames.framesRequested, "the first paint is not a frame request")
        }
    }

    // ---- what it does when there IS something to do -----------------------

    @Test
    fun `output repaints the screen`() {
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            frames.invalidate()
            settle()
            frame()
            assertEquals(2, painted.size)
        }
    }

    @Test
    fun `however much output arrives between two frames, it is one repaint`() {
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            // A fast `cat` feeds the terminal hundreds of times a second. A
            // repaint per feed would spend the whole frame budget in layout
            // and show FEWER frames than one repaint per frame does.
            repeat(300) {
                terminal.feed("line $it\r\n")
                frames.invalidate()
            }
            settle()
            frame()
            assertEquals(2, painted.size, "three hundred feeds are one repaint")
            assertEquals(1L, frames.framesRequested)
            assertTrue(lastText.contains("line 299"), lastText)
            // And then it goes quiet again, rather than spinning on the
            // three hundred wake-ups it just coalesced.
            assertFalse(clock.hasAwaiters)
        }
    }

    @Test
    fun `what arrives while a frame is pending is painted by that frame`() {
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            terminal.feed("first\r\n")
            frames.invalidate()
            settle()
            assertTrue(clock.hasAwaiters, "the loop asked for a frame")

            // Between the request and the frame, which on a phone is most of
            // the elapsed time: this is where the bytes actually land.
            terminal.feed("second\r\n")
            frames.invalidate()
            frame()

            assertEquals(2, painted.size)
            assertTrue(lastText.contains("second"), "the late arrival was painted: $lastText")
            // And it did NOT leave a wake-up behind it. Without the drain in
            // `run` this is one more frame painting pixels that are already on
            // the screen, once per burst, for ever.
            assertFalse(clock.hasAwaiters, "the change was already painted; no second frame is due")
        }
    }

    @Test
    fun `a change the terminal knows nothing about still repaints`() {
        // Scrolling, searching and selecting change what is painted without
        // changing one cell, so `Terminal.onChanged` never fires for them. A
        // loop that woke only on output would make the scrollback unusable.
        repeat(40) { terminal.feed("line $it\r\n") }
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            assertTrue(lastText.contains("line 39"), lastText)
            viewport.scrollBy(-20)
            frames.invalidate()
            settle()
            frame()
            assertFalse(lastText.contains("line 39"), "the scroll was never painted: $lastText")
        }
    }

    @Test
    fun `the loop stops asking when the screen goes away`() {
        val frames = TerminalFrames(viewport)
        withLoop(frames) {
            job.cancel()
            settle()
            frames.invalidate()
            settle()
            assertFalse(clock.hasAwaiters, "a cancelled loop must not hold the frame clock open")
        }
    }

    // ---- the wiring, which is the half a loop test cannot see --------------

    @Test
    fun `the controller wires the terminal's output to the loop`() {
        // The loop is correct and the screen is still blank if nobody tells it
        // that bytes arrived. `TerminalController.init` is the one line that
        // does; delete it and this is the assertion that goes red, while every
        // test above still passes.
        val controller = detachedController()
        withLoop(controller.frames) {
            controller.terminal.feed("from the machine\r\n")
            settle()
            frame()
            assertTrue(lastText.contains("from the machine"), lastText)
        }
        controller.close()
    }

    @Test
    fun `typing repaints even when nothing comes back`() {
        // `send` jumps the view to the bottom, and that is a change to what is
        // painted with no change to the terminal. Somebody typing while
        // scrolled back would otherwise stay looking at the history until the
        // far end echoed something — and while detached nothing ever will.
        val controller = detachedController()
        repeat(40) { controller.terminal.feed("line $it\r\n") }
        controller.viewport.scrollBy(-30)
        withLoop(controller.frames) {
            assertFalse(lastText.contains("line 39"), "the view starts scrolled back")
            assertFalse(controller.send("x".toByteArray()), "nothing is attached, so it is dropped")
            settle()
            frame()
            assertTrue(lastText.contains("line 39"), "typing must bring the view back: $lastText")
        }
        controller.close()
    }

    /**
     * A controller that is never started, so nothing connects.
     *
     * Its pump thread is built and not started; `connect` is here only because
     * the constructor takes it. Everything asserted through it is what happens
     * on THIS side of the socket.
     */
    private fun detachedController(): TerminalController = TerminalController(
        sessionId = 1,
        connect = { error("this test never connects") },
        cols = 20,
        rows = 6,
    )

    private companion object {
        /**
         * Enough turns for the loop to reach its next suspension point.
         *
         * A number rather than a condition because the condition being waited
         * for is "nothing more will happen", which cannot be observed
         * directly. Both coroutines are on one thread, so each `yield` runs
         * the loop until it suspends; anything beyond two is slack.
         */
        const val SETTLE_TURNS = 20
    }
}

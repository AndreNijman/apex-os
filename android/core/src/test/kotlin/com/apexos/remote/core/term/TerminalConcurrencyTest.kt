package com.apexos.remote.core.term

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicReference

/**
 * The renderer and the socket pump are different threads, and they both touch
 * the screen.
 *
 * `Screen` moves whole [Line] objects between its grid and its scrollback with
 * `removeAt`/`add`. A draw pass walking `lineAt(i)` while that happens does not
 * read a slightly stale screen; it reads a torn one, or an index that stopped
 * existing between the bounds check and the read. On a phone the reader is the
 * main thread and the writer is a socket, so this is not a hypothetical race —
 * it is the normal case, every time output arrives while a frame is being
 * drawn.
 *
 * So the property is: a reader that takes [Terminal.read] never sees an
 * exception and never sees a half-applied write.
 */
class TerminalConcurrencyTest {
    @Test
    @Timeout(30)
    fun `a reader under the lock never sees a torn screen while bytes arrive`() {
        val t = Terminal(80, 24)
        val stop = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>(null)
        var reads = 0L

        val writer = Thread({
            try {
                var n = 0
                while (stop.count > 0L) {
                    t.feed("line ${n++} with some ${Ansi.ESC}[31mcolour${Ansi.ESC}[0m in it\r\n")
                }
            } catch (e: Throwable) {
                failure.compareAndSet(null, e)
            }
        }, "pump")
        writer.isDaemon = true

        val reader = Thread({
            try {
                while (stop.count > 0L) {
                    // Exactly what a draw pass does: walk every line, read
                    // every cell, all inside one lock.
                    val cells = t.read { s ->
                        var count = 0
                        for (i in 0 until s.totalLines) {
                            val line = s.lineAt(i) ?: continue
                            for (c in 0 until line.cols) {
                                line.cellAt(c)
                                count++
                            }
                        }
                        count
                    }
                    assertTrue(cells >= 0)
                    reads++
                }
            } catch (e: Throwable) {
                failure.compareAndSet(null, e)
            }
        }, "renderer")
        reader.isDaemon = true

        writer.start()
        reader.start()
        Thread.sleep(500)
        stop.countDown()
        writer.join(5_000)
        reader.join(5_000)

        assertEquals(null, failure.get(), "a concurrent read threw: ${failure.get()}")
        assertTrue(reads > 10, "the reader only managed $reads passes; the test proved little")
    }

    @Test
    @Timeout(30)
    fun `a resize from one thread does not tear a read on another`() {
        val t = Terminal(80, 24)
        t.feed("some content\r\nand more\r\n")
        val stop = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>(null)

        val resizer = Thread({
            try {
                var w = 20
                while (stop.count > 0L) {
                    t.resize(w, 10 + (w % 7))
                    w = if (w >= 120) 20 else w + 1
                }
            } catch (e: Throwable) {
                failure.compareAndSet(null, e)
            }
        }, "rotator")
        resizer.isDaemon = true

        val reader = Thread({
            try {
                while (stop.count > 0L) {
                    t.read { s ->
                        for (i in 0 until s.totalLines) {
                            val line = s.lineAt(i) ?: continue
                            // The tearing this catches: `cols` read from the
                            // screen while the line still has the old array.
                            for (c in 0 until s.cols) line.cellAt(c)
                        }
                    }
                }
            } catch (e: Throwable) {
                failure.compareAndSet(null, e)
            }
        }, "renderer")
        reader.isDaemon = true

        resizer.start()
        reader.start()
        Thread.sleep(400)
        stop.countDown()
        resizer.join(5_000)
        reader.join(5_000)
        assertEquals(null, failure.get(), "a read during a resize threw: ${failure.get()}")
    }
}

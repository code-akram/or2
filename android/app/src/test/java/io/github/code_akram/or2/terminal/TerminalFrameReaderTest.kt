package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalModes
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class TerminalFrameReaderTest {
    private fun frame(sequence: ULong) = TerminalFrame(sequence, 1u, 1u, false, emptyList(), emptyList(),
        null, 0u, Scrollback(1u, 0u), TerminalModes(false, false))

    @Test fun notificationsCoalesceAndTakenFramesCannotOvertakeTheirBase() = runTest {
        var reads = 0
        var ready = 0
        val reader = TerminalFrameReader(this, { { frame((++reads).toULong()) } }, { ready++ },
            { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        repeat(100) { reader.request() }
        advanceUntilIdle()
        assertEquals(1, reads)
        assertEquals(1, ready)
        repeat(100) { reader.request() }
        advanceUntilIdle()
        assertEquals("Only one taken frame may wait for a display tick", 1, reads)
        assertEquals(1uL, reader.take()!!.frame.sequence)
        advanceUntilIdle()
        assertEquals(2, reads)
        assertEquals(2uL, reader.take()!!.frame.sequence)
        advanceUntilIdle()
        assertEquals(2, reads)
    }

    @Test fun detachRetainsAnInFlightFinalFrameAndDoesNotDrainMore() = runTest {
        var reads = 0
        var ready = 0
        val reader = TerminalFrameReader(this, { { if (++reads == 1) frame(1u) else null } }, { ready++ },
            { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        reader.stop()
        repeat(10) { reader.request() }
        advanceUntilIdle()
        assertEquals(1, reads)
        assertEquals(0, ready)
        reader.start()
        assertEquals(1, ready)
        assertEquals(1uL, reader.take()!!.frame.sequence)
        advanceUntilIdle()
        assertEquals(2, reads)
        assertEquals(1, ready)
        assertNull(reader.take())
    }

    @Test fun detachRetainsAnAlreadyPendingFrameUntilItIsConsumed() = runTest {
        var reads = 0
        var ready = 0
        val reader = TerminalFrameReader(this, { { frame((++reads).toULong()) } }, { ready++ },
            { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        advanceUntilIdle()
        assertEquals(1, ready)
        reader.stop()
        repeat(10) { reader.request() }
        advanceUntilIdle()
        assertEquals(1, reads)
        reader.start()
        assertEquals("Reattach reschedules the retained frame", 2, ready)
        advanceUntilIdle()
        assertEquals("Reattach must not take a frame ahead of its retained base", 1, reads)
        assertEquals(1uL, reader.take()!!.frame.sequence)
        advanceUntilIdle()
        assertEquals(2, reads)
        assertEquals(2uL, reader.take()!!.frame.sequence)
    }

    @Test fun attachmentBeforeBindingRetainsDemandWithoutPolling() = runTest {
        var bound = false
        var preparations = 0
        var reads = 0
        var ready = 0
        val reader = TerminalFrameReader(this, {
            preparations++
            if (bound) ({ reads++; frame(1u) }) else null
        }, { ready++ }, { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        advanceUntilIdle()
        assertEquals(1, preparations)
        assertEquals(0, reads)
        bound = true
        reader.request() // Binding/resize/full snapshot publishes through the normal notification.
        advanceUntilIdle()
        assertEquals(2, preparations)
        assertEquals(1, reads)
        assertEquals(1, ready)
        assertEquals(1uL, reader.take()!!.frame.sequence)
        advanceUntilIdle()
        assertEquals(1, reads)
    }

    @Test fun anEmptyReadWithoutAnotherNotificationDoesNotPoll() = runTest {
        var reads = 0
        val reader = TerminalFrameReader(this, { { reads++; null } }, { fail("Empty read became ready") },
            { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        advanceUntilIdle()
        assertEquals(1, reads)
        assertNull(reader.take())
        advanceUntilIdle()
        assertEquals(1, reads)
        reader.request()
        advanceUntilIdle()
        assertEquals(2, reads)
    }

    @Test fun aNotificationRacingAnEmptyReadIsNotLost() = runTest {
        var reads = 0
        var ready = 0
        lateinit var reader: TerminalFrameReader
        reader = TerminalFrameReader(this, { {
            if (++reads == 1) {
                launch { reader.request() }
                null
            } else frame(2u)
        } }, { ready++ }, { fail("Live source destroyed") }, StandardTestDispatcher(testScheduler))
        reader.start()
        advanceUntilIdle()
        assertEquals(2, reads)
        assertEquals(1, ready)
        assertEquals(2uL, reader.take()!!.frame.sequence)
    }

    @Test fun destroyedBorrowedHandleIsReportedWithoutPollingOrDrawing() = runTest {
        var alive = true
        var reads = 0
        var destroyed = 0
        val reader = TerminalFrameReader(this, {
            if (alive) ({ reads++; throw IllegalStateException("destroyed") }) else null
        }, { fail("Destroyed source produced a frame") }, { alive = false; destroyed++ },
            StandardTestDispatcher(testScheduler))
        reader.start()
        advanceUntilIdle()
        reader.request()
        advanceUntilIdle()
        assertEquals(1, reads)
        assertEquals(1, destroyed)
        assertNull(reader.take())
    }
}

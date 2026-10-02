package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/** Wheel-aware scrolling (contracts.md): the route of a swipe, the target scroller and the button. */
@OptIn(ExperimentalCoroutinesApi::class)
class TargetScrollerTest {
    private val plain = TerminalModes(mouseTracking = false, alternateScreen = false)
    private val alternate = TerminalModes(mouseTracking = false, alternateScreen = true)
    private val mouse = TerminalModes(mouseTracking = true, alternateScreen = true)
    private val tmux = TerminalTarget.Tmux("work")
    private val herdr = TerminalTarget.Herdr(null, "w1:p1")

    @Test fun mouseTrackingSendsWheelEventsWhateverTheTarget() {
        for (target in listOf(TerminalTarget.Shell, tmux, herdr)) {
            assertEquals(ScrollRoute.WHEEL, scrollRoute(mouse, target))
            assertEquals(ScrollRoute.WHEEL, scrollRoute(mouse.copy(alternateScreen = false), target))
        }
    }

    @Test fun withoutTheMouseTmuxAndHerdrScrollTheirOwnHistoryAndAShellItsViewport() {
        assertEquals(ScrollRoute.TMUX, scrollRoute(alternate, tmux))
        // mosh never relays the alternate screen: tmux over mosh is on the primary screen here.
        assertEquals(ScrollRoute.TMUX, scrollRoute(plain, tmux))
        assertEquals(ScrollRoute.HERDR, scrollRoute(alternate, herdr))
        assertEquals(ScrollRoute.HERDR, scrollRoute(plain, herdr))
        assertEquals(ScrollRoute.VIEWPORT, scrollRoute(plain, TerminalTarget.Shell))
        // A full-screen program without the mouse keeps today's behaviour (arrow keys, in Rust).
        assertEquals(ScrollRoute.VIEWPORT, scrollRoute(alternate, TerminalTarget.Shell))
    }

    @Test fun theButtonShowsAboveTheViewportsBottomOrWhileATargetIsScrolledUp() {
        // 10 rows on screen: at the bottom the first visible row is total - 10.
        assertFalse(scrollToBottomVisible(Scrollback(10u, 0u), plain, 10, targetAway = false))
        assertFalse(scrollToBottomVisible(Scrollback(110u, 100u), plain, 10, targetAway = false))
        assertTrue(scrollToBottomVisible(Scrollback(110u, 99u), plain, 10, targetAway = false))
        assertTrue(scrollToBottomVisible(Scrollback(110u, 0u), plain, 10, targetAway = false))
        // The alternate screen has no scrollback of its own.
        assertFalse(scrollToBottomVisible(Scrollback(110u, 0u), alternate, 10, targetAway = false))
        // No grid yet.
        assertFalse(scrollToBottomVisible(Scrollback(0u, 0u), plain, 0, targetAway = false))
        assertTrue(scrollToBottomVisible(Scrollback(10u, 0u), alternate, 10, targetAway = true))
    }

    /** A `scroll_target` stand-in that records each call and returns when the test releases it. */
    private class Calls {
        val sent = mutableListOf<TargetScroll>()
        var gate: CompletableDeferred<Unit>? = null
        var failure: Exception? = null
        suspend fun send(scroll: TargetScroll) {
            sent += scroll
            gate?.await()
            failure?.let { throw it }
        }
    }

    private fun TestScope.scroller(calls: Calls, away: MutableList<Boolean> = mutableListOf()) =
        TargetScroller(this, calls::send, onAwayChanged = { away += it })

    @Test fun swipesAreCoalescedWithOneCallInFlight() = runTest {
        val gate = CompletableDeferred<Unit>()
        val calls = Calls().apply { this.gate = gate }
        val away = mutableListOf<Boolean>()
        val scroller = scroller(calls, away)
        scroller.scroll(-3)
        runCurrent()
        assertEquals(listOf<TargetScroll>(TargetScroll.Up(3u)), calls.sent)
        // While it is in flight the deltas add up, down and up together.
        scroller.scroll(-4)
        scroller.scroll(-2)
        scroller.scroll(1)
        runCurrent()
        assertEquals(1, calls.sent.size)
        assertEquals(8L, scroller.awayLines)
        assertFalse(scroller.idle)
        // The first call returns: the summed one follows, alone.
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(3u), TargetScroll.Up(5u)), calls.sent)
        assertEquals(listOf(true), away)
        assertTrue(scroller.idle)
    }

    @Test fun downIsSentOnlyWhileScrolledUpAndReachingTheBottomEndsAway() = runTest {
        val calls = Calls()
        val away = mutableListOf<Boolean>()
        val scroller = scroller(calls, away)
        scroller.scroll(5)
        runCurrent()
        assertTrue("nothing to scroll down at the bottom", calls.sent.isEmpty())
        scroller.scroll(-2)
        runCurrent()
        scroller.scroll(1)
        runCurrent()
        assertTrue(scroller.away)
        scroller.scroll(6)
        runCurrent()
        assertFalse(scroller.away)
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Down(1u), TargetScroll.Down(6u)), calls.sent)
        assertEquals(listOf(true, false), away)
    }

    @Test fun inputAtTheBottomGoesOutAtOnce() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        val typed = mutableListOf<String>()
        scroller.input { typed += "a" }
        assertEquals(listOf("a"), typed)
        assertTrue(calls.sent.isEmpty())
    }

    @Test fun inputWhileScrolledUpSendsBottomFirstAndGoesOutAfterIt() = runTest {
        val calls = Calls()
        val away = mutableListOf<Boolean>()
        val scroller = scroller(calls, away)
        scroller.scroll(-10)
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        val typed = mutableListOf<String>()
        scroller.input { typed += "k" }
        scroller.input { typed += "e" }
        assertFalse(scroller.away)
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(10u), TargetScroll.Bottom), calls.sent)
        assertTrue("held until tmux has left copy mode", typed.isEmpty())
        // More typing while the Bottom is in flight queues behind it, in order.
        scroller.input { typed += "y" }
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("k", "e", "y"), typed)
        assertEquals(2, calls.sent.size)
        assertEquals(listOf(true, false), away)
        assertTrue(scroller.idle)
    }

    @Test fun inputWhileAScrollIsInFlightWaitsForIt() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-1)
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        scroller.scroll(3) // Back down: no longer away, but the call is still out.
        runCurrent()
        assertFalse(scroller.away)
        val typed = mutableListOf<String>()
        scroller.input { typed += "x" }
        assertTrue(typed.isEmpty())
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("x"), typed)
        assertEquals(listOf(TargetScroll.Up(1u), TargetScroll.Down(3u)), calls.sent)
    }

    @Test fun aKeyStaysHeldThroughAFailedBottomAndGoesOutOnceAfterOneSucceeds() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-4)
        runCurrent()
        calls.failure = IllegalStateException("the host did not answer in time")
        val typed = mutableListOf<String>()
        assertTrue(scroller.input { typed += "z" })
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(4u), TargetScroll.Bottom), calls.sent)
        // tmux may still be in copy mode: the key waits, the button shows.
        assertTrue("held while the target may be away", typed.isEmpty())
        assertTrue(scroller.unconfirmed)
        assertTrue(scroller.awayState.value)
        assertFalse(scroller.idle)
        // The retry fails too: still held.
        advanceTimeBy(TargetScroller.RETRY_DELAYS_MS[0])
        runCurrent()
        assertEquals(3, calls.sent.size)
        assertTrue(typed.isEmpty())
        // The next one succeeds: the key goes out, exactly once.
        calls.failure = null
        advanceTimeBy(TargetScroller.RETRY_DELAYS_MS[1])
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(4u), TargetScroll.Bottom, TargetScroll.Bottom, TargetScroll.Bottom), calls.sent)
        assertEquals(listOf("z"), typed)
        assertFalse(scroller.away)
        assertTrue(scroller.idle)
        advanceUntilIdle()
        assertEquals(listOf("z"), typed)
        assertEquals(4, calls.sent.size)
    }

    @Test fun aFailedBottomIsRetriedWithABoundedBackoffWhileInputIsHeld() = runTest {
        val times = mutableListOf<Long>()
        val failing = Calls()
        val timed = TargetScroller(this, { scroll -> times += testScheduler.currentTime; failing.send(scroll) })
        timed.scroll(-1)
        runCurrent()
        failing.failure = IllegalStateException("not connected")
        timed.input { }
        runCurrent()
        advanceTimeBy(20_000)
        runCurrent()
        val bottoms = times.drop(1)
        // At once, then 250 ms, 1 s, 2 s and every 4 s after.
        assertEquals(listOf(0L, 250L, 1_250L, 3_250L, 7_250L, 11_250L, 15_250L, 19_250L), bottoms)
        timed.close()
        advanceTimeBy(60_000)
        assertEquals(8, times.size - 1)
    }

    @Test fun aFailedDownKeepsTheTypingHeldUntilABottomSucceeds() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-3)
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        scroller.scroll(3) // Down to the bottom, as far as this terminal knows...
        runCurrent()
        assertFalse(scroller.away)
        val typed = mutableListOf<String>()
        scroller.input { typed += "x" }
        // ...but the Down fails: the target may not have moved, so the key needs a Bottom that succeeds.
        calls.failure = IllegalStateException("the host did not answer in time")
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(3u), TargetScroll.Down(3u), TargetScroll.Bottom), calls.sent)
        assertTrue(typed.isEmpty())
        assertTrue(scroller.unconfirmed)
        assertTrue(scroller.awayState.value)
        calls.failure = null
        advanceTimeBy(TargetScroller.RETRY_DELAYS_MS[0])
        runCurrent()
        assertEquals(listOf("x"), typed)
        assertFalse(scroller.away)
    }

    @Test fun aFailedUpLeavesThePositionUnconfirmed() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        calls.failure = IllegalStateException("not connected")
        scroller.scroll(-2)
        runCurrent()
        assertTrue(scroller.unconfirmed)
        assertTrue(scroller.away)
        // A swipe back down is sent (how far up is unknown) and does not confirm anything.
        calls.failure = null
        scroller.scroll(2)
        runCurrent()
        assertEquals(0L, scroller.awayLines)
        assertTrue(scroller.away)
        val typed = mutableListOf<String>()
        scroller.input { typed += "u" }
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Down(2u), TargetScroll.Bottom), calls.sent)
        assertEquals(listOf("u"), typed)
        assertFalse(scroller.away)
    }

    @Test fun heldInputWaitsOutAConnectionDropAndGoesOutWhenItIsBack() = runTest {
        val calls = Calls()
        val live = MutableStateFlow(true)
        val scroller = TargetScroller(this, calls::send, live)
        scroller.scroll(-2)
        runCurrent()
        live.value = false
        calls.failure = IllegalStateException("not connected")
        val typed = mutableListOf<String>()
        scroller.input { typed += "a" }
        scroller.input { typed += "b" }
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Bottom), calls.sent)
        // No retries while the connection is down, however long.
        advanceTimeBy(600_000)
        runCurrent()
        assertEquals(2, calls.sent.size)
        assertTrue(typed.isEmpty())
        // Back: the next Bottom goes at once and the input follows it, in order, once.
        calls.failure = null
        live.value = true
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Bottom, TargetScroll.Bottom), calls.sent)
        assertEquals(listOf("a", "b"), typed)
        advanceUntilIdle()
        assertEquals(listOf("a", "b"), typed)
    }

    @Test fun heldInputIsCappedAndTheNewestPastTheCapIsDropped() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-1)
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        val typed = mutableListOf<String>()
        val max = TargetScroller.MAX_HELD_BYTES
        assertTrue(scroller.input(max - 10) { typed += "big" })
        assertFalse("past the cap", scroller.input(11) { typed += "over" })
        assertTrue(scroller.input(10) { typed += "fits" })
        assertEquals(max, scroller.heldSize)
        assertFalse(scroller.input(1) { typed += "full" })
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("big", "fits"), typed)
        assertEquals(0, scroller.heldSize)
        // Room again once released.
        assertTrue(scroller.input(max) { typed += "again" })
        assertEquals("at the bottom it goes at once", "again", typed.last())
    }

    @Test fun inputIsCountedInUtf8Bytes() {
        assertEquals(1 + 2 + 3 + 4, utf8Length("aé€😀"))
        assertEquals(0, utf8Length(""))
    }

    @Test fun closingDropsTheHeldInputAndStopsRetrying() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-1)
        runCurrent()
        calls.failure = IllegalStateException("not connected")
        val typed = mutableListOf<String>()
        scroller.input { typed += "q" }
        runCurrent()
        assertEquals(2, calls.sent.size)
        scroller.close()
        assertEquals(0, scroller.heldSize)
        calls.failure = null
        advanceTimeBy(60_000)
        runCurrent()
        assertEquals("no retry after the close", 2, calls.sent.size)
        assertTrue("dropped, never sent", typed.isEmpty())
        // Nothing is held for a closed terminal: its session refuses what comes.
        scroller.input { typed += "late" }
        assertEquals(listOf("late"), typed)
        scroller.bottom()
        runCurrent()
        assertEquals(2, calls.sent.size)
    }

    @Test fun theButtonStaysWhileUnconfirmedAndTappingItTriesAtOnce() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-5)
        runCurrent()
        calls.failure = IllegalStateException("not connected")
        val typed = mutableListOf<String>()
        scroller.input { typed += "t" }
        runCurrent()
        assertTrue(scroller.awayState.value)
        // The tap: a Bottom now, without waiting for the retry's delay. The button stays while it is out.
        calls.failure = null
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        scroller.bottom()
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(5u), TargetScroll.Bottom, TargetScroll.Bottom), calls.sent)
        assertTrue("still unconfirmed while it is out", scroller.awayState.value)
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("t"), typed)
        assertFalse(scroller.awayState.value)
        // The retry it replaced never fires.
        advanceUntilIdle()
        assertEquals(3, calls.sent.size)
    }

    @Test fun aFailedBottomKeepsTheTargetAwayUntilABottomSucceeds() = runTest {
        val calls = Calls()
        val away = mutableListOf<Boolean>()
        val scroller = scroller(calls, away)
        scroller.scroll(-6)
        runCurrent()
        calls.failure = IllegalStateException("not connected")
        scroller.bottom()
        assertFalse("optimistic while the first Bottom is out", scroller.away)
        runCurrent()
        assertTrue(scroller.away)
        assertTrue(scroller.awayState.value)
        assertEquals(0L, scroller.awayLines)
        // A swipe down is allowed (how far up is unknown) and does not clear it.
        scroller.scroll(2)
        runCurrent()
        assertTrue(scroller.away)
        // The next input sends Bottom first; once it succeeds, the target is live and the input goes out.
        calls.failure = null
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        val typed = mutableListOf<String>()
        scroller.input { typed += "i" }
        runCurrent()
        assertTrue(typed.isEmpty())
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("i"), typed)
        assertFalse(scroller.away)
        assertFalse(scroller.awayState.value)
        assertEquals(listOf(TargetScroll.Up(6u), TargetScroll.Bottom, TargetScroll.Down(2u), TargetScroll.Bottom), calls.sent)
        assertEquals(listOf(true, false, true, false), away)
    }

    @Test fun aSecondBottomQueuedBehindAFailedOneDecides() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-1)
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        calls.failure = IllegalStateException("not connected")
        scroller.bottom()
        runCurrent()
        scroller.scroll(-3)
        scroller.bottom() // Queued behind the one in flight.
        val second = CompletableDeferred<Unit>()
        calls.gate = second
        gate.complete(Unit) // The first fails...
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(1u), TargetScroll.Bottom, TargetScroll.Bottom), calls.sent)
        assertFalse("...but the queued Bottom decides", scroller.unconfirmed)
        calls.failure = null
        second.complete(Unit)
        runCurrent()
        assertFalse(scroller.away)
        assertTrue(scroller.idle)
    }

    @Test fun leavingSendsBottomOnlyWhileAway() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.leave()
        runCurrent()
        assertTrue(calls.sent.isEmpty())
        scroller.scroll(-7)
        runCurrent()
        scroller.leave()
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(7u), TargetScroll.Bottom), calls.sent)
        assertFalse(scroller.away)
    }

    @Test fun aCancelledBottomLeavesTheTargetUnconfirmed() = runTest {
        val calls = Calls()
        val job = Job()
        val scroller = TargetScroller(this + job, calls::send)
        scroller.scroll(-5)
        runCurrent()
        calls.gate = CompletableDeferred()
        scroller.leave()
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(5u), TargetScroll.Bottom), calls.sent)
        job.cancel()
        runCurrent()
        assertTrue(scroller.unconfirmed)
        assertTrue(scroller.awayState.value)
    }

    @Test fun bottomDropsQueuedSwipes() = runTest {
        val calls = Calls()
        val gate = CompletableDeferred<Unit>()
        calls.gate = gate
        val scroller = scroller(calls)
        scroller.scroll(-2)
        runCurrent()
        scroller.scroll(-5)
        scroller.bottom()
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Bottom), calls.sent)
        assertFalse(scroller.away)
    }
}

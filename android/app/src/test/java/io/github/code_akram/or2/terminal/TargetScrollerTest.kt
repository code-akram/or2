package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.Scrollback
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalModes
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
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
        TargetScroller(this, calls::send) { away += it }

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

    @Test fun aFailedCallStillReleasesTheTyping() = runTest {
        val calls = Calls()
        val scroller = scroller(calls)
        scroller.scroll(-4)
        runCurrent()
        calls.failure = IllegalStateException("the host did not answer in time")
        val typed = mutableListOf<String>()
        scroller.input { typed += "z" }
        runCurrent()
        assertEquals(listOf("z"), typed)
        assertEquals(listOf(TargetScroll.Up(4u), TargetScroll.Bottom), calls.sent)
        assertTrue(scroller.idle)
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

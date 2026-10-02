package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.terminal.TerminalSession
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/**
 * A terminal's input and target-scroll state belong to the [ActiveTerminal], not to a view (Codex
 * v0.1.1 review, the two P1s): input through a view still bound to the handle the SSH-to-mosh swap
 * replaced reaches mosh exactly once, and a tmux or herdr target scrolled into its history stays known
 * to be away across the swap, a new view, and the terminal being hidden and shown again.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsTerminalInputTest {
    private val tmux = TerminalTarget.Tmux("main")

    private class Rig(
        val holder: HostConnections, val port: FakePort, val active: ActiveHost, val host: Host, val listener: () -> HostListener,
    )

    /** An AUTO host whose UDP is untested: a tmux terminal opens over SSH with mosh in the background. */
    private suspend fun TestScope.rig(): Rig {
        val host = testHost(transport = TransportPref.AUTO)
        val port = FakePort()
        port.caps = port.caps.copy(moshServer = "/usr/bin/mosh-server")
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler),
            UnconfinedTestDispatcher(testScheduler))
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        return Rig(holder, port, holder.host(host.id)!!, host) { listener!! }
    }

    private fun TestScope.state(rig: Rig, index: Int, state: SessionState) {
        rig.port.terminals[index].second.onStateChanged(state)
        advanceUntilIdle()
    }

    /** The displayed view's session access, bound the way `TerminalScreen` binds it. */
    private fun view(terminal: ActiveTerminal) = TerminalSession().apply { bind(terminal.handle.value!!, terminal.input) }

    /** What a terminal view does for a typed key: through the target scroller, then the session. */
    private fun type(terminal: ActiveTerminal, view: TerminalSession, text: String) {
        val send = { view.call { sendText(text) } }
        terminal.targetScroller?.input { send() } ?: send()
    }

    private fun scrolls(rig: Rig) = rig.port.scrolls.map { it.third }

    // --- input during the SSH-to-mosh swap ---------------------------------------------------

    @Test
    fun inputThroughTheViewStillBoundToSshAfterTheSwapReachesMoshExactlyOnce() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH), rig.port.transports)
        val ssh = rig.port.terminals[0].third
        val mosh = rig.port.terminals[1].third
        state(rig, 0, SessionState.Connected)
        val shown = view(terminal)
        assertTrue(shown.call { sendText("a") })
        assertEquals(listOf("text:a"), ssh.inputs)

        // The background mosh session connects: the handle is published and SSH disconnected in one
        // main-dispatcher turn. The screen has not recomposed: the view on screen is still the SSH one.
        state(rig, 1, SessionState.Connected)
        assertSame(mosh, terminal.handle.value)
        assertTrue("disconnect" in ssh.events)

        // A hardware key, an IME commit, a composer submit and a wheel scroll all reach mosh, once each.
        assertTrue(shown.call { sendText("b") })
        assertTrue(shown.call { sendKey(KeyInput(TerminalKey.Enter, KeyModifiers(false, false, false, false))) })
        assertTrue(shown.call { submitText("go") })
        assertEquals(listOf("text:b", "key:${TerminalKey.Enter}", "submit:go"), mosh.inputs)
        assertEquals(listOf("text:a"), ssh.inputs)
        assertFalse(shown.gone)

        // The old SSH object has been destroyed since (released a turn after the swap): the old view still types into mosh.
        assertTrue(ssh.destroyed)
        assertTrue(shown.call { sendText("c") })
        assertEquals(listOf("text:b", "key:${TerminalKey.Enter}", "submit:go", "text:c"), mosh.inputs)
        // Its frames are over: it took them from its own handle, now destroyed.
        assertNull(shown.takeFrame())
        assertTrue(shown.gone)
        assertTrue(shown.call { sendText("d") })
        assertEquals("text:d", mosh.inputs.last())
        assertEquals(1, mosh.inputs.count { it == "text:d" })
    }

    @Test
    fun inputHeldBehindATargetsBottomAcrossTheSwapGoesToMoshOnce() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        val ssh = rig.port.terminals[0].third
        val mosh = rig.port.terminals[1].third
        state(rig, 0, SessionState.Connected)
        val shown = view(terminal)
        terminal.targetScroller!!.scroll(-5)
        advanceUntilIdle()
        // A key typed while scrolled up waits for the Bottom; the swap lands before it returns.
        rig.port.scrollGate = CompletableDeferred()
        type(terminal, shown, "x")
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(5u), TargetScroll.Bottom), scrolls(rig))
        state(rig, 1, SessionState.Connected)
        rig.port.scrollGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("text:x"), mosh.inputs)
        assertTrue(ssh.inputs.isEmpty())
    }

    // --- target scroll state lives with the terminal ------------------------------------------

    @Test
    fun aScrolledTargetStaysAwayAcrossTheSwapAndTheNewViewShowsTheButton() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        val scroller = terminal.targetScroller!!
        scroller.scroll(-12)
        advanceUntilIdle()
        assertTrue(scroller.awayState.value)

        state(rig, 1, SessionState.Connected)
        // The same terminal, the same scroller: the swap's new view starts scrolled away (the button shows).
        assertSame(scroller, terminal.targetScroller)
        assertTrue(scroller.awayState.value)
        assertEquals(12L, scroller.awayLines)
        // The swap itself sends nothing: the user is still reading the history.
        assertEquals(listOf(TargetScroll.Up(12u)), scrolls(rig))

        // The first key through the new view leaves copy mode first, then reaches mosh.
        val mosh = rig.port.terminals[1].third
        type(terminal, view(terminal), "k")
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(12u), TargetScroll.Bottom), scrolls(rig))
        assertEquals(listOf("text:k"), mosh.inputs)
        assertFalse(scroller.awayState.value)
    }

    @Test
    fun aRecreatedViewOfAScrolledTerminalKeepsItsState() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Herdr("work", null))
        state(rig, 0, SessionState.Connected)
        val ssh = rig.port.terminals[0].third
        val first = view(terminal)
        terminal.targetScroller!!.scroll(-3)
        advanceUntilIdle()
        // The view goes (a configuration change) and another is made for the same handle.
        val second = view(terminal)
        assertTrue(terminal.targetScroller!!.awayState.value)
        type(terminal, second, "q")
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(3u), TargetScroll.Bottom), scrolls(rig))
        assertEquals(listOf("text:q"), ssh.inputs)
        assertNotSame(first, second)
    }

    @Test
    fun hidingAScrolledTerminalReturnsItsTargetToTheLiveScreen() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        val scroller = terminal.targetScroller!!
        scroller.scroll(-8)
        advanceUntilIdle()
        // Minimised, or another terminal selected: the screen leaves composition.
        rig.holder.hideTerminal(terminal)
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(8u), TargetScroll.Bottom), scrolls(rig))
        assertFalse(scroller.awayState.value)
        // Shown again: at the bottom, typing goes straight out.
        val ssh = rig.port.terminals[0].third
        type(terminal, view(terminal), "a")
        assertEquals(listOf("text:a"), ssh.inputs)
        assertEquals(2, rig.port.scrolls.size)
        // A terminal at its bottom sends nothing when hidden.
        rig.holder.hideTerminal(terminal)
        advanceUntilIdle()
        assertEquals(2, rig.port.scrolls.size)
    }

    @Test
    fun aFailedBottomOnHidingKeepsTheTerminalAwaySoTheNextInputSendsBottomFirst() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        val scroller = terminal.targetScroller!!
        scroller.scroll(-4)
        advanceUntilIdle()
        rig.port.scrollFailure = HostException.NotConnected()
        rig.holder.hideTerminal(terminal)
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(4u), TargetScroll.Bottom), scrolls(rig))
        // Shown again: tmux may still be in copy mode, so the button shows.
        assertTrue(scroller.unconfirmed)
        assertTrue(scroller.awayState.value)

        // The next key waits for a Bottom that succeeds, then goes out (never after a failed one).
        rig.port.scrollFailure = null
        rig.port.scrollGate = CompletableDeferred()
        val ssh = rig.port.terminals[0].third
        type(terminal, view(terminal), "y")
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(4u), TargetScroll.Bottom, TargetScroll.Bottom), scrolls(rig))
        assertTrue("held until tmux has left copy mode", ssh.inputs.isEmpty())
        rig.port.scrollGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf("text:y"), ssh.inputs)
        assertFalse(scroller.awayState.value)
    }

    @Test
    fun aBottomInFlightWhenTheTerminalIsHiddenIsNotCancelled() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        val scroller = terminal.targetScroller!!
        scroller.scroll(-2)
        advanceUntilIdle()
        rig.port.scrollGate = CompletableDeferred()
        rig.holder.hideTerminal(terminal)
        advanceUntilIdle()
        // The screen is gone; the call still completes in the holder's scope and leaves the target live.
        rig.port.scrollGate!!.complete(Unit)
        advanceUntilIdle()
        assertTrue(scroller.idle)
        assertFalse(scroller.unconfirmed)
        assertFalse(scroller.awayState.value)
    }

    @Test
    fun inputHeldBehindAFailedBottomWaitsOutAHostDropAndReachesTheSurvivingMoshTerminalOnReconnect() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        state(rig, 1, SessionState.Connected) // Swapped to mosh, which outlives the SSH connection.
        val mosh = rig.port.terminals[1].third
        val scroller = terminal.targetScroller!!
        scroller.scroll(-3)
        advanceUntilIdle()

        // The host connection drops; a key typed meanwhile is held behind a Bottom that fails.
        rig.listener().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        runCurrent()
        rig.port.scrollFailure = HostException.NotConnected()
        type(terminal, view(terminal), "k")
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(3u), TargetScroll.Bottom), scrolls(rig))
        // No retries while there is no connection to retry on, however long; the key stays held.
        advanceTimeBy(600_000)
        runCurrent()
        assertEquals(2, rig.port.scrolls.size)
        assertTrue(mosh.inputs.isEmpty())
        assertTrue(scroller.awayState.value)

        // Reconnected: the Bottom goes at once over the new connection, then the key, once.
        rig.port.scrollFailure = null
        rig.holder.connect(rig.host, byteArrayOf(1))
        rig.port.nativeState = HostState.Connected(0u)
        rig.listener().onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        assertEquals(listOf(TargetScroll.Up(3u), TargetScroll.Bottom, TargetScroll.Bottom), scrolls(rig))
        assertEquals(listOf("text:k"), mosh.inputs)
        assertFalse(scroller.awayState.value)
    }

    @Test
    fun closingTheTerminalDropsItsHeldInputAndStopsRetrying() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        state(rig, 0, SessionState.Connected)
        val ssh = rig.port.terminals[0].third
        val scroller = terminal.targetScroller!!
        scroller.scroll(-2)
        advanceUntilIdle()
        rig.port.scrollFailure = HostException.NotConnected()
        type(terminal, view(terminal), "z")
        runCurrent()
        assertEquals(listOf(TargetScroll.Up(2u), TargetScroll.Bottom), scrolls(rig))
        rig.port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        runCurrent()
        assertTrue(scroller.closed)
        assertEquals(0, scroller.heldSize)
        rig.port.scrollFailure = null
        advanceUntilIdle()
        assertEquals("no retry after the close", 2, rig.port.scrolls.size)
        assertTrue(ssh.inputs.isEmpty())
    }

    @Test
    fun aShellHasNoTargetScroller() = runTest {
        val rig = rig()
        val shell = rig.holder.openTerminal(rig.active, TerminalTarget.Shell)
        assertNull(shell.targetScroller)
        rig.holder.hideTerminal(shell) // Nothing to do.
        assertTrue(rig.port.scrolls.isEmpty())
    }
}

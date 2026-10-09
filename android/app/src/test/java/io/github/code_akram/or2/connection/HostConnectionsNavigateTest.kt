package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.ffi.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/** The gestures' moves (`HostConnections.navigate`) on fakes: what reaches the host, and when nothing does. */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsNavigateTest {
    private val host = testHost()

    private suspend fun TestScope.connectedHolder(port: FakePort): HostConnections {
        val holder = HostConnections(
            { _, _ -> port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
        )
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        return holder
    }

    @Test
    fun swipesMoveTmuxAndHerdrTargetsFromTheFocusedPaneAndAShellIgnoresThem() = runTest {
        val port = FakePort()
        val holder = connectedHolder(port)
        val active = holder.host(host.id)!!
        val shell = holder.openTerminal(active, TerminalTarget.Shell)
        val tmux = holder.openTerminal(active, TerminalTarget.Tmux("work"))
        val herdr = holder.openTerminal(active, TerminalTarget.Herdr("work", "w1:p2"))

        assertFalse(holder.navigate(shell, TargetNav.NextWindow))
        assertTrue("a shell never reaches the host", port.navigations.isEmpty())
        assertTrue(holder.navigate(tmux, TargetNav.NextSession))
        assertTrue(holder.navigate(herdr, TargetNav.Pane(NavDirection.LEFT)))
        // herdr moves start from herdr's focused pane (null), not from the pane the terminal opened on.
        assertEquals(
            listOf(
                Triple(TerminalTarget.Tmux("work"), null, TargetNav.NextSession),
                Triple(TerminalTarget.Herdr("work", "w1:p2"), null, TargetNav.Pane(NavDirection.LEFT)),
            ),
            port.navigations,
        )

        // A failed move has no error to show: false, and the next one still runs.
        port.navigateFailure = HostException.CommandFailed("no tmux client is attached to work")
        assertFalse(holder.navigate(tmux, TargetNav.NextWindow))
        port.navigateFailure = null
        assertTrue(holder.navigate(tmux, TargetNav.PreviousWindow))

        // A host the user disconnected moves nothing.
        port.navigations.clear()
        holder.disconnect(host.id)
        assertFalse(holder.navigate(tmux, TargetNav.NextWindow))
        assertTrue(port.navigations.isEmpty())
        holder.dismissHost(host.id)
    }

    /**
     * The history sheet's read (`HostConnections.readHistory`) asks for 2000 lines of the pane a scroll would move:
     * the terminal's own tmux client, herdr's focused pane. A failure reaches the caller.
     */
    @Test
    fun aHistoryReadAsksForTheTerminalsOwnPaneAndClient() = runTest {
        val port = FakePort()
        port.history = HistoryText("one\ntwo", true)
        val holder = connectedHolder(port)
        val active = holder.host(host.id)!!
        val tmux = holder.openTerminal(active, TerminalTarget.Tmux("work"))
        val herdr = holder.openTerminal(active, TerminalTarget.Herdr("work", "w1:p2"))

        assertEquals(HistoryText("one\ntwo", true), holder.readHistory(tmux))
        holder.readHistory(herdr)
        assertEquals(
            listOf(
                HistoryRead(TerminalTarget.Tmux("work"), null, tmux.handle.value!!.clientId(), 2000u),
                HistoryRead(TerminalTarget.Herdr("work", "w1:p2"), null, null, 2000u),
            ),
            port.historyReads,
        )

        port.historyFailure = HostException.PaneNotFound()
        val failed = runCatching { holder.readHistory(herdr) }.exceptionOrNull()
        assertTrue("$failed", failed is HostException.PaneNotFound)
        holder.dismissHost(host.id)
    }

    /**
     * Two terminals on one tmux session: each gesture carries the client id of the terminal it was
     * made on, so Rust moves that terminal's tmux client and never the other's. herdr and shells carry
     * none.
     */
    @Test
    fun eachTerminalOnTheSameTmuxSessionMovesItsOwnClient() = runTest {
        val port = FakePort()
        val holder = connectedHolder(port)
        val active = holder.host(host.id)!!
        val first = holder.openTerminal(active, TerminalTarget.Tmux("work"))
        val second = holder.openTerminal(active, TerminalTarget.Tmux("work"))
        val herdr = holder.openTerminal(active, TerminalTarget.Herdr("work", null))
        val firstId = first.handle.value!!.clientId()!!
        val secondId = second.handle.value!!.clientId()!!
        assertNotEquals(firstId, secondId)

        assertTrue(holder.navigate(first, TargetNav.NextSession))
        assertTrue(holder.navigate(second, TargetNav.PreviousSession))
        assertTrue(holder.navigate(first, TargetNav.NextWindow))
        assertTrue(holder.navigate(second, TargetNav.Pane(NavDirection.UP)))
        assertTrue(holder.navigate(herdr, TargetNav.NextSession))
        assertEquals(listOf(firstId, secondId, firstId, secondId, null), port.navigationClients)
        holder.dismissHost(host.id)
    }

    /**
     * The SSH-to-mosh swap: until the mosh session replaces the SSH one, the terminal shows the SSH
     * session's tmux client and a gesture moves that one; from the swap on, the mosh session's.
     */
    @Test
    fun aGestureMovesTheClientOfTheSessionTheTerminalShowsAcrossTheMoshSwap() = runTest {
        val port = FakePort()
        port.caps = port.caps.copy(moshServer = "/usr/bin/mosh-server")
        var listener: HostListener? = null
        val holder = HostConnections(
            { _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
            moshServers = MoshServerLedger(MemoryPrefStore()),
        )
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        val terminal = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Tmux("work"))
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH), port.transports)
        val (ssh, mosh) = port.terminals[0].third to port.terminals[1].third
        port.terminals[0].second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()

        // The background mosh session's client exists already, but the terminal still shows SSH's.
        assertTrue(holder.navigate(terminal, TargetNav.NextSession))
        port.terminals[1].second.onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        assertSame(mosh, terminal.handle.value)
        assertTrue(holder.navigate(terminal, TargetNav.NextSession))
        assertEquals(listOf(ssh.clientId(), mosh.clientId()), port.navigationClients)
        assertNotEquals(ssh.clientId(), mosh.clientId())
        holder.dismissHost(host.id)
    }

    @Test
    fun movesOnOneTerminalRunOneAtATimeInOrder() = runTest {
        val port = FakePort()
        val holder = connectedHolder(port)
        val tmux = holder.openTerminal(holder.host(host.id)!!, TerminalTarget.Tmux("work"))
        val gate = CompletableDeferred<Unit>()
        port.navigateGate = gate
        val first = async { holder.navigate(tmux, TargetNav.NextWindow) }
        val second = async { holder.navigate(tmux, TargetNav.PreviousWindow) }
        runCurrent()
        assertEquals("the second move waits for the first", 1, port.events.count { it == "navigate" })
        gate.complete(Unit)
        assertTrue(first.await())
        assertTrue(second.await())
        assertEquals(listOf(TargetNav.NextWindow, TargetNav.PreviousWindow), port.navigations.map { it.third })
        holder.dismissHost(host.id)
    }
}

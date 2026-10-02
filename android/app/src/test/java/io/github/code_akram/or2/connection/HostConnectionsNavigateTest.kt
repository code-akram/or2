package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
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

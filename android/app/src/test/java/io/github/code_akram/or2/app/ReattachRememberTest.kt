package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The terminal screen's remembering effect against the holder's real state: only a terminal that is
 * connected now, and not being closed, becomes the Resume target. A terminal the user ended (or
 * whose shell exited) stays listed with its final frame, and the effect runs again when the
 * activity is recreated or the screen is visited once more: it must not bring the target back, or
 * the next foreground return reopens what was ended on purpose.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ReattachRememberTest {
    private val host = testHost()
    private val pane = TerminalTarget.Herdr("work", "w1:p2")

    private class Rig(val scope: TestScope, val memory: ReattachMemory) {
        val port = FakePort()
        private var listener: HostListener? = null
        val holder = HostConnections(
            { _, l -> listener = l; port }, FakeTrust(),
            StandardTestDispatcher(scope.testScheduler), UnconfinedTestDispatcher(scope.testScheduler),
        ).also { it.userClose = memory }

        suspend fun connect(host: io.github.code_akram.or2.data.Host) {
            holder.connect(host, byteArrayOf(1))
            port.nativeState = HostState.Connected(0u)
            listener!!.onHostStateChanged(HostState.Connected(0u))
            scope.advanceUntilIdle()
        }

        fun state(state: SessionState) {
            port.terminals.last().second.onStateChanged(state)
            scope.advanceUntilIdle()
        }

        /** What `Or2App`'s `LaunchedEffect` does each time it starts for [terminal]. */
        fun effect(terminal: ActiveTerminal) {
            val shownState = terminal.state.value
            val transport = terminal.transport.value
            if (shownState == SessionState.Connected) memory.rememberShown(terminal, transport)
        }
    }

    private suspend fun TestScope.opened(target: TerminalTarget = pane): Pair<Rig, ActiveTerminal> {
        val rig = Rig(this, ReattachMemory(MemoryPrefStore()))
        rig.connect(host)
        val terminal = rig.holder.openTerminal(rig.holder.host(host.id)!!, target)
        return rig to terminal
    }

    /** What the app decides on a foreground return for the host's current state. */
    private fun Rig.decision() = decideReattach(
        memory.last.value,
        holder.terminals.value.map { OpenSession(it.id, it.host.id, it.target, it.state.value !is SessionState.Closed) },
        liveHosts = setOf(host.id), knownHosts = setOf(host.id),
    )

    @Test
    fun aConnectedTerminalIsRememberedAndANotYetConnectedOneIsNot() = runTest {
        val (rig, terminal) = opened()
        rig.effect(terminal)
        assertNull(rig.memory.last.value) // Still connecting: nothing yet.
        rig.state(SessionState.Connected)
        rig.effect(terminal)
        assertEquals(LastTerminal(host.id, pane, terminal.transport.value), rig.memory.last.value)
    }

    @Test
    fun aTerminalTheUserDisconnectedIsNotRememberedAgainWhenTheScreenIsRecreatedOrRevisited() = runTest {
        val (rig, terminal) = opened()
        rig.state(SessionState.Connected)
        rig.effect(terminal)
        assertTrue(rig.memory.last.value != null)

        rig.holder.disconnectTerminal(terminal)
        assertNull(rig.memory.last.value)
        rig.state(SessionState.Closed(CloseReason.Disconnected))
        // The final frame stays listed and shown: the screen's effect starts again (recreation, a revisit).
        rig.effect(terminal)
        assertNull("a closed terminal resurrected its Resume memory", rig.memory.last.value)
        assertEquals(Reattach.None, rig.decision())
    }

    @Test
    fun aCloseProcessedBeforeTheRememberingEffectWins() = runTest {
        val (rig, terminal) = opened()
        rig.state(SessionState.Connected)
        // The user taps Disconnect; Rust has not reported the close yet, and the effect (queued by the
        // earlier Connected) only now runs.
        rig.holder.disconnectTerminal(terminal)
        assertEquals(SessionState.Connected, terminal.state.value)
        rig.effect(terminal)
        assertNull(rig.memory.last.value)
        assertEquals(Reattach.None, rig.decision())
    }

    @Test
    fun aShellThatExitedIsNotRememberedAgainEitherAndAFastExitNeverIs() = runTest {
        val (rig, terminal) = opened()
        rig.state(SessionState.Connected)
        rig.effect(terminal)
        rig.state(SessionState.Closed(CloseReason.RemoteExited(0u)))
        assertNull(rig.memory.last.value)
        rig.effect(terminal) // Recreation or a revisit.
        assertNull(rig.memory.last.value)
        assertEquals(Reattach.None, rig.decision())

        // Connected and RemoteExited are both processed before the effect first runs.
        val rig2 = Rig(this, ReattachMemory(MemoryPrefStore()))
        rig2.connect(host)
        val fast = rig2.holder.openTerminal(rig2.holder.host(host.id)!!, TerminalTarget.Shell)
        rig2.state(SessionState.Connected)
        rig2.state(SessionState.Closed(CloseReason.RemoteExited(0u)))
        rig2.effect(fast)
        assertNull(rig2.memory.last.value)
        assertEquals(Reattach.None, rig2.decision())
    }

    @Test
    fun aDismissedTerminalIsNotRemembered() = runTest {
        val (rig, terminal) = opened()
        rig.state(SessionState.Connected)
        rig.holder.dismissTerminal(terminal)
        rig.effect(terminal)
        assertNull(rig.memory.last.value)
    }

    @Test
    fun losingTheConnectionKeepsTheMemoryButAClosedTerminalNeverSetsIt() = runTest {
        val (rig, terminal) = opened()
        rig.state(SessionState.Connected)
        rig.effect(terminal)
        val remembered = rig.memory.last.value
        // The network took it (not the user): the memory stays, and a revisit changes nothing.
        rig.state(SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        rig.effect(terminal)
        assertEquals(remembered, rig.memory.last.value)

        // A terminal that failed without ever having been remembered stays unremembered.
        val (other, failed) = opened(TerminalTarget.Tmux("x"))
        other.state(SessionState.Connected)
        other.state(SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        other.effect(failed)
        assertNull(other.memory.last.value)
    }
}

package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Reconnecting the SSH connection of a host whose mosh terminals survived the loss must not end
 * them: releasing (or disconnecting) the old native host connection is cancellation in Rust, so the
 * holder keeps that connection while any of its mosh terminals runs, and an explicit disconnect of
 * the host, its deletion and "Disconnect all" still reach those older connections. On fakes; the
 * real FFI is in `HostConnectionsNativeTest`.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsReconnectTest {
    private class Rig(val scope: TestScope, transport: TransportPref = TransportPref.AUTO) {
        val host: Host = testHost(transport = transport)
        val ports = mutableListOf<FakePort>()
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections(
            { _, listener ->
                listeners += listener
                FakePort().also {
                    it.caps = it.caps.copy(moshServer = "/usr/bin/mosh-server")
                    it.nextServerPid = 4242u
                    ports += it
                }
            },
            FakeTrust(), StandardTestDispatcher(scope.testScheduler), UnconfinedTestDispatcher(scope.testScheduler),
            moshServers = MoshServerLedger(MemoryPrefStore()),
        )

        suspend fun connect() {
            holder.connect(host, byteArrayOf(1))
            ports.last().nativeState = HostState.Connected(0u)
            listeners.last().onHostStateChanged(HostState.Connected(0u))
            scope.advanceUntilIdle()
        }

        /** The SSH connection [generation] is lost; its mosh sessions carry on. */
        fun lose(generation: Int = listeners.size - 1) {
            listeners[generation].onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
            scope.advanceUntilIdle()
        }

        fun open(target: TerminalTarget = TerminalTarget.Shell): ActiveTerminal {
            val terminal = holder.openTerminal(holder.host(host.id)!!, target)
            val generation = ports.last()
            generation.terminals.last().second.onStateChanged(SessionState.Connected)
            scope.advanceUntilIdle()
            return terminal
        }

        fun sessionState(port: Int, terminal: Int, state: SessionState) {
            ports[port].terminals[terminal].second.onStateChanged(state)
            scope.advanceUntilIdle()
        }
    }

    @Test
    fun replacingALostHostKeepsItsSurvivingMoshSessionRunning() = runTest {
        val rig = Rig(this)
        rig.connect()
        val terminal = rig.open()
        rig.lose()
        rig.connect()

        val old = rig.ports[0]
        assertFalse("the replacement sent the user-cancel signal to a surviving mosh session", "disconnect" in old.events)
        assertFalse("releasing the old host connection also cancels its surviving mosh sessions", old.destroyed)
        assertEquals(SessionState.Connected, terminal.state.value)
        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
        assertFalse((terminal.handle.value as FakeSession).destroyed)
        assertFalse("disconnect" in (terminal.handle.value as FakeSession).events)
        assertEquals(HostState.Connected(0u), rig.holder.host(rig.host.id)!!.state.value)
    }

    @Test
    fun theRetainedConnectionIsReleasedOnceItsLastMoshSessionHasClosed() = runTest {
        val rig = Rig(this)
        rig.connect()
        val first = rig.open()
        val second = rig.open(TerminalTarget.Tmux("work"))
        rig.lose()
        rig.connect()

        // One of two ends: the other still needs the connection.
        rig.sessionState(0, 0, SessionState.Closed(CloseReason.Disconnected))
        assertFalse(rig.ports[0].destroyed)
        assertEquals(SessionState.Connected, second.state.value)
        // The last one ends: nothing is left to keep the old connection for.
        rig.sessionState(0, 1, SessionState.Closed(CloseReason.RemoteExited(0u)))
        assertEquals(listOf("disconnect", "close"), rig.ports[0].events.takeLast(2))
        assertTrue(rig.ports[0].destroyed)
        assertEquals(SessionState.Closed(CloseReason.Disconnected), first.state.value)
        // The current connection is untouched.
        assertFalse(rig.ports[1].destroyed)
        assertEquals(HostState.Connected(0u), rig.holder.host(rig.host.id)!!.state.value)
    }

    @Test
    fun dismissingTheLastSurvivorReleasesTheRetainedConnection() = runTest {
        val rig = Rig(this)
        rig.connect()
        val terminal = rig.open()
        rig.lose()
        rig.connect()
        assertFalse(rig.ports[0].destroyed)

        rig.holder.dismissTerminal(terminal)
        assertTrue(rig.ports[0].destroyed)
        assertFalse(rig.ports[1].destroyed)
    }

    @Test
    fun anExplicitDisconnectOfTheHostAfterAReconnectReachesTheOlderConnection() = runTest {
        val rig = Rig(this)
        rig.connect()
        val survivor = rig.open()
        rig.lose()
        rig.connect()
        val fresh = rig.open(TerminalTarget.Tmux("fresh"))

        rig.holder.disconnect(rig.host.id)
        assertTrue("disconnect" in rig.ports[0].events)
        assertTrue("disconnect" in rig.ports[1].events)
        // Rust closes the sessions of each connection it disconnects; the fakes do it by hand.
        rig.sessionState(0, 0, SessionState.Closed(CloseReason.Disconnected))
        rig.sessionState(1, 0, SessionState.Closed(CloseReason.Disconnected))
        assertEquals(SessionState.Closed(CloseReason.Disconnected), survivor.state.value)
        assertEquals(SessionState.Closed(CloseReason.Disconnected), fresh.state.value)
        assertTrue(rig.ports[0].destroyed)
    }

    @Test
    fun deletingTheHostAfterAReconnectEndsTheOlderConnectionAndItsSurvivors() = runTest {
        val rig = Rig(this)
        rig.connect()
        val survivor = rig.open()
        rig.lose()
        rig.connect()

        rig.holder.release(rig.host.id, closeTerminals = true)
        assertTrue("disconnect" in rig.ports[0].events)
        assertTrue(rig.ports[0].destroyed)
        assertTrue(rig.ports[1].destroyed)
        assertTrue(rig.holder.terminals.value.none { it === survivor })
        assertTrue((survivor.handle.value as FakeSession).destroyed)
    }

    @Test
    fun dismissingTheHostAfterAReconnectEndsTheOlderConnectionToo() = runTest {
        val rig = Rig(this)
        rig.connect()
        rig.open()
        rig.lose()
        rig.connect()

        // An edit that changes the destination releases the host without closing its terminals.
        rig.holder.release(rig.host.id, closeTerminals = false)
        assertTrue("a destination edit must still end the old connection", "disconnect" in rig.ports[0].events)
        assertTrue(rig.ports[0].destroyed)
        assertTrue(rig.ports[1].destroyed)
    }

    @Test
    fun disconnectAllReachesTheOlderConnection() = runTest {
        val rig = Rig(this)
        rig.connect()
        val survivor = rig.open()
        rig.lose()
        rig.connect()

        rig.holder.disconnectAll()
        assertTrue("disconnect" in (survivor.handle.value as FakeSession).events)
        assertTrue("disconnect" in rig.ports[0].events)
        assertTrue("disconnect" in rig.ports[1].events)
    }

    @Test
    fun aLostHostWithoutSurvivorsIsReplacedAsBefore() = runTest {
        val ssh = Rig(this, TransportPref.SSH)
        ssh.connect()
        ssh.open()
        ssh.sessionState(0, 0, SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset")))) // SSH terminals die with the host.
        ssh.lose()
        ssh.connect()
        assertEquals(listOf("disconnect", "close"), ssh.ports[0].events.takeLast(2))

        val none = Rig(this)
        none.connect()
        none.lose()
        none.connect()
        assertEquals(listOf("disconnect", "close"), none.ports[0].events.takeLast(2))
    }

    @Test
    fun everyConnectionWithSurvivorsIsKeptAcrossSeveralReconnects() = runTest {
        val rig = Rig(this)
        rig.connect()
        rig.open()
        rig.lose()
        rig.connect()
        rig.open(TerminalTarget.Tmux("second"))
        rig.lose()
        rig.connect()

        assertFalse(rig.ports[0].destroyed)
        assertFalse(rig.ports[1].destroyed)
        rig.holder.disconnect(rig.host.id)
        assertTrue("disconnect" in rig.ports[0].events)
        assertTrue("disconnect" in rig.ports[1].events)
        assertTrue("disconnect" in rig.ports[2].events)
    }
}

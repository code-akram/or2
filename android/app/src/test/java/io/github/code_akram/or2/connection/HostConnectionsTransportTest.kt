package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/**
 * M3: which transport a terminal opens over, the AUTO fallback from mosh to SSH on the same
 * `ActiveTerminal`, link health, and the service-facing helpers, on fakes.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsTransportTest {
    private val shell = TerminalTarget.Shell

    private class Rig(val holder: HostConnections, val port: FakePort, val hostListener: HostListener, val host: Host) {
        val active get() = holder.host(host.id)!!
    }

    /** A connected host. [probed] false leaves the capability probe unanswered. */
    private suspend fun TestScope.rig(
        pref: TransportPref = TransportPref.AUTO, moshServer: String? = "/usr/bin/mosh-server", probed: Boolean = true,
    ): Rig {
        val host = testHost(transport = pref)
        val port = FakePort()
        port.caps = port.caps.copy(moshServer = moshServer)
        if (!probed) port.capsFailure = IllegalStateException("probe not answered")
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        return Rig(holder, port, listener!!, host)
    }

    private fun TestScope.sessionListener(rig: Rig, index: Int): SessionListener = rig.port.terminals[index].second

    private fun TestScope.fail(rig: Rig, index: Int, failure: SessionFailure) {
        sessionListener(rig, index).onStateChanged(SessionState.Closed(CloseReason.Failed(failure)))
        advanceUntilIdle()
    }

    @Test
    fun autoOpensMoshWhenTheHostHasMoshServerAndReportsTheRealTransport() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
        assertNull(terminal.note.value)
        assertEquals(TerminalTransport.MOSH, rig.holder.transports().first()[terminal.id])
    }

    @Test
    fun autoOpensSshWithoutMoshServerOrBeforeTheProbeAnswered() = runTest {
        val none = rig(moshServer = null)
        none.holder.openTerminal(none.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), none.port.transports)
        assertNull(none.holder.terminals.value.single().note.value) // No mosh to fall back from: nothing to explain.

        val early = rig(probed = false)
        assertNull(early.active.capabilities.value)
        val terminal = early.holder.openTerminal(early.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), early.port.transports)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
    }

    @Test
    fun explicitPreferencesAreNeverSecondGuessed() = runTest {
        val ssh = rig(TransportPref.SSH)
        ssh.holder.openTerminal(ssh.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), ssh.port.transports) // Even with mosh-server present.

        val mosh = rig(TransportPref.MOSH, moshServer = null)
        val terminal = mosh.holder.openTerminal(mosh.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), mosh.port.transports) // Asked for, so it fails visibly if it must.
        fail(mosh, 0, SessionFailure.NotInstalled("mosh-server"))
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.NotInstalled("mosh-server"))), terminal.state.value)
        assertEquals(1, mosh.port.transports.size) // No silent SSH.
    }

    @Test
    fun rememberedTransportWinsUnderAutoButStillNeedsMoshServer() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, shell, remembered = TerminalTransport.SSH)
        assertEquals(listOf(TerminalTransport.SSH), rig.port.transports)
        val none = rig(moshServer = null)
        none.holder.openTerminal(none.active, shell, remembered = TerminalTransport.MOSH)
        assertEquals(listOf(TerminalTransport.SSH), none.port.transports)
    }

    @Test
    fun autoFallsBackToSshWhenMoshTimesOutAndRemembersItForTheConnection() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        val mosh = rig.port.terminals[0].third
        sessionListener(rig, 0).onLinkHealth(LinkHealth(300uL, 300uL))
        advanceUntilIdle()
        assertEquals(300uL, terminal.linkHealth.value?.sinceHeardMs)

        fail(rig, 0, SessionFailure.TimedOut)

        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), rig.port.transports)
        assertSame(rig.port.terminals[1].third, terminal.handle.value) // The same terminal, a new session.
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        assertEquals(SessionState.Connecting, terminal.state.value) // The mosh failure is not shown.
        assertFalse(terminal.hasConnected.value)
        assertNull(terminal.linkHealth.value)
        assertTrue(terminal.note.value!!.contains("UDP"))
        assertTrue(mosh.destroyed) // The replaced native object is released.
        assertEquals(listOf(terminal), rig.holder.terminals.value)

        // The replaced attempt can no longer speak for the terminal.
        sessionListener(rig, 0).onStateChanged(SessionState.Connected)
        sessionListener(rig, 0).onLinkHealth(LinkHealth(9000uL, 9000uL))
        advanceUntilIdle()
        assertEquals(SessionState.Connecting, terminal.state.value)
        assertNull(terminal.linkHealth.value)

        sessionListener(rig, 1).onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        assertEquals(SessionState.Connected, terminal.state.value)
        assertTrue(terminal.hasConnected.value)

        // Remembered for this connection: the next terminal goes straight to SSH, explained the same way.
        val second = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("work"))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.SSH), rig.port.transports)
        assertEquals(terminal.note.value, second.note.value)
    }

    @Test
    fun aMissingMoshServerAlsoFallsBackAndTheFallbackIsOnlyForTheConnection() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        fail(rig, 0, SessionFailure.NotInstalled("mosh-server"))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), rig.port.transports)
        assertTrue(terminal.note.value!!.startsWith("mosh-server is not installed"))
        // A new connection (here: a new rig) starts with no memory of it.
        val again = rig()
        again.holder.openTerminal(again.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), again.port.transports)
    }

    @Test
    fun otherFailuresAndAfterConnectedNeverFallBack() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        fail(rig, 0, SessionFailure.Unreachable("no route"))
        assertEquals(1, rig.port.transports.size)
        assertTrue(terminal.state.value is SessionState.Closed)

        val connected = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("a"))
        sessionListener(rig, 1).onStateChanged(SessionState.Connected)
        advanceUntilIdle()
        fail(rig, 1, SessionFailure.TimedOut) // Mid-session: mosh was working, so this is a real failure.
        assertEquals(2, rig.port.transports.size)
        assertTrue(connected.state.value is SessionState.Closed)
        assertNull(rig.active.moshFallbackNote)
    }

    @Test
    fun aDisconnectedTerminalDoesNotFallBackAndAFailedRetryShowsTheMoshFailure() = runTest {
        val rig = rig()
        val ended = rig.holder.openTerminal(rig.active, shell)
        rig.holder.disconnectTerminal(ended)
        fail(rig, 0, SessionFailure.TimedOut)
        assertEquals(1, rig.port.transports.size)

        val other = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        rig.port.openFailure = HostException.Closed()
        fail(rig, 1, SessionFailure.TimedOut)
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.TimedOut)), other.state.value)
        assertEquals(TerminalTransport.MOSH, other.transport.value)
        assertNull(rig.active.moshFallbackNote)
    }

    @Test
    fun linkHealthIsKeptPerTerminal() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        assertNull(terminal.linkHealth.value)
        sessionListener(rig, 0).onLinkHealth(LinkHealth(6000uL, 9000uL))
        advanceUntilIdle()
        assertEquals(LinkHealth(6000uL, 9000uL), terminal.linkHealth.value)
        assertEquals("Last heard 6 s ago", linkStaleLabel(terminal.linkHealth.value))
    }

    @Test
    fun disconnectAllEndsEveryTerminalAndHostAndTheUserCloseListenerHearsIt() = runTest {
        val rig = rig()
        val events = mutableListOf<String>()
        rig.holder.userClose = object : UserCloseListener {
            override fun hostClosed(hostId: Long) { events += "host:$hostId" }
            override fun terminalClosed(hostId: Long, target: TerminalTarget) { events += "terminal:$hostId:${target::class.simpleName}" }
        }
        val a = rig.holder.openTerminal(rig.active, shell)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("w"))
        assertTrue(rig.holder.hasOpenSession())

        rig.holder.disconnectAll()

        assertEquals(listOf("terminal:7:Shell", "terminal:7:Tmux", "host:7"), events)
        assertTrue(a.disconnectRequested)
        rig.port.terminals.forEach { assertEquals(listOf("disconnect"), it.third.events) }
        assertEquals(listOf("disconnect"), rig.port.events)
        // Closed sessions stay listed until dismissed, and no longer count as open.
        rig.port.terminals.forEach { it.second.onStateChanged(SessionState.Closed(CloseReason.Disconnected)) }
        advanceUntilIdle()
        assertFalse(rig.holder.hasOpenSession())
        events.clear()
        rig.holder.disconnectAll() // Nothing left to end: no repeated calls.
        assertTrue(events.isEmpty())
    }

    @Test
    fun aRemoteExitIsReportedAsTheUserEndingTheTerminalButAFailureIsNot() = runTest {
        val rig = rig()
        val events = mutableListOf<String>()
        rig.holder.userClose = object : UserCloseListener {
            override fun hostClosed(hostId: Long) = Unit
            override fun terminalClosed(hostId: Long, target: TerminalTarget) { events += "closed:$target" }
        }
        rig.holder.openTerminal(rig.active, shell)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("w"))
        sessionListener(rig, 1).onStateChanged(SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("gone"))))
        advanceUntilIdle()
        assertTrue(events.isEmpty())
        sessionListener(rig, 0).onStateChanged(SessionState.Closed(CloseReason.RemoteExited(0u)))
        advanceUntilIdle()
        assertEquals(listOf("closed:${TerminalTarget.Shell}"), events)
    }

    @Test
    fun aHostIsLostOnlyWhenItWasConnectedAndThenFailed() = runTest {
        val rig = rig()
        assertFalse(rig.active.wasLost)
        rig.hostListener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        advanceUntilIdle()
        assertTrue(rig.active.wasLost)

        val user = rig()
        user.holder.disconnect(user.host.id)
        user.hostListener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("race"))))
        advanceUntilIdle()
        assertFalse(user.active.wasLost) // The user ended it first.
        val clean = rig()
        clean.hostListener.onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertFalse(clean.active.wasLost)
    }

    @Test
    fun awaitCapabilitiesReturnsAtOnceWhenKnownAndGivesUpAfterTheTimeout() = runTest {
        val known = rig()
        known.holder.awaitCapabilities(known.active, timeoutMs = 10_000)
        assertEquals(0L, testScheduler.currentTime) // No waiting.

        val never = rig(probed = false)
        // The failed probe is an answer too: it must not make the caller wait out the timeout.
        assertNotNull(never.active.capabilitiesError.value)
        never.holder.awaitCapabilities(never.active, timeoutMs = 10_000)
        assertEquals(0L, testScheduler.currentTime)
    }
}

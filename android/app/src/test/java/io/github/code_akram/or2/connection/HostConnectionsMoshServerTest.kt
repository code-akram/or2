package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostException
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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A mosh server orphaned by the process's death: its pid is recorded while the session lives,
 * forgotten when the session ends, and stopped over the next connection to the host, on fakes.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsMoshServerTest {
    private val shell = TerminalTarget.Shell
    private val host = testHost()

    /** One app process: a holder over [store] whose every connection is a new [FakePort]. */
    private inner class Proc(
        val scope: TestScope, val store: MemoryPrefStore, private val serverPid: UInt? = 4242u,
        /** Every connection's `stop_mosh_server` fails with it. */
        private val stopFailure: Exception? = null,
    ) {
        val ports = mutableListOf<FakePort>()
        private val listeners = mutableListOf<HostListener>()
        val holder = HostConnections(
            { _, listener ->
                listeners += listener
                FakePort().also {
                    it.caps = it.caps.copy(moshServer = "/usr/bin/mosh-server") // AUTO chooses mosh.
                    it.nextServerPid = serverPid
                    it.stopFailure = stopFailure
                    ports += it
                }
            },
            FakeTrust(), StandardTestDispatcher(scope.testScheduler), UnconfinedTestDispatcher(scope.testScheduler),
            moshServers = MoshServerLedger(store),
        )

        /** The host connects and reaches `Connected` (as after Resume's unlock). */
        suspend fun connect() {
            holder.connect(host, byteArrayOf(1))
            ports.last().nativeState = HostState.Connected(0u)
            listeners.last().onHostStateChanged(HostState.Connected(0u))
            scope.advanceUntilIdle()
        }

        fun lose() {
            listeners.last().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
            scope.advanceUntilIdle()
        }

        fun open(): ActiveTerminal = holder.openTerminal(holder.host(host.id)!!, shell)

        fun sessionState(port: Int, terminal: Int, state: SessionState) {
            ports[port].terminals[terminal].second.onStateChanged(state)
            scope.advanceUntilIdle()
        }

        val recorded get() = MoshServerLedger(store).pids(host.id)
    }

    @Test
    fun aMoshSessionsServerIsRecordedWhenItConnectsAndForgottenWhenTheUserEndsIt() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        // Not before the session connects: a start that never connects is cleaned up in Rust.
        assertEquals(emptyList<UInt>(), process.recorded)
        process.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(4242u), process.recorded)

        process.sessionState(0, 0, SessionState.Closed(CloseReason.Disconnected))
        assertEquals(emptyList<UInt>(), process.recorded)
        assertNull(process.store.getString("mosh_servers"))
    }

    @Test
    fun aServerThatEndedItselfIsForgottenToo() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        process.sessionState(0, 0, SessionState.Closed(CloseReason.RemoteExited(0u)))
        assertEquals(emptyList<UInt>(), process.recorded)
    }

    @Test
    fun aFailedSessionKeepsItsRecordSoTheNextConnectionTriesAgain() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        process.sessionState(0, 0, SessionState.Closed(CloseReason.Failed(SessionFailure.Internal("a screen fault"))))
        assertEquals(listOf(4242u), process.recorded)
    }

    @Test
    fun aMoshSessionThatReportsNoPidRecordsNothing() = runTest {
        val process = Proc(this, MemoryPrefStore(), serverPid = null)
        process.connect()
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(TerminalTransport.MOSH), process.ports[0].transports)
        assertEquals(emptyList<UInt>(), process.recorded)
    }

    @Test
    fun aNewProcessStopsTheServerAnOldOneLeftAndForgetsItOnSuccess() = runTest {
        val store = MemoryPrefStore()
        // The old process: a mosh session connects, then the process dies (nothing is closed or cleared).
        val old = Proc(this, store)
        old.connect()
        old.open()
        old.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(4242u), old.recorded)

        // The new process: the host connects (the Resume path), then the remembered target reopens.
        val fresh = Proc(this, store, serverPid = 5151u)
        fresh.connect()
        assertEquals(listOf(4242u), fresh.ports[0].stopped)
        assertEquals(emptyList<UInt>(), fresh.recorded)
        fresh.open()
        fresh.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(5151u), fresh.recorded) // Only the new server is recorded...
        assertEquals(listOf(4242u), fresh.ports[0].stopped) // ...and it is not stopped.
    }

    @Test
    fun aStopThatFailsKeepsTheRecordForTheNextConnection() = runTest {
        val store = MemoryPrefStore()
        MoshServerLedger(store).record(host.id, 4242u)
        val first = Proc(this, store, stopFailure = HostException.CommandFailed("the host did not answer in time"))
        first.connect()
        assertEquals(listOf(4242u), first.ports[0].stopped)
        assertEquals(listOf(4242u), first.recorded)

        val second = Proc(this, store)
        second.connect()
        assertEquals(listOf(4242u), second.ports[0].stopped)
        assertEquals(emptyList<UInt>(), second.recorded)
    }

    @Test
    fun aReconnectSparesTheServerOfASessionThisProcessStillRunsButStopsAnotherOne() = runTest {
        val store = MemoryPrefStore()
        MoshServerLedger(store).record(host.id, 999u) // An orphan of a dead process on this host.
        val process = Proc(this, store)
        process.connect()
        assertEquals(listOf(999u), process.ports[0].stopped)
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(4242u), process.recorded)

        // The SSH connection is lost; the mosh session carries on. The user reconnects (the chip).
        process.lose()
        process.connect()
        assertTrue("its own live session's server is not an orphan", process.ports[1].stopped.isEmpty())
        assertEquals(listOf(4242u), process.recorded)
    }

    @Test
    fun theOrphanDecisionSparesLiveServersOnly() {
        assertEquals(listOf(999u), orphanedServers(listOf(4242u, 999u), live = setOf(4242u)))
        assertEquals(listOf(4242u, 999u), orphanedServers(listOf(4242u, 999u), live = emptySet()))
        assertEquals(emptyList<UInt>(), orphanedServers(emptyList(), live = setOf(1u)))
    }

    @Test
    fun deletingAHostForgetsItsServers() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        process.holder.release(host.id, closeTerminals = true)
        assertEquals(emptyList<UInt>(), process.recorded)
    }

    @Test
    fun theServersOfOtherHostsAreNotStoppedOverThisConnection() = runTest {
        val store = MemoryPrefStore()
        MoshServerLedger(store).record(99, 777u)
        val process = Proc(this, store)
        process.connect()
        assertTrue(process.ports[0].stopped.isEmpty())
        assertEquals(listOf(777u), MoshServerLedger(store).pids(99))
    }
}

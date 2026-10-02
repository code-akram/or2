package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.app.PrefStore
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
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
        val scope: TestScope, val store: PrefStore, private val serverPid: UInt? = 4242u,
        /** Every connection's `stop_mosh_server` fails with it. */
        private val stopFailure: Exception? = null,
    ) {
        val ports = mutableListOf<FakePort>()
        /** The holder's own record (a second instance over [store] would not see what is written through it). */
        val ledger = MoshServerLedger(store)
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
            moshServers = ledger,
        )

        /** The host connects and reaches `Connected` (as after Resume's unlock). */
        suspend fun connect(target: Host = host) {
            holder.connect(target, byteArrayOf(1))
            ports.last().nativeState = HostState.Connected(0u)
            listeners.last().onHostStateChanged(HostState.Connected(0u))
            scope.advanceUntilIdle()
        }

        fun lose() {
            listeners.last().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
            scope.advanceUntilIdle()
        }

        fun open(target: Host = host): ActiveTerminal = holder.openTerminal(holder.host(target.id)!!, shell)

        /** As Rust reports it: a mosh session names its server (`on_server_pid`) before it can be `Connected`. */
        fun sessionState(port: Int, terminal: Int, state: SessionState) {
            if (state == SessionState.Connected) ports[port].serverStarted(terminal)
            ports[port].terminals[terminal].second.onStateChanged(state)
            scope.advanceUntilIdle()
        }

        val recorded get() = MoshServerLedger(store).pids(host)
    }

    @Test
    fun aMoshSessionsServerIsRecordedWhenRustNamesItAndForgottenWhenTheUserEndsIt() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        assertEquals(emptyList<UInt>(), process.recorded)
        // Rust names the server before the session sends it anything: recorded right there, on Rust's
        // thread, with no main-dispatcher turn in between.
        process.ports[0].serverStarted(0)
        assertEquals(listOf(4242u), process.recorded)
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
    fun aProcessThatDiesBeforeMainHandlesConnectedStillLeavesThePidOnDiskToStop() = runTest {
        val store = DiskPrefStore()
        val old = Proc(this, store)
        old.connect()
        old.open()
        store.flush() // What the connect wrote is on the disk; only what follows is in question.
        // On Rust's thread: the server is named, its first datagram is accepted and `Connected` is posted to
        // the main dispatcher, which is slow to run it (no advance here). Then the process dies: nothing
        // queued runs, and no `apply()` reaches the disk.
        old.ports[0].serverStarted(0)
        old.ports[0].terminals[0].second.onStateChanged(SessionState.Connected)
        val disk = MemoryPrefStore().apply { putString("mosh_servers", store.disk.getString("mosh_servers")) }

        // The next process connects to the host and stops the server, which no client will ever speak to again.
        val fresh = Proc(this, disk)
        fresh.connect()
        assertEquals(listOf(4242u), fresh.ports[0].stopped)
        assertEquals(emptyList<UInt>(), fresh.recorded)
    }

    @Test
    fun aBackgroundAttemptIsRecordedBeforeItConnectsSparedByAReconnectAndForgottenWhenCancelled() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        val terminal = process.holder.openTerminal(process.holder.host(host.id)!!, TerminalTarget.Tmux("main"))
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH), process.ports[0].transports)
        process.sessionState(0, 0, SessionState.Connected) // The SSH terminal (it names no server).
        process.ports[0].serverStarted(1) // The background mosh session's server, still connecting.
        assertEquals(listOf(4242u), process.recorded)

        // The SSH connection drops and comes back while the attempt is still on its way: its server is ours.
        process.lose()
        process.connect()
        assertTrue("a starting session's server is not an orphan", process.ports[1].stopped.isEmpty())

        // The user closes the terminal: the attempt is cancelled, Rust stops its server and says Disconnected.
        process.holder.disconnectTerminal(terminal)
        process.sessionState(0, 1, SessionState.Closed(CloseReason.Disconnected))
        assertEquals(emptyList<UInt>(), process.recorded)
    }

    @Test
    fun aMoshStartThatFailsBeforeConnectingKeepsItsRecordLikeAnyFailure() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.connect()
        process.open()
        process.ports[0].serverStarted(0)
        // AUTO's shell falls back to SSH on the same terminal; the stop Rust tried may not have reached the host.
        process.sessionState(0, 0, SessionState.Closed(CloseReason.Failed(SessionFailure.TimedOut)))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), process.ports[0].transports)
        assertEquals(listOf(4242u), process.recorded)
        // The next connection stops it again: it is no session of this process any more.
        process.lose()
        process.connect()
        assertEquals(listOf(4242u), process.ports[1].stopped)
        assertEquals(emptyList<UInt>(), process.recorded)
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
        MoshServerLedger(store).record(host, 4242u)
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
        MoshServerLedger(store).record(host, 999u) // An orphan of a dead process on this host.
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
    fun anEqualPidOnAnotherHostDoesNotProtectAnOrphan() = runTest {
        val store = MemoryPrefStore()
        val a = testHost(id = 1)
        val b = testHost(id = 2)
        val process = Proc(this, store)
        process.connect(a)
        process.open(a)
        process.sessionState(0, 0, SessionState.Connected) // A's live session runs pid 4242 on host A.
        process.ledger.record(b, 4242u) // B's orphan has the same number, on another machine.
        process.connect(b)
        assertEquals("a pid only identifies a process within its host", listOf(4242u), process.ports[1].stopped)
        assertEquals(emptyList<UInt>(), process.ledger.pids(b))
        assertEquals(listOf(4242u), process.ledger.pids(a)) // A's own live server is untouched.
    }

    @Test
    fun changingTheHostDestinationDoesNotSendOldPidsToTheNewHost() = runTest {
        val process = Proc(this, MemoryPrefStore())
        process.ledger.record(host, 4242u)
        process.holder.release(host.id, closeTerminals = false) // What an edit of the destination does to the connection.
        val edited = host.copy(addresses = listOf(HostEndpoint("different.invalid", 22)))
        process.connect(edited)
        assertTrue("an old server id was sent to an unrelated destination", process.ports[0].stopped.isEmpty())
    }

    @Test
    fun aProcessThatDiedBeforeTheEditNeverSendsItsPidsToTheEditedDestination() = runTest {
        val store = MemoryPrefStore()
        // The old process recorded a server for the old destination and died without any cleanup.
        val old = Proc(this, store)
        old.connect()
        old.open()
        old.sessionState(0, 0, SessionState.Connected)
        assertEquals(listOf(4242u), old.recorded)

        // In the new process the stored host now has another address (same id), and the user connects it.
        // Whatever runs at that address (here: pid 4242 is a live mosh-server of somebody else) is not ours.
        val edited = host.copy(addresses = listOf(HostEndpoint("other.invalid", 2222)))
        val fresh = Proc(this, store)
        fresh.connect(edited)
        assertTrue("an old server id was sent to the edited destination", fresh.ports[0].stopped.isEmpty())
        // The same applies to another login on the same machine.
        val otherUser = host.copy(record = host.record.copy(username = "someone-else"))
        val account = Proc(this, store)
        account.connect(otherUser)
        assertTrue("an old server id was sent to another account", account.ports[0].stopped.isEmpty())
        // The record is kept for the destination it belongs to.
        val same = Proc(this, store)
        same.connect(host)
        assertEquals(listOf(4242u), same.ports[0].stopped)
    }

    @Test
    fun editingTheDestinationOrLoginPurgesTheRecordsAndTheRememberedTerminalButOtherEditsKeepThem() = runTest {
        val store = MemoryPrefStore()
        val memory = io.github.code_akram.or2.app.ReattachMemory(store)
        val remembered = io.github.code_akram.or2.app.LastTerminal(host.id, shell, TerminalTransport.MOSH)
        val process = Proc(this, store)
        process.holder.userClose = memory
        process.connect()
        process.open()
        process.sessionState(0, 0, SessionState.Connected)
        memory.remember(remembered)
        assertEquals(listOf(4242u), process.recorded)

        // Label, key, inbox and transport edits move nothing.
        process.holder.hostEdited(host, host.copy(record = host.record.copy(label = "Renamed", keyId = "another", showInInbox = false)))
        assertEquals(listOf(4242u), process.ledger.allPids(host.id))
        assertEquals(remembered, memory.last.value)

        // A new address list invalidates both, and reverting the edit does not bring them back.
        val moved = host.copy(addresses = listOf(HostEndpoint("other.invalid", 2222)))
        process.holder.hostEdited(host, moved)
        assertEquals(emptyList<UInt>(), process.ledger.allPids(host.id))
        assertNull(memory.last.value)
        process.holder.hostEdited(moved, host)
        assertEquals(emptyList<UInt>(), process.ledger.pids(host))

        // So does another login.
        process.ledger.record(host, 777u)
        memory.remember(remembered)
        process.holder.hostEdited(host, host.copy(record = host.record.copy(username = "someone-else")))
        assertEquals(emptyList<UInt>(), process.ledger.allPids(host.id))
        assertNull(memory.last.value)
    }

    @Test
    fun theIdentityOfAHostIsItsOrderedDestinationsAndLoginOnly() {
        val base = testHost(addresses = listOf(HostEndpoint("a.example", 22), HostEndpoint("b.example", 2222)))
        assertEquals(base.moshIdentity(), base.copy(record = base.record.copy(label = "x", keyId = "k2", showInInbox = false)).moshIdentity())
        assertEquals(base.moshIdentity(), testHost(id = 99, addresses = base.addresses).moshIdentity()) // The id is not part of it.
        assertTrue(base.moshIdentity() != base.copy(addresses = base.addresses.reversed()).moshIdentity())
        assertTrue(base.moshIdentity() != base.copy(addresses = listOf(HostEndpoint("a.example", 22), HostEndpoint("b.example", 2223))).moshIdentity())
        assertTrue(base.moshIdentity() != base.copy(addresses = base.addresses.take(1)).moshIdentity())
        assertTrue(base.moshIdentity() != base.copy(record = base.record.copy(username = "other")).moshIdentity())
        // Concatenation cannot fake another list.
        assertTrue(
            testHost(addresses = listOf(HostEndpoint("ab", 1))).moshIdentity() !=
                testHost(addresses = listOf(HostEndpoint("a", 1), HostEndpoint("b", 1))).moshIdentity(),
        )
    }

    @Test
    fun theServersOfOtherHostsAreNotStoppedOverThisConnection() = runTest {
        val store = MemoryPrefStore()
        MoshServerLedger(store).record(testHost(id = 99), 777u)
        val process = Proc(this, store)
        process.connect()
        assertTrue(process.ports[0].stopped.isEmpty())
        assertEquals(listOf(777u), MoshServerLedger(store).pids(testHost(id = 99)))
    }
}

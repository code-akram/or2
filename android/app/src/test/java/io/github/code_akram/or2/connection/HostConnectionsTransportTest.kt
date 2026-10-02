package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.MoshFailureStore
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
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
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
 * M3: which transport a terminal opens over, the AUTO fallback from mosh to SSH on the same
 * `ActiveTerminal`, link health, and the service-facing helpers, on fakes.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsTransportTest {
    private val shell = TerminalTarget.Shell

    /** A [MoshFailureStore] that records what Room would be told. */
    private class Failures : MoshFailureStore {
        val marks = mutableListOf<Pair<Long, Long>>()
        var cleared = 0
        var failure: Exception? = null
        override suspend fun markMoshFailed(hostId: Long, until: Long) {
            failure?.let { throw it }
            marks += hostId to until
        }
        override suspend fun clearMoshFailure(hostId: Long) { cleared++ }
    }

    private class Rig(val holder: HostConnections, val port: FakePort, val hostListener: HostListener, val host: Host) {
        val active get() = holder.host(host.id)!!
    }

    /**
     * A connected host. [probed] false makes the capability probe fail; [pending] leaves it
     * unanswered (neither capabilities nor an error) until `port.capsGate` completes.
     */
    private suspend fun TestScope.rig(
        pref: TransportPref = TransportPref.AUTO, moshServer: String? = "/usr/bin/mosh-server", probed: Boolean = true,
        pending: Boolean = false, store: MoshFailureStore? = null, clock: () -> Long = { NOW }, failedUntil: Long = 0,
    ): Rig {
        val host = testHost(transport = pref, moshFailedUntil = failedUntil)
        val port = FakePort()
        port.caps = port.caps.copy(moshServer = moshServer)
        if (!probed) port.capsFailure = IllegalStateException("probe not answered")
        if (pending) port.capsGate = CompletableDeferred()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
            store, clock)
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

        // The probe has not answered at all (not failed): the synchronous open still has no mosh-server to go on.
        val early = rig(pending = true)
        assertNull(early.active.capabilities.value)
        assertNull(early.active.capabilitiesError.value)
        val terminal = early.holder.openTerminal(early.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), early.port.transports)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
    }

    @Test
    fun aTapBeforeTheProbeAnsweredWaitsForItSoAutoStillChoosesMosh() = runTest {
        val rig = rig(pending = true)
        val opened = async {
            rig.holder.awaitTransportChoice(rig.active)
            rig.holder.openTerminal(rig.active, shell)
        }
        runCurrent()
        assertFalse(opened.isCompleted) // Waiting for the probe, not opened over SSH.
        assertTrue(rig.port.transports.isEmpty())
        rig.port.capsGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
        assertEquals(TerminalTransport.MOSH, opened.await().transport.value)
    }

    @Test
    fun anExplicitPreferenceNeverWaitsForTheProbe() = runTest {
        for (pref in listOf(TransportPref.SSH, TransportPref.MOSH)) {
            val rig = rig(pref, pending = true)
            rig.holder.awaitTransportChoice(rig.active)
            assertEquals(0L, testScheduler.currentTime)
        }
    }

    @Test
    fun aTransportPreferenceEditedOnALiveConnectionAppliesToTheNextTerminal() = runTest {
        val rig = rig() // AUTO with mosh-server: mosh.
        rig.holder.openTerminal(rig.active, shell)
        rig.holder.setTransport(rig.host.id, TransportPref.SSH)
        val ssh = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("a"))
        assertEquals(TerminalTransport.SSH, ssh.transport.value)
        assertFalse(ssh.fallbackEligible)
        rig.holder.setTransport(rig.host.id, TransportPref.MOSH)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        rig.holder.setTransport(rig.host.id, TransportPref.AUTO)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("c"))
        assertEquals(
            listOf(TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.MOSH, TerminalTransport.MOSH),
            rig.port.transports,
        )
        // The terminals already open keep what they run over.
        assertEquals(TerminalTransport.MOSH, rig.holder.terminals.value.first().transport.value)
        rig.holder.setTransport(999, TransportPref.SSH) // A host without a connection: nothing to do.
    }

    @Test
    fun changingThePreferenceDropsTheMemoryOfAnEarlierMoshFailure() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, shell)
        fail(rig, 0, SessionFailure.TimedOut)
        assertNotNull(rig.active.moshFallbackNote)
        rig.holder.setTransport(rig.host.id, TransportPref.SSH)
        rig.holder.setTransport(rig.host.id, TransportPref.AUTO)
        assertNull(rig.active.moshFallbackNote)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("again"))
        assertEquals(TerminalTransport.MOSH, rig.port.transports.last()) // A fresh decision: mosh is tried again.
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
    fun aMissingTmuxOrHerdrIsNotAMoshFailureAndDoesNotMarkMoshAsRejected() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("work"))
        fail(rig, 0, SessionFailure.NotInstalled("tmux"))
        assertEquals(1, rig.port.transports.size) // No SSH retry that would fail the same way.
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.NotInstalled("tmux"))), terminal.state.value)
        assertNull(rig.active.moshFallbackNote)
        rig.holder.openTerminal(rig.active, shell)
        assertEquals(TerminalTransport.MOSH, rig.port.transports.last()) // Mosh itself was fine.
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

        // A closed session hears nothing: the stale line must not outlive it.
        sessionListener(rig, 0).onStateChanged(SessionState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("gone"))))
        advanceUntilIdle()
        assertNull(terminal.linkHealth.value)
        assertNull(linkStaleLabel(terminal.linkHealth.value))
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

        val failed = rig(probed = false)
        // The failed probe is an answer too: it must not make the caller wait out the timeout.
        assertNotNull(failed.active.capabilitiesError.value)
        failed.holder.awaitCapabilities(failed.active, timeoutMs = 10_000)
        assertEquals(0L, testScheduler.currentTime)

        // A probe that never answers: the caller gives up after the timeout and carries on without it.
        val never = rig(pending = true)
        val before = testScheduler.currentTime
        never.holder.awaitCapabilities(never.active, timeoutMs = 3_000)
        assertEquals(3_000L, testScheduler.currentTime - before)
        assertNull(never.active.capabilities.value)
        assertNull(never.active.capabilitiesError.value)

        // And one that answers within the timeout ends the wait as soon as it does.
        val slow = rig(pending = true)
        val started = testScheduler.currentTime
        val waiting = async { slow.holder.awaitCapabilities(slow.active, timeoutMs = 3_000) }
        advanceTimeBy(1_000)
        slow.port.capsGate!!.complete(Unit)
        waiting.await()
        assertEquals(1_000L, testScheduler.currentTime - started)
        assertNotNull(slow.active.capabilities.value)
    }

    // --- the follow-up: AUTO's 5 s budget and the 24 h memory of a failure ----------------------

    private fun TestScope.timedOut(rig: Rig, index: Int = 0) = fail(rig, index, SessionFailure.TimedOut)

    @Test
    fun autoGivesMoshAFiveSecondBudgetAndAnExplicitChoiceKeepsTheDefault() = runTest {
        val auto = rig()
        auto.holder.openTerminal(auto.active, shell)
        assertEquals(listOf<UInt?>(5_000u), auto.port.budgets)
        assertEquals(AUTO_MOSH_BUDGET_MS, auto.port.budgets.single())

        // Explicit Mosh: no budget (the 15 s default); explicit SSH: none either.
        val mosh = rig(TransportPref.MOSH)
        mosh.holder.openTerminal(mosh.active, shell)
        assertEquals(listOf<UInt?>(null), mosh.port.budgets)
        val ssh = rig(TransportPref.SSH)
        ssh.holder.openTerminal(ssh.active, shell)
        assertEquals(listOf<UInt?>(null), ssh.port.budgets)

        // The SSH retry after a fallback has no budget to give.
        timedOut(auto)
        assertEquals(listOf<UInt?>(5_000u, null), auto.port.budgets)
    }

    @Test
    fun aMoshTimeoutIsRememberedPerHostForTwentyFourHours() = runTest {
        val store = Failures()
        val rig = rig(store = store)
        rig.holder.openTerminal(rig.active, shell)
        timedOut(rig)
        assertEquals(listOf(rig.host.id to NOW + MOSH_PAUSE_MS), store.marks)
        assertEquals(24L * 60 * 60 * 1000, MOSH_PAUSE_MS)
        assertEquals(NOW + MOSH_PAUSE_MS, rig.active.moshPausedUntil)
    }

    @Test
    fun aHostWithAnUnexpiredFailureSkipsMoshUnderAutoWithANoteAndTriesAgainOnceItExpired() = runTest {
        var now = NOW
        val paused = rig(failedUntil = NOW + 1_000, clock = { now })
        val first = paused.holder.openTerminal(paused.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), paused.port.transports) // Straight to SSH, no mosh attempt.
        assertEquals(listOf<UInt?>(null), paused.port.budgets)
        assertEquals(MOSH_PAUSED_NOTE, first.note.value)
        assertFalse(first.fallbackEligible)

        // The memory expires on its own: no clearing, the clock moves past it.
        now = NOW + 1_001
        val again = paused.holder.openTerminal(paused.active, TerminalTarget.Tmux("later"))
        assertEquals(TerminalTransport.MOSH, paused.port.transports.last())
        assertNull(again.note.value)
        assertTrue(again.fallbackEligible)
    }

    @Test
    fun anExplicitMoshPreferenceIgnoresTheMemory() = runTest {
        val rig = rig(TransportPref.MOSH, failedUntil = NOW + 10_000)
        rig.holder.openTerminal(rig.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
    }

    @Test
    fun changingThePreferenceClearsThePersistedPause() = runTest {
        val rig = rig(failedUntil = NOW + 10_000)
        assertEquals(NOW + 10_000, rig.active.moshPausedUntil)
        rig.holder.setTransport(rig.host.id, TransportPref.SSH)
        assertEquals(0L, rig.active.moshPausedUntil)
        rig.holder.setTransport(rig.host.id, TransportPref.AUTO)
        rig.holder.openTerminal(rig.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
    }

    @Test
    fun aMissingMoshServerIsNotRememberedForADayAndAStorageFailureChangesNothingElse() = runTest {
        val store = Failures()
        val missing = rig(store = store)
        missing.holder.openTerminal(missing.active, shell)
        fail(missing, 0, SessionFailure.NotInstalled("mosh-server"))
        assertTrue(store.marks.isEmpty()) // The probe says so on every connection; installing it must just work.
        assertEquals(0L, missing.active.moshPausedUntil)

        val broken = Failures().also { it.failure = IllegalStateException("disk full") }
        val rig = rig(store = broken)
        val terminal = rig.holder.openTerminal(rig.active, shell)
        timedOut(rig)
        // The fallback itself still happened, and this connection remembers it in memory.
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), rig.port.transports)
        assertNotNull(terminal.note.value)
        assertEquals(NOW + MOSH_PAUSE_MS, rig.active.moshPausedUntil)
    }

    @Test
    fun aTimeoutIsRememberedEvenIfTheSshRetryCannotStart() = runTest {
        val store = Failures()
        val rig = rig(store = store)
        val terminal = rig.holder.openTerminal(rig.active, shell)
        // Coupled SSH and UDP loss: the connection is gone just before the fallback's own open.
        rig.port.openFailure = HostException.Closed()
        timedOut(rig)
        assertEquals(NOW + MOSH_PAUSE_MS, rig.active.moshPausedUntil)
        assertEquals(listOf(rig.host.id to NOW + MOSH_PAUSE_MS), store.marks)
        // The mosh failure is shown as it was, and the retry is not claimed.
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.TimedOut)), terminal.state.value)
        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
        assertNull(terminal.note.value)
        assertNull(rig.active.moshFallbackNote)
        // The next AUTO terminal on a connection that works goes straight to SSH.
        rig.port.openFailure = null
        val next = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("later"))
        assertEquals(TerminalTransport.SSH, rig.port.transports.last())
        assertEquals(MOSH_PAUSED_NOTE, next.note.value)
    }

    @Test
    fun aTimeoutOfATerminalThatIsNoLongerOwnedIsNotRemembered() = runTest {
        val store = Failures()
        val ended = rig(store = store)
        val terminal = ended.holder.openTerminal(ended.active, shell)
        ended.holder.disconnectTerminal(terminal) // The user ended it: not mosh's failure.
        timedOut(ended)
        assertTrue(store.marks.isEmpty())
        assertEquals(0L, ended.active.moshPausedUntil)

        // A destination edit released the connection: whatever its terminals say, the memory is
        // about a destination that no longer exists.
        val edited = rig(store = store)
        edited.holder.openTerminal(edited.active, shell)
        val old = edited.active
        edited.holder.release(edited.host.id, closeTerminals = false)
        timedOut(edited)
        assertTrue(store.marks.isEmpty())
        assertEquals(0L, old.moshPausedUntil)
    }

    @Test
    fun aFallbackWithoutAStoreStillWorks() = runTest {
        val rig = rig() // No store: nothing is persisted, the connection still remembers.
        rig.holder.openTerminal(rig.active, shell)
        timedOut(rig)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("x"))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.SSH), rig.port.transports)
    }

    @Test
    fun clipboardWritesArePassedOnWithTheTerminalOnlyFromItsCurrentSession() = runTest {
        val rig = rig()
        val copies = mutableListOf<Pair<Long, String>>()
        rig.holder.clipboardWrite = { id, text -> copies += id to text }
        val terminal = rig.holder.openTerminal(rig.active, shell)
        sessionListener(rig, 0).onClipboardWrite("from mosh")
        advanceUntilIdle()
        assertEquals(listOf(terminal.id to "from mosh"), copies)

        fail(rig, 0, SessionFailure.TimedOut) // AUTO falls back to SSH: a new session for the same terminal.
        sessionListener(rig, 0).onClipboardWrite("stale")
        sessionListener(rig, 1).onClipboardWrite("from ssh")
        advanceUntilIdle()
        assertEquals(listOf(terminal.id to "from mosh", terminal.id to "from ssh"), copies)
    }

    private companion object {
        const val NOW = 1_800_000_000_000L
    }
}

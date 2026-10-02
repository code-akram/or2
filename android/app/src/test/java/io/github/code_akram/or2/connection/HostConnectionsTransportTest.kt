package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
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
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
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
 * Which transport a terminal opens over (v0.1.1 "Instant opens": the choice table, the per-connection
 * UDP verdict, the background mosh attempt and the swap, the shell's budget), the AUTO fallback from
 * mosh to SSH on the same `ActiveTerminal`, link health, and the service-facing helpers, on fakes.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HostConnectionsTransportTest {
    private val shell = TerminalTarget.Shell
    private val tmux = TerminalTarget.Tmux("main")

    private class Rig(val holder: HostConnections, val port: FakePort, val hostListener: HostListener, val host: Host, val ledger: MoshServerLedger) {
        val active get() = holder.host(host.id)!!
    }

    /**
     * A connected host. [probed] false makes the capability probe fail; [pending] leaves it
     * unanswered until `port.capsGate` completes, and [moshPending] does the same for `mosh_server()`
     * (`port.moshServerGate`). The program probe's round trip is [roundTripMs] of virtual time.
     */
    private suspend fun TestScope.rig(
        pref: TransportPref = TransportPref.AUTO, moshServer: String? = "/usr/bin/mosh-server", probed: Boolean = true,
        pending: Boolean = false, moshPending: Boolean = false, failedUntil: Long = 0, roundTripMs: Long = 0,
    ): Rig {
        val host = testHost(transport = pref, moshFailedUntil = failedUntil)
        val port = FakePort()
        port.caps = port.caps.copy(moshServer = moshServer)
        if (!probed) port.capsFailure = IllegalStateException("probe not answered")
        if (pending) port.capsGate = CompletableDeferred()
        port.moshServerGate = if (moshPending) CompletableDeferred() else if (roundTripMs > 0) {
            CompletableDeferred<Unit>().also { gate -> launch { kotlinx.coroutines.delay(roundTripMs); gate.complete(Unit) } }
        } else null
        var listener: HostListener? = null
        val ledger = MoshServerLedger(MemoryPrefStore())
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler),
            monotonicMs = { testScheduler.currentTime }, moshServers = ledger)
        holder.connect(host, byteArrayOf(1))
        port.nativeState = HostState.Connected(0u)
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        return Rig(holder, port, listener!!, host, ledger)
    }

    private fun TestScope.sessionListener(rig: Rig, index: Int): SessionListener = rig.port.terminals[index].second

    private fun TestScope.state(rig: Rig, index: Int, state: SessionState) {
        sessionListener(rig, index).onStateChanged(state)
        advanceUntilIdle()
    }

    private fun TestScope.fail(rig: Rig, index: Int, failure: SessionFailure) = state(rig, index, SessionState.Closed(CloseReason.Failed(failure)))

    /** Counts what a terminal's view would be told to draw. */
    private fun TestScope.frames(terminal: ActiveTerminal): () -> Int {
        var count = 0
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { terminal.frameReady.collect { count++ } }
        return { count }
    }

    // --- the choice on a live connection -----------------------------------------------------

    @Test
    fun autoShellOpensMoshOnABudgetFromTheProgramProbesRoundTrip() = runTest {
        val rig = rig()
        assertEquals(MoshServerAnswer("/usr/bin/mosh-server", 0), rig.active.moshServer.value)
        val terminal = rig.holder.openTerminal(rig.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
        assertEquals(listOf<UInt?>(700u), rig.port.budgets) // max(700 ms, 6 x 0 ms)
        assertTrue(terminal.fallbackEligible)
        assertEquals(TerminalTransport.MOSH, rig.holder.transports().first()[terminal.id])

        val slow = rig(roundTripMs = 250)
        assertEquals(250L, slow.active.moshServer.value!!.roundTripMs)
        slow.holder.openTerminal(slow.active, shell)
        assertEquals(listOf<UInt?>(1_500u), slow.port.budgets)
    }

    @Test
    fun autoShellOpensSshWithoutMoshServerOrBeforeTheProgramProbeAnswered() = runTest {
        val none = rig(moshServer = null)
        none.holder.openTerminal(none.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), none.port.transports)

        // `mosh_server()` has not answered: the open does not wait for it.
        val early = rig(moshPending = true)
        assertNull(early.active.moshServer.value)
        val terminal = early.holder.openTerminal(early.active, shell)
        assertEquals(listOf(TerminalTransport.SSH), early.port.transports)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
    }

    @Test
    fun theTransportChoiceAwaitsTheProgramProbeOnlyWhenTheTapConnectedTheHost() = runTest {
        // A tap on a live host (an inbox tap) never waits, even while the answer is outstanding.
        val live = rig(moshPending = true)
        val before = testScheduler.currentTime
        live.holder.awaitTransportChoice(live.active)
        live.holder.awaitTransportChoice(live.active, connectedInThisTap = false)
        assertEquals(before, testScheduler.currentTime)

        // A Resume that connected the host waits for `mosh_server()` alone (not capabilities()), and
        // its shell then chooses mosh.
        val resumed = rig(moshPending = true, pending = true)
        val opened = async {
            resumed.holder.awaitTransportChoice(resumed.active, connectedInThisTap = true)
            resumed.holder.openTerminal(resumed.active, shell)
        }
        runCurrent()
        assertFalse(opened.isCompleted)
        resumed.port.moshServerGate!!.complete(Unit)
        advanceUntilIdle()
        assertEquals(TerminalTransport.MOSH, opened.await().transport.value)
        assertNull(resumed.active.capabilities.value) // The full probe is still out: never waited for.

        // A failed query ends the wait too, and a query that never answers is given up on.
        val failing = rig(moshPending = true)
        failing.port.moshServerFailure = HostException.CommandFailed("no channel")
        failing.port.moshServerGate!!.complete(Unit)
        advanceUntilIdle()
        val start = testScheduler.currentTime
        failing.holder.awaitTransportChoice(failing.active, connectedInThisTap = true)
        assertEquals(start, testScheduler.currentTime)
        assertNull(failing.active.moshServer.value)
        val never = rig(moshPending = true)
        val waitFrom = testScheduler.currentTime
        never.holder.awaitTransportChoice(never.active, connectedInThisTap = true, timeoutMs = 3_000)
        assertEquals(3_000L, testScheduler.currentTime - waitFrom)
    }

    @Test
    fun anExplicitPreferenceNeverWaits() = runTest {
        for (pref in listOf(TransportPref.SSH, TransportPref.MOSH)) {
            val rig = rig(pref, moshPending = true)
            val before = testScheduler.currentTime
            rig.holder.awaitTransportChoice(rig.active, connectedInThisTap = true)
            assertEquals(before, testScheduler.currentTime)
        }
    }

    @Test
    fun explicitPreferencesAreNeverSecondGuessed() = runTest {
        val ssh = rig(TransportPref.SSH)
        ssh.holder.openTerminal(ssh.active, shell)
        ssh.holder.openTerminal(ssh.active, tmux)
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.SSH), ssh.port.transports) // No background try either.

        val mosh = rig(TransportPref.MOSH, moshServer = null)
        val terminal = mosh.holder.openTerminal(mosh.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), mosh.port.transports) // Asked for, so it fails visibly if it must.
        assertEquals(listOf<UInt?>(null), mosh.port.budgets) // The 15 s default.
        fail(mosh, 0, SessionFailure.NotInstalled("mosh-server"))
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.NotInstalled("mosh-server"))), terminal.state.value)
        assertEquals(1, mosh.port.transports.size) // No silent SSH.
    }

    @Test
    fun aTransportPreferenceEditedOnALiveConnectionAppliesToTheNextTerminal() = runTest {
        val rig = rig()
        rig.active.mutableUdpVerdict.value = UdpVerdict.OK
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
        assertEquals(TerminalTransport.MOSH, rig.holder.terminals.value.first().transport.value)
        rig.holder.setTransport(999, TransportPref.SSH) // A host without a connection: nothing to do.
    }

    @Test
    fun udpKnownToWorkOpensTmuxAndHerdrOverMoshDirectly() = runTest {
        val rig = rig()
        rig.active.mutableUdpVerdict.value = UdpVerdict.OK
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Herdr(null, "w1:p1"))
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
        assertEquals(listOf<UInt?>(AUTO_MOSH_BUDGET_MS), rig.port.budgets)
        assertTrue(terminal.fallbackEligible)
    }

    // --- the background attempt and the swap -------------------------------------------------

    @Test
    fun tmuxOpensOverSshAtOnceAndSwapsToTheBackgroundMoshSessionOnceItConnects() = runTest {
        val rig = rig()
        rig.port.nextServerPid = 4242u
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        // SSH now, mosh behind it with the explicit-mosh budget (15 s, Rust's default).
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH), rig.port.transports)
        assertEquals(listOf<UInt?>(null, null), rig.port.budgets)
        val ssh = rig.port.terminals[0].third
        val mosh = rig.port.terminals[1].third
        assertSame(ssh, terminal.handle.value)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        val drawn = frames(terminal)
        state(rig, 0, SessionState.Connected)
        sessionListener(rig, 0).onFrameReady()
        advanceUntilIdle()
        val beforeSwap = drawn()

        // The background session's frames and health do not reach the terminal before the swap.
        sessionListener(rig, 1).onFrameReady()
        sessionListener(rig, 1).onLinkHealth(LinkHealth(300uL, 300uL))
        advanceUntilIdle()
        assertEquals(beforeSwap, drawn())
        assertNull(terminal.linkHealth.value)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)

        state(rig, 1, SessionState.Connected)

        // Same terminal, now on mosh: the badge follows, the server is recorded, UDP is known to work.
        assertEquals(listOf(terminal), rig.holder.terminals.value)
        assertSame(mosh, terminal.handle.value)
        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
        assertEquals(TerminalTransport.MOSH, rig.holder.transports().first()[terminal.id])
        assertEquals(SessionState.Connected, terminal.state.value)
        assertEquals(4242u, terminal.moshServerPid)
        assertEquals(listOf(4242u), rig.ledger.pids(rig.host))
        assertEquals(UdpVerdict.OK, rig.active.udpVerdict.value)
        assertTrue(drawn() > beforeSwap) // The view is told to draw the new session.
        // The SSH session is disconnected and released.
        assertEquals(listOf("disconnect", "close"), ssh.events)
        assertFalse(mosh.destroyed)

        // The old session can no longer speak for the terminal: its frames and its close are dropped.
        val afterSwap = drawn()
        sessionListener(rig, 0).onFrameReady()
        state(rig, 0, SessionState.Closed(CloseReason.Disconnected))
        assertEquals(afterSwap, drawn())
        assertEquals(SessionState.Connected, terminal.state.value)
        sessionListener(rig, 1).onFrameReady()
        sessionListener(rig, 1).onLinkHealth(LinkHealth(400uL, 400uL))
        advanceUntilIdle()
        assertEquals(afterSwap + 1, drawn())
        assertEquals(LinkHealth(400uL, 400uL), terminal.linkHealth.value)

        // UDP works now: the next tmux terminal opens over mosh directly.
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("next"))
        assertEquals(TerminalTransport.MOSH, rig.port.transports.last())
        assertEquals(3, rig.port.transports.size)
    }

    @Test
    fun aBackgroundMoshThatConnectsBeforeTheSshSessionStillSwapsIn() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Herdr("work", null))
        state(rig, 1, SessionState.Connected)
        assertEquals(TerminalTransport.MOSH, terminal.transport.value)
        assertEquals(SessionState.Connected, terminal.state.value)
        assertTrue(terminal.hasConnected.value)
        // The SSH open that lost the race is ended, and what it says later does not count.
        assertEquals("disconnect", rig.port.terminals[0].third.events.first())
        state(rig, 0, SessionState.Connected)
        state(rig, 0, SessionState.Closed(CloseReason.Disconnected))
        assertEquals(SessionState.Connected, terminal.state.value)
    }

    @Test
    fun aBlockedBackgroundAttemptLeavesTheTerminalOnSshUnseen() = runTest {
        for (failure in listOf(SessionFailure.TimedOut, SessionFailure.NotInstalled("mosh-server"))) {
            val rig = rig()
            val terminal = rig.holder.openTerminal(rig.active, tmux)
            state(rig, 0, SessionState.Connected)
            fail(rig, 1, failure)
            // Nothing the user sees changes: the terminal stays on SSH, connected.
            assertSame(rig.port.terminals[0].third, terminal.handle.value)
            assertEquals(TerminalTransport.SSH, terminal.transport.value)
            assertEquals(SessionState.Connected, terminal.state.value)
            assertTrue(rig.port.terminals[1].third.destroyed) // The failed attempt is released.
            assertEquals(UdpVerdict.BLOCKED, rig.active.udpVerdict.value)
            // BLOCKED opens over SSH with no attempt, for every target.
            rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
            rig.holder.openTerminal(rig.active, shell)
            assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.SSH), rig.port.transports)
        }
    }

    @Test
    fun anInconclusiveBackgroundFailureKeepsTheVerdictUnknown() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        fail(rig, 1, SessionFailure.CommandFailed("mosh-server did not start"))
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        // At most one attempt per terminal: nothing is retried for it.
        advanceTimeBy(60_000)
        assertEquals(2, rig.port.transports.size)
        // The next terminal tries again.
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.MOSH), rig.port.transports)
    }

    @Test
    fun oneAttemptIsInFlightPerHostAndTheOthersWaitForItsVerdict() = runTest {
        val rig = rig()
        val a = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("a"))
        val b = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        val c = rig.holder.openTerminal(rig.active, TerminalTarget.Herdr(null, "w1:p1"))
        // Every terminal opened at once over SSH; only the first tries mosh.
        assertEquals(listOf(TerminalTransport.SSH, TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.SSH), rig.port.transports)
        listOf(a, b, c).forEach { assertEquals(TerminalTransport.SSH, it.transport.value) }

        state(rig, 1, SessionState.Connected) // a's attempt: UDP works.
        assertEquals(TerminalTransport.MOSH, a.transport.value)
        // The waiting terminals now try theirs, each its own.
        assertEquals(6, rig.port.transports.size)
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.MOSH), rig.port.transports.drop(4))
        state(rig, 4, SessionState.Connected)
        state(rig, 5, SessionState.Connected)
        assertEquals(TerminalTransport.MOSH, b.transport.value)
        assertEquals(TerminalTransport.MOSH, c.transport.value)
    }

    @Test
    fun aBlockedVerdictKeepsTheWaitingTerminalsOnSshWithoutAnAttempt() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("a"))
        val b = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        fail(rig, 1, SessionFailure.TimedOut)
        advanceTimeBy(60_000)
        assertEquals(3, rig.port.transports.size)
        assertEquals(TerminalTransport.SSH, b.transport.value)
    }

    @Test
    fun anInconclusiveFailureHandsTheProbeToTheNextWaitingTerminal() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("a"))
        val b = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        fail(rig, 1, SessionFailure.ConnectionLost("reset"))
        assertEquals(TerminalTransport.MOSH, rig.port.transports.last())
        assertEquals(4, rig.port.transports.size)
        state(rig, 3, SessionState.Connected)
        assertEquals(TerminalTransport.MOSH, b.transport.value)
    }

    @Test
    fun closingTheTerminalCancelsItsBackgroundAttemptAndStopsItsServer() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        val waiting = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("b"))
        val mosh = rig.port.terminals[1].third
        rig.holder.disconnectTerminal(terminal)
        advanceUntilIdle()
        // Disconnecting the mosh session before it connected is Rust's abandon path, which stops its server.
        assertEquals(listOf("disconnect", "close"), mosh.events)
        // A late Connected from the cancelled attempt swaps nothing in.
        state(rig, 1, SessionState.Connected)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        assertSame(rig.port.terminals[0].third, terminal.handle.value)
        // The host's probe passed on to the terminal that was waiting.
        assertEquals(4, rig.port.transports.size)
        assertEquals(TerminalTransport.MOSH, rig.port.terminals[3].third.transport)
        // Dismissing (closing) that one cancels its attempt too.
        rig.holder.dismissTerminal(waiting)
        advanceUntilIdle()
        assertEquals("disconnect", rig.port.terminals[3].third.events.first())
        assertTrue(rig.port.terminals[3].third.destroyed)
    }

    @Test
    fun disconnectingTheHostCancelsTheBackgroundAttempts() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, tmux)
        rig.holder.disconnect(rig.host.id)
        advanceUntilIdle()
        assertEquals("disconnect", rig.port.terminals[1].third.events.first())
        state(rig, 1, SessionState.Connected)
        assertEquals(TerminalTransport.SSH, rig.holder.terminals.value.single().transport.value)
    }

    @Test
    fun anSshSessionThatClosesTakesItsBackgroundAttemptWithIt() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, tmux)
        fail(rig, 0, SessionFailure.ConnectionLost("gone"))
        assertEquals("disconnect", rig.port.terminals[1].third.events.first())
        state(rig, 1, SessionState.Connected)
        assertTrue(terminal.state.value is SessionState.Closed)
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
    }

    // --- the verdict: per connection, never remembered ------------------------------------

    @Test
    fun theVerdictIsResetOnEveryNewConnectionAndOnAHostEdit() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, tmux)
        fail(rig, 1, SessionFailure.TimedOut)
        assertEquals(UdpVerdict.BLOCKED, rig.active.udpVerdict.value)

        // An edit (here: the label) is a fresh decision: the user may have just fixed the firewall.
        val renamed = Host(rig.host.record.copy(label = "Renamed"), rig.host.addresses)
        rig.holder.hostEdited(rig.host, renamed)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("again"))
        assertEquals(TerminalTransport.MOSH, rig.port.transports.last()) // Tried again in the background.

        // A new connection starts from UNKNOWN whatever the old one learned.
        fail(rig, 3, SessionFailure.TimedOut)
        assertEquals(UdpVerdict.BLOCKED, rig.active.udpVerdict.value)
        val old = rig.active
        rig.holder.disconnect(rig.host.id)
        rig.hostListener.onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        rig.holder.connect(rig.host, byteArrayOf(1))
        advanceUntilIdle()
        assertNotSame(old, rig.active)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)
    }

    @Test
    fun nothingAboutUdpIsRememberedAcrossConnections() = runTest {
        // A host record still carrying the old 24 h memory: it is not read any more.
        val rig = rig(failedUntil = Long.MAX_VALUE)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)
        rig.holder.openTerminal(rig.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), rig.port.transports)
    }

    // --- the shell's fallback --------------------------------------------------------------

    @Test
    fun autoFallsBackToSshWhenAShellsMoshTimesOutAndBlocksMoshForTheConnection() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        val mosh = rig.port.terminals[0].third
        sessionListener(rig, 0).onLinkHealth(LinkHealth(300uL, 300uL))
        advanceUntilIdle()
        assertEquals(300uL, terminal.linkHealth.value?.sinceHeardMs)

        fail(rig, 0, SessionFailure.TimedOut)

        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), rig.port.transports)
        assertEquals(listOf<UInt?>(700u, null), rig.port.budgets) // The SSH retry has no budget to give.
        assertSame(rig.port.terminals[1].third, terminal.handle.value) // The same terminal, a new session.
        assertEquals(TerminalTransport.SSH, terminal.transport.value)
        assertEquals(SessionState.Connecting, terminal.state.value) // The mosh failure is not shown.
        assertFalse(terminal.hasConnected.value)
        assertNull(terminal.linkHealth.value)
        assertTrue(mosh.destroyed) // The replaced native object is released.
        assertEquals(listOf(terminal), rig.holder.terminals.value)
        assertEquals(UdpVerdict.BLOCKED, rig.active.udpVerdict.value)

        // The replaced attempt can no longer speak for the terminal.
        sessionListener(rig, 0).onStateChanged(SessionState.Connected)
        sessionListener(rig, 0).onLinkHealth(LinkHealth(9000uL, 9000uL))
        advanceUntilIdle()
        assertEquals(SessionState.Connecting, terminal.state.value)
        assertNull(terminal.linkHealth.value)

        state(rig, 1, SessionState.Connected)
        assertEquals(SessionState.Connected, terminal.state.value)
        assertTrue(terminal.hasConnected.value)

        // For this connection: the next terminal goes straight to SSH, with no background try.
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("work"))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH, TerminalTransport.SSH), rig.port.transports)
    }

    @Test
    fun aShellOnMoshThatConnectsMakesTheVerdictOk() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, shell)
        state(rig, 0, SessionState.Connected)
        assertEquals(UdpVerdict.OK, rig.active.udpVerdict.value)
        rig.holder.openTerminal(rig.active, tmux)
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.MOSH), rig.port.transports)
    }

    @Test
    fun aMissingMoshServerAlsoFallsBack() = runTest {
        val rig = rig()
        rig.holder.openTerminal(rig.active, shell)
        fail(rig, 0, SessionFailure.NotInstalled("mosh-server"))
        assertEquals(listOf(TerminalTransport.MOSH, TerminalTransport.SSH), rig.port.transports)
        // A new connection (here: a new rig) starts with no memory of it.
        val again = rig()
        again.holder.openTerminal(again.active, shell)
        assertEquals(listOf(TerminalTransport.MOSH), again.port.transports)
    }

    @Test
    fun aMissingTmuxOrHerdrIsNotAMoshFailure() = runTest {
        val rig = rig()
        rig.active.mutableUdpVerdict.value = UdpVerdict.OK
        val terminal = rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("work"))
        fail(rig, 0, SessionFailure.NotInstalled("tmux"))
        assertEquals(1, rig.port.transports.size) // No SSH retry that would fail the same way.
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.NotInstalled("tmux"))), terminal.state.value)
        assertEquals(UdpVerdict.OK, rig.active.udpVerdict.value)
        // Nor in the background: the attempt says nothing about UDP.
        val other = rig()
        other.holder.openTerminal(other.active, tmux)
        fail(other, 1, SessionFailure.NotInstalled("tmux"))
        assertEquals(UdpVerdict.UNKNOWN, other.active.udpVerdict.value)
    }

    @Test
    fun otherFailuresAndAfterConnectedNeverFallBack() = runTest {
        val rig = rig()
        val terminal = rig.holder.openTerminal(rig.active, shell)
        fail(rig, 0, SessionFailure.Unreachable("no route"))
        assertEquals(1, rig.port.transports.size)
        assertTrue(terminal.state.value is SessionState.Closed)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value)

        val connected = rig.holder.openTerminal(rig.active, shell)
        state(rig, 1, SessionState.Connected)
        fail(rig, 1, SessionFailure.TimedOut) // Mid-session: mosh was working, so this is a real failure.
        assertEquals(2, rig.port.transports.size)
        assertTrue(connected.state.value is SessionState.Closed)
        assertEquals(UdpVerdict.OK, rig.active.udpVerdict.value)
    }

    @Test
    fun aDisconnectedTerminalDoesNotFallBackAndAFailedRetryShowsTheMoshFailure() = runTest {
        val rig = rig()
        val ended = rig.holder.openTerminal(rig.active, shell)
        rig.holder.disconnectTerminal(ended)
        fail(rig, 0, SessionFailure.TimedOut)
        assertEquals(1, rig.port.transports.size)
        assertEquals(UdpVerdict.UNKNOWN, rig.active.udpVerdict.value) // The user ended it: not mosh's failure.

        val other = rig.holder.openTerminal(rig.active, shell)
        rig.port.openFailure = HostException.Closed()
        fail(rig, 1, SessionFailure.TimedOut)
        assertEquals(SessionState.Closed(CloseReason.Failed(SessionFailure.TimedOut)), other.state.value)
        assertEquals(TerminalTransport.MOSH, other.transport.value)
        assertEquals(UdpVerdict.BLOCKED, rig.active.udpVerdict.value)
    }

    // --- the rest of the terminal's life ---------------------------------------------------

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
        fail(rig, 0, SessionFailure.ConnectionLost("gone"))
        assertNull(terminal.linkHealth.value)
        assertNull(linkStaleLabel(terminal.linkHealth.value))
    }

    @Test
    fun disconnectAllEndsEveryTerminalAndHostAndTheUserCloseListenerHearsIt() = runTest {
        val rig = rig(TransportPref.SSH)
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
        val rig = rig(TransportPref.SSH)
        val events = mutableListOf<String>()
        rig.holder.userClose = object : UserCloseListener {
            override fun hostClosed(hostId: Long) = Unit
            override fun terminalClosed(hostId: Long, target: TerminalTarget) { events += "closed:$target" }
        }
        rig.holder.openTerminal(rig.active, shell)
        rig.holder.openTerminal(rig.active, TerminalTarget.Tmux("w"))
        fail(rig, 1, SessionFailure.ConnectionLost("gone"))
        assertTrue(events.isEmpty())
        state(rig, 0, SessionState.Closed(CloseReason.RemoteExited(0u)))
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
        val t0 = testScheduler.currentTime
        known.holder.awaitCapabilities(known.active, timeoutMs = 10_000)
        assertEquals(t0, testScheduler.currentTime) // No waiting.

        val failed = rig(probed = false)
        // The failed probe is an answer too: it must not make the caller wait out the timeout.
        assertNotNull(failed.active.capabilitiesError.value)
        val t1 = testScheduler.currentTime
        failed.holder.awaitCapabilities(failed.active, timeoutMs = 10_000)
        assertEquals(t1, testScheduler.currentTime)

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
}

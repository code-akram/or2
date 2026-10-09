package io.github.code_akram.or2.service

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.app.SessionMarker
import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

/** The foreground service's ownership rules, notification text, starter and network debounce. */
@OptIn(ExperimentalCoroutinesApi::class)
class ServiceTest {
    private fun host(id: Long, label: String, open: Boolean = true) = OpenHost(id, label, open)
    private fun terminal(hostId: Long, label: String, open: Boolean = true) = OpenTerminal(hostId, label, open)

    @Test
    fun theServiceRunsWhileAnyHostOrSessionIsOpen() {
        assertTrue(serviceSnapshot(emptyList(), emptyList()).idle)
        assertFalse(serviceSnapshot(listOf(host(1, "A")), emptyList()).idle) // A connected host with no session.
        assertFalse(serviceSnapshot(emptyList(), listOf(terminal(1, "A"))).idle)
        // Closed hosts and sessions stay listed in the app but hold nothing open.
        assertTrue(serviceSnapshot(listOf(host(1, "A", open = false)), listOf(terminal(1, "A", open = false))).idle)
    }

    @Test
    fun aMoshSessionKeepsItsHostListedAfterTheSshConnectionIsLost() {
        val snapshot = serviceSnapshot(listOf(host(1, "Alpha", open = false), host(2, "Beta")), listOf(terminal(1, "Alpha"), terminal(2, "Beta")))
        assertEquals(listOf(HostEntry(1, "Alpha", 1), HostEntry(2, "Beta", 1)), snapshot.hosts)
        assertEquals(2, snapshot.sessions)
    }

    @Test
    fun hostsAreListedByLabelWithTheirOpenSessionCounts() {
        val snapshot = serviceSnapshot(
            listOf(host(2, "beta"), host(1, "Alpha")),
            listOf(terminal(1, "Alpha"), terminal(1, "Alpha"), terminal(1, "Alpha", open = false)),
        )
        assertEquals(listOf(HostEntry(1, "Alpha", 2), HostEntry(2, "beta", 0)), snapshot.hosts)
    }

    @Test
    fun theNotificationListsHostsAndSessions() {
        assertEquals(
            NotificationContent("Connected to Alpha", "1 open session", listOf("Alpha · 1 session")),
            notificationContent(ServiceSnapshot(listOf(HostEntry(1, "Alpha", 1)))),
        )
        val many = notificationContent(ServiceSnapshot(listOf(HostEntry(1, "Alpha", 2), HostEntry(2, "Beta", 0))))
        assertEquals("Connected to 2 hosts", many.title)
        assertEquals("2 open sessions", many.text)
        assertEquals(listOf("Alpha · 2 sessions", "Beta"), many.lines)
        assertEquals("No open sessions", notificationContent(ServiceSnapshot(listOf(HostEntry(1, "Alpha", 0)))).text)
    }

    private fun agent(pane: String, status: AgentStatus) =
        HerdrAgent(pane, "w1:t1", "w1", null, "claude", null, status, null, 1u, "term_$pane", null)

    private fun herdrView(vararg agents: HerdrAgent) = HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), agents.toList())

    @Test
    fun theNotificationSummarisesTheAgentsAcrossHosts() {
        val summary = agentSummary(
            listOf(
                herdrView(agent("w1:p1", AgentStatus.BLOCKED), agent("w1:p2", AgentStatus.WORKING)),
                herdrView(agent("w1:p1", AgentStatus.WORKING), agent("w1:p2", AgentStatus.DONE), agent("w1:p3", AgentStatus.IDLE)),
            ),
        )
        assertEquals(AgentSummary(needsInput = 1, working = 2), summary)
        assertEquals("1 needs input · 2 working", summary.text)
        assertEquals("3 needs input", AgentSummary(needsInput = 3).text)
        assertEquals("1 working", AgentSummary(working = 1).text)
        assertNull(AgentSummary().text)
        assertNull(agentSummary(listOf(herdrView(agent("w1:p1", AgentStatus.DONE)))).text)
        val content = notificationContent(ServiceSnapshot(listOf(HostEntry(1, "Alpha", 1)), summary))
        assertEquals("1 needs input · 2 working · 1 open session", content.text)
        assertEquals(listOf("Alpha · 1 session"), content.lines)
    }

    /** While an agent needs input the notification asks to be a Live Update, with a short critical text; else not. */
    @Test
    fun anAgentThatNeedsInputPromotesTheNotificationAndNoneDemotesIt() {
        val hosts = listOf(HostEntry(1, "Alpha", 1))
        val promoted = notificationContent(ServiceSnapshot(hosts, AgentSummary(needsInput = 2, working = 1)))
        assertTrue(promoted.promoted)
        assertEquals("2 input", promoted.shortCriticalText)
        assertEquals("Connected to Alpha", promoted.title)
        for (summary in listOf(AgentSummary(working = 3), AgentSummary())) {
            val ordinary = notificationContent(ServiceSnapshot(hosts, summary))
            assertFalse(ordinary.promoted)
            assertNull(ordinary.shortCriticalText)
        }
        // Nothing open: nothing to promote, whatever was counted.
        assertFalse(notificationContent(ServiceSnapshot(emptyList(), AgentSummary(needsInput = 1))).promoted)
    }

    @Test
    fun theSnapshotsCountTheAgentsOfTheLiveWatches() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val ports = mutableListOf<FakePort>()
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections({ _, listener -> listeners += listener; FakePort().also { ports += it } }, FakeTrust(), dispatcher, dispatcher)
        val snapshots = mutableListOf<ServiceSnapshot>()
        val job = launch(dispatcher) { holder.serviceSnapshots().collect { snapshots += it } }
        holder.connect(testHost(1, "Alpha"), byteArrayOf(1))
        listeners.last().onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        val watch = ports.last().watches.single().second
        watch.onHerdrStateChanged(HerdrState.Live(herdrView(agent("w1:p1", AgentStatus.BLOCKED), agent("w1:p2", AgentStatus.WORKING))))
        runCurrent()
        assertEquals(AgentSummary(1, 1), snapshots.last().agents)
        assertTrue(notificationContent(snapshots.last()).promoted)
        watch.onHerdrStateChanged(HerdrState.Live(herdrView(agent("w1:p1", AgentStatus.WORKING), agent("w1:p2", AgentStatus.WORKING))))
        runCurrent()
        assertEquals(AgentSummary(0, 2), snapshots.last().agents)
        assertFalse(notificationContent(snapshots.last()).promoted)
        // herdr went away: nothing is counted.
        watch.onHerdrStateChanged(HerdrState.Unavailable(HerdrUnavailable.NotRunning, ""))
        runCurrent()
        assertEquals(AgentSummary(), snapshots.last().agents)
        job.cancel()
    }

    private class RecordingHost : ServiceHost {
        val shown = mutableListOf<NotificationContent>()
        var stops = 0
        override fun show(content: NotificationContent) { shown += content }
        override fun stop() { stops++ }
    }

    @Test
    fun theControllerPostsTheNotificationAtOnceUpdatesItAndStopsWhenTheLastThingCloses() = runTest {
        val snapshots = MutableStateFlow(ServiceSnapshot(listOf(HostEntry(1, "Alpha", 1))))
        val host = RecordingHost()
        val state = ServiceRunState()
        val controller = ServiceController(this, snapshots, host, state)
        controller.begin()
        assertEquals("Starting…", host.shown.first().text) // Posted before any snapshot arrived: Android's start-up deadline.
        runCurrent()
        assertEquals("Connected to Alpha", host.shown.last().title)
        assertTrue(state.running)

        snapshots.value = ServiceSnapshot(listOf(HostEntry(1, "Alpha", 2), HostEntry(2, "Beta", 0)))
        runCurrent()
        assertEquals("Connected to 2 hosts", host.shown.last().title)
        assertEquals(0, host.stops)

        snapshots.value = ServiceSnapshot(emptyList())
        runCurrent()
        assertEquals(1, host.stops)
        assertFalse(state.running) // Cleared before the stop, so a new connection starts a fresh service.
        snapshots.value = ServiceSnapshot(listOf(HostEntry(3, "Late", 1)))
        runCurrent()
        assertEquals(1, host.stops) // The ended controller listens no more.
        controller.close()
    }

    @Test
    fun aServiceStartedWithNothingOpenStopsOnTheFirstSnapshot() = runTest {
        val host = RecordingHost()
        val controller = ServiceController(this, MutableStateFlow(ServiceSnapshot(emptyList())), host, ServiceRunState())
        controller.begin()
        runCurrent()
        assertEquals(1, host.stops)
        assertEquals(1, host.shown.size) // It still posted the mandatory notification first.
    }

    @Test
    fun aSecondStartCommandRepostsWithoutSecondCollector() = runTest {
        val snapshots = MutableStateFlow(ServiceSnapshot(listOf(HostEntry(1, "A", 1))))
        val host = RecordingHost()
        val controller = ServiceController(this, snapshots, host, ServiceRunState())
        controller.begin()
        runCurrent()
        val before = host.shown.size
        controller.begin()
        runCurrent()
        assertEquals(before + 1, host.shown.size) // One repost, not a duplicated collector.
        snapshots.value = ServiceSnapshot(listOf(HostEntry(1, "A", 3)))
        runCurrent()
        assertEquals(before + 2, host.shown.size)
        controller.close()
    }

    @Test
    fun theSessionMarkerFollowsTheRealHoldersOpenTerminalsAndSurvivesOnlyAKill() = runTest {
        val store = MemoryPrefStore()
        val port = FakePort()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        val marker = SessionMarker(store)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) {
            holder.serviceSnapshots().collect { marker.onOpenSessions(it.sessions > 0) }
        }
        runCurrent()
        assertFalse(SessionMarker(store).diedWithSessions) // Idle at start: nothing to resume.

        holder.connect(testHost(7, "Alpha"), byteArrayOf(1))
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        assertFalse(SessionMarker(store).diedWithSessions) // A connection alone is not a session.

        holder.openTerminal(holder.host(7)!!, TerminalTarget.Shell)
        advanceUntilIdle()
        // The process is killed here: the next one finds the marker.
        assertTrue(SessionMarker(store).diedWithSessions)

        // An orderly end clears it: the last session closes (Disconnect all, the user's close, a remote exit).
        port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertFalse(SessionMarker(store).diedWithSessions)

        // A session whose host connection is lost still counts (mosh survives it): killed now, it resumes.
        holder.openTerminal(holder.host(7)!!, TerminalTarget.Shell)
        listener!!.onHostStateChanged(HostState.Closed(CloseReason.Failed(io.github.code_akram.or2.ffi.SessionFailure.ConnectionLost("x"))))
        advanceUntilIdle()
        assertTrue(SessionMarker(store).diedWithSessions)
        holder.disconnectAll()
        port.terminals[1].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertFalse(SessionMarker(store).diedWithSessions)
    }

    @Test
    fun theServiceFollowsTheRealConnectionHolder() = runTest {
        val port = FakePort()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        val seen = mutableListOf<ServiceSnapshot>()
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { holder.serviceSnapshots().collect { seen += it } }
        runCurrent()
        assertTrue(seen.last().idle)

        val alpha = testHost(7, "Alpha")
        holder.connect(alpha, byteArrayOf(1))
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        assertEquals(listOf(HostEntry(7, "Alpha", 0)), seen.last().hosts)

        val terminal = holder.openTerminal(holder.host(7)!!, TerminalTarget.Shell)
        advanceUntilIdle()
        assertEquals(listOf(HostEntry(7, "Alpha", 1)), seen.last().hosts)

        // The SSH connection drops; a (mosh) terminal that is still open keeps the service up.
        listener!!.onHostStateChanged(HostState.Closed(CloseReason.Failed(io.github.code_akram.or2.ffi.SessionFailure.ConnectionLost("x"))))
        advanceUntilIdle()
        assertEquals(listOf(HostEntry(7, "Alpha", 1)), seen.last().hosts)

        port.terminals[0].second.onStateChanged(SessionState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertTrue(seen.last().idle)
        assertEquals(terminal, holder.terminals.value.single()) // Closed, still listed, no longer counted.
    }

    @Test
    fun theStarterStartsOnlyWhileNothingIsRunningAndSomethingIsOpen() {
        val state = ServiceRunState()
        var starts = 0
        val starter = ServiceStarter(state) { starts++ }
        starter.onSnapshot(ServiceSnapshot(emptyList()))
        assertEquals(0, starts)
        starter.onSnapshot(ServiceSnapshot(listOf(HostEntry(1, "A", 0))))
        assertEquals(1, starts)
        state.running = true
        starter.onSnapshot(ServiceSnapshot(listOf(HostEntry(1, "A", 1))))
        assertEquals(1, starts) // Already running: the notification follows by itself.
        state.running = false // The service just stopped, and the user connected again.
        starter.onSnapshot(ServiceSnapshot(listOf(HostEntry(2, "B", 0))))
        assertEquals(2, starts)
    }

    @Test
    fun aServiceDestroyedFromOutsideWhileSomethingIsOpenIsStartedAgainOnRecheck() {
        val state = ServiceRunState()
        var starts = 0
        val starter = ServiceStarter(state) { starts++ }
        starter.recheck() // No snapshot yet: nothing to start for.
        assertEquals(0, starts)

        starter.onSnapshot(ServiceSnapshot(listOf(HostEntry(1, "A", 1))))
        assertEquals(1, starts)
        state.running = true // The service came up...
        starter.recheck()
        assertEquals(1, starts)
        state.running = false // ...and was stopped from outside; no snapshot changed, so only a recheck notices.
        starter.recheck()
        assertEquals(2, starts)

        starter.onSnapshot(ServiceSnapshot(emptyList())) // Everything closed: a destroyed service stays destroyed.
        starter.recheck()
        assertEquals(2, starts)
    }

    @Test
    fun theControllerCountsEachBeginSoATestCanTellNeverStartedFromStartedAndStopped() = runTest {
        val state = ServiceRunState()
        val controller = ServiceController(this, MutableStateFlow(ServiceSnapshot(emptyList())), RecordingHost(), state)
        assertEquals(0, state.begins.get())
        controller.begin()
        assertEquals(1, state.begins.get())
        runCurrent()
        assertFalse(state.running) // Stopped itself, and the count still says it started.
        assertEquals(1, state.begins.get())
    }

    @Test
    fun networkChangesAreDebouncedByHalfASecond() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.available(2L)
        advanceTimeBy(499)
        assertEquals(0, notified)
        changes.available(3L) // A burst: Wi-Fi to mobile data to Wi-Fi.
        advanceTimeBy(499)
        assertEquals(0, notified)
        advanceTimeBy(2)
        assertEquals(1, notified) // One notification, 500 ms after the last event.
        advanceTimeBy(5_000)
        assertEquals(1, notified)
    }

    @Test
    fun onlyARealDefaultNetworkChangeCounts() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.available(1L) // The callback's first report of the network that is already the default.
        advanceTimeBy(1_000)
        assertEquals(0, notified)
        changes.lost(9L) // Not the default: nothing.
        changes.available(1L)
        advanceTimeBy(1_000)
        assertEquals(0, notified)

        changes.lost(1L) // Lost, then back: the sockets are stale.
        advanceTimeBy(1_000)
        assertEquals(0, notified) // Nothing to roam onto while lost.
        changes.available(1L)
        advanceTimeBy(1_000)
        assertEquals(1, notified)
        changes.available(2L)
        advanceTimeBy(1_000)
        assertEquals(2, notified)
    }

    @Test
    fun noNetworkAtRegistrationMeansTheFirstAvailableIsAChange() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }
        changes.available(7L)
        advanceTimeBy(500)
        runCurrent()
        assertEquals(1, notified)
    }

    // --- the follow-up: transport and interface changes, and the return to the foreground --------

    @Test
    fun aTransportSetChangeOnTheSameDefaultNetworkRoams() = runTest {
        // A VPN-carried connection: the default network (the VPN) never changes, only what it rides on.
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.capabilitiesChanged(1L, "VPN,WIFI") // The first report is the baseline.
        advanceTimeBy(1_000)
        assertEquals(0, notified)
        changes.capabilitiesChanged(1L, "VPN,CELLULAR")
        advanceTimeBy(499)
        assertEquals(0, notified)
        advanceTimeBy(2)
        assertEquals(1, notified)
        changes.capabilitiesChanged(1L, "VPN,WIFI") // And back again.
        advanceTimeBy(1_000)
        assertEquals(2, notified)
    }

    @Test
    fun bandwidthAndSignalUpdatesDoNothing() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.capabilitiesChanged(1L, "WIFI")
        changes.linkChanged(1L, "wlan0")
        // The same callbacks fire again and again with the same transports and interface (a new
        // bandwidth estimate, a signal-strength tick): not a change.
        repeat(20) {
            changes.capabilitiesChanged(1L, "WIFI")
            changes.linkChanged(1L, "wlan0")
            advanceTimeBy(300)
        }
        advanceTimeBy(5_000)
        assertEquals(0, notified)
    }

    @Test
    fun anInterfaceChangeRoamsAndAnInterfaceThatAppearsOrVanishesCounts() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.linkChanged(1L, "wlan0")
        changes.linkChanged(1L, "wlan1")
        advanceTimeBy(1_000)
        assertEquals(1, notified)
        changes.linkChanged(1L, null) // No interface name any more.
        advanceTimeBy(1_000)
        assertEquals(2, notified)
        changes.linkChanged(1L, null)
        advanceTimeBy(1_000)
        assertEquals(2, notified)
    }

    @Test
    fun aNewDefaultNetworkStartsFromANewBaseline() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.capabilitiesChanged(1L, "WIFI")
        changes.linkChanged(1L, "wlan0")
        changes.available(2L) // Mobile data takes over: one roam for the switch.
        changes.capabilitiesChanged(2L, "CELLULAR") // Its first reports are baselines, not further changes.
        changes.linkChanged(2L, "rmnet_data1")
        advanceTimeBy(1_000)
        assertEquals(1, notified)
        // A report that arrives for a network we have not seen yet counts as it becoming the default.
        changes.capabilitiesChanged(3L, "WIFI")
        advanceTimeBy(1_000)
        assertEquals(2, notified)
    }

    @Test
    fun theReturnToTheForegroundRoamsOnceThroughTheSameDebounce() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }.apply { seed(1L) }
        changes.foregrounded()
        advanceTimeBy(499)
        assertEquals(0, notified)
        advanceTimeBy(2)
        assertEquals(1, notified)

        // A handover callback arriving with the return is one roam, not two.
        changes.available(2L)
        advanceTimeBy(200)
        changes.foregrounded()
        advanceTimeBy(1_000)
        assertEquals(2, notified)
        changes.foregrounded() // Every return counts, even with nothing else going on.
        advanceTimeBy(1_000)
        assertEquals(3, notified)
    }

    @Test
    fun seedingTracksTheCurrentNetworkWithoutCountingIt() = runTest {
        var notified = 0
        val changes = NetworkChanges(this) { notified++ }
        changes.seed(4L)
        changes.available(4L)
        advanceTimeBy(1_000)
        assertEquals(0, notified)
        changes.available(5L)
        advanceTimeBy(1_000)
        assertEquals(1, notified)
    }
}

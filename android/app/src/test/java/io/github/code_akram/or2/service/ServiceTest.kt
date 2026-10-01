package io.github.code_akram.or2.service

import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.CloseReason
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
        val changes = NetworkChanges(this, initial = 1L) { notified++ }
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
        val changes = NetworkChanges(this, initial = 1L) { notified++ }
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
        val changes = NetworkChanges(this, initial = null) { notified++ }
        changes.available(7L)
        advanceTimeBy(500)
        runCurrent()
        assertEquals(1, notified)
    }
}

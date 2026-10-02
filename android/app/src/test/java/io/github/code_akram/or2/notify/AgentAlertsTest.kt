package io.github.code_akram.or2.notify

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrListener
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A sink that records what would be posted and cancelled. */
class RecordingSink : AgentAlertSink {
    val events = mutableListOf<String>()
    val posted = mutableListOf<AgentAlert>()
    override fun post(alert: AgentAlert) {
        posted += alert
        events += "post ${alert.key.paneId} ${alert.text}"
    }

    override fun cancel(key: AgentPaneKey) {
        events += "cancel ${key.paneId}"
    }
}

@OptIn(ExperimentalCoroutinesApi::class)
class AgentAlertsTest {
    private val sink = RecordingSink()
    private var on = true
    private val alerts = AgentAlerts(sink) { on }
    private val watch = Any()

    private fun agent(pane: String, status: AgentStatus, seq: ULong, name: String? = "Claude Code") =
        HerdrAgent(pane, "w1:t1", "w1", name, "claude", name, status, "/work", "title", false, seq)

    private fun view(vararg agents: HerdrAgent, focused: String? = null) =
        HerdrView(1uL, 22u, focused, emptyList(), emptyList(), emptyList(), agents.toList())

    private fun deliver(view: HerdrView?, watch: Any = this.watch, session: String? = null, host: Long = 1) =
        alerts.viewChanged(watch, host, "Workstation", session, view)

    @Test
    fun theFirstSnapshotNeverNotifiesOnlyAnAdvanceSeenLive() {
        // Blocked and Done before the app watched (the first view after a connect): nothing.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 5u), agent("w1:p2", AgentStatus.DONE, 3u)))
        assertTrue(sink.events.isEmpty())
        // The same view again: still nothing.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 5u), agent("w1:p2", AgentStatus.DONE, 3u)))
        assertTrue(sink.events.isEmpty())
        // Live edges: one each.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 6u), agent("w1:p2", AgentStatus.DONE, 3u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 7u), agent("w1:p2", AgentStatus.DONE, 3u)))
        assertEquals(listOf("post w1:p1 Needs input"), sink.events)
    }

    @Test
    fun exactlyOneNotificationPerSequenceNumber() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
        // Re-deliveries of the same edge (another view version, the focus moved) post nothing more.
        repeat(3) { deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), focused = "w1:p9")) }
        // Blocked straight to Done is a new edge: it replaces the pane's notification.
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u)))
        assertEquals(listOf("post w1:p1 Needs input", "post w1:p1 Done"), sink.events)
        assertEquals(setOf(AgentPaneKey(1, null, "w1:p1")), alerts.active)
        // Idle, Unknown and Working edges never notify.
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 4u)))
        deliver(view(agent("w1:p1", AgentStatus.UNKNOWN, 5u)))
        assertEquals(2, sink.posted.size)
        // A sequence that went backwards (herdr restarted under the watch) is no advance.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 1u)))
        assertEquals(2, sink.posted.size)
    }

    @Test
    fun theNotificationSaysWhoAndWhatAndWhereNothingFromTheOutput() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u, name = "Codex")))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u, name = "Codex")))
        assertEquals(AgentAlert(AgentPaneKey(1, null, "w1:p1"), "Codex", "Needs input", "Workstation"), sink.posted.single())
        // Unnamed agents read as the inbox shows them.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u, name = null)))
        deliver(view(agent("w1:p1", AgentStatus.DONE, 4u, name = null)))
        assertEquals("claude", sink.posted.last().title)
        assertEquals("Done", sink.posted.last().text)
        assertEquals("Needs input", alertText(AgentStatus.BLOCKED))
        assertNull(alertText(AgentStatus.WORKING))
    }

    @Test
    fun backToWorkingOrGoneCancelsThePanesNotification() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u)))
        sink.events.clear()
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u), agent("w1:p2", AgentStatus.DONE, 2u)))
        assertEquals(listOf("cancel w1:p1"), sink.events)
        // Done going Idle keeps the notification: only Working, opening and disappearing cancel it.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u), agent("w1:p2", AgentStatus.IDLE, 3u)))
        assertEquals(listOf("cancel w1:p1"), sink.events)
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u)))
        assertEquals(listOf("cancel w1:p1", "cancel w1:p2"), sink.events)
        assertTrue(alerts.active.isEmpty())
        // A pane with nothing posted is not cancelled again.
        deliver(view())
        assertEquals(2, sink.events.size)
    }

    @Test
    fun nothingWhileThePaneIsOnScreen() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), focused = "w1:p1"))
        alerts.screenChanged(OnScreen(1, TerminalTarget.Herdr(null, "w1:p1")))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), focused = "w1:p1"))
        assertTrue(sink.events.isEmpty())
        // The edge was seen on screen: leaving the screen does not post it late.
        alerts.screenChanged(null)
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), focused = "w1:p1"))
        assertTrue(sink.events.isEmpty())
        // The app is not resumed (screen null): the next edge posts.
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u), focused = "w1:p1"))
        assertEquals(listOf("post w1:p1 Done"), sink.events)
    }

    @Test
    fun onScreenMeansTheFocusedPaneOfThatHostAndSession() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u), focused = "w1:p2"))
        // A terminal opened for p1 shows herdr's focused pane, p2: p1 is not on screen.
        alerts.screenChanged(OnScreen(1, TerminalTarget.Herdr(null, "w1:p1")))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.BLOCKED, 2u), focused = "w1:p2"))
        assertEquals(listOf("post w1:p1 Needs input"), sink.events)
        // Another session, another host, a shell or tmux terminal: not on screen.
        for (screen in listOf(
            OnScreen(1, TerminalTarget.Herdr("work", "w1:p3")), OnScreen(2, TerminalTarget.Herdr(null, "w1:p3")),
            OnScreen(1, TerminalTarget.Shell), OnScreen(1, TerminalTarget.Tmux("main")),
        )) {
            alerts.screenChanged(screen)
            deliver(view(agent("w1:p3", AgentStatus.WORKING, 1u), focused = "w1:p3"), watch = screen)
            deliver(view(agent("w1:p3", AgentStatus.BLOCKED, 2u), focused = "w1:p3"), watch = screen)
        }
        assertEquals(5, sink.posted.size)
        // Before herdr reported a focus, a pane-less terminal shows nothing known, a pane terminal its pane.
        val fresh = Any()
        alerts.screenChanged(OnScreen(3, TerminalTarget.Herdr(null, "w1:p1")))
        alerts.viewChanged(fresh, 3, "Laptop", null, view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        alerts.viewChanged(fresh, 3, "Laptop", null, view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
        assertEquals(5, sink.posted.size)
        alerts.screenChanged(OnScreen(3, TerminalTarget.Herdr(null, null)))
        alerts.viewChanged(fresh, 3, "Laptop", null, view(agent("w1:p1", AgentStatus.DONE, 3u)))
        assertEquals(6, sink.posted.size)
    }

    @Test
    fun showingThePaneCancelsItsNotification() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u), focused = "w1:p1"))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u), focused = "w1:p1"))
        sink.events.clear()
        // Opened (an inbox tap, a thumbnail, the notification): on screen, so its notification goes.
        alerts.screenChanged(OnScreen(1, TerminalTarget.Herdr(null, "w1:p1")))
        assertEquals(listOf("cancel w1:p1"), sink.events)
        // The focus moves to p2 inside the shown terminal: p2 is on screen now.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u), focused = "w1:p2"))
        assertEquals(listOf("cancel w1:p1", "cancel w1:p2"), sink.events)
        // A notification's tap cancels it too, whatever is on screen.
        alerts.screenChanged(null)
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u), agent("w1:p2", AgentStatus.DONE, 2u), focused = "w1:p2"))
        alerts.opened(AgentPaneKey(1, null, "w1:p1"))
        assertEquals("cancel w1:p1", sink.events.last())
        assertTrue(alerts.active.isEmpty())
        // One left up by a process that died (this one never posted it) goes too.
        alerts.opened(AgentPaneKey(1, null, "w1:p7"))
        assertEquals("cancel w1:p7", sink.events.last())
    }

    @Test
    fun aWatchThatWasUnavailableOrIsNewStartsFromABaselineAgain() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        deliver(null) // herdr stopped (NotRunning), or the watch closed.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
        assertTrue(sink.events.isEmpty())
        // A reconnect makes a new watch object: its first view is a baseline too.
        val second = Any()
        deliver(view(agent("w1:p1", AgentStatus.DONE, 9u)), watch = second)
        assertTrue(sink.events.isEmpty())
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 10u)), watch = second)
        assertEquals(listOf("post w1:p1 Needs input"), sink.events)
    }

    @Test
    fun aPaneFirstSeenInALaterViewIsBaselinedNotNotified() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.BLOCKED, 4u)))
        assertTrue(sink.events.isEmpty())
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.DONE, 5u)))
        assertEquals(listOf("post w1:p2 Done"), sink.events)
    }

    @Test
    fun panesOfDifferentSessionsAndHostsAreDifferentNotifications() {
        val work = Any()
        val other = Any()
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)), watch = work, session = "work")
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)), watch = other, host = 2)
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)), watch = work, session = "work")
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)), watch = other, host = 2)
        assertEquals(setOf(AgentPaneKey(1, null, "w1:p1"), AgentPaneKey(1, "work", "w1:p1"), AgentPaneKey(2, null, "w1:p1")), alerts.active)
        // Back to work in one session cancels that session's only.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u)), watch = work, session = "work")
        assertEquals(setOf(AgentPaneKey(1, null, "w1:p1"), AgentPaneKey(2, null, "w1:p1")), alerts.active)
    }

    @Test
    fun switchedOffNothingPostsAndWhatWasUpGoes() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        on = false
        alerts.enabledChanged()
        assertEquals(listOf("post w1:p1 Needs input", "cancel w1:p1"), sink.events)
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u)))
        assertEquals(2, sink.events.size)
        // Switched on again: an edge seen while off is not posted late; the next one is.
        on = true
        alerts.enabledChanged()
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u)))
        assertEquals(2, sink.events.size)
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u), agent("w1:p2", AgentStatus.DONE, 2u)))
        assertEquals("post w1:p1 Done", sink.events.last())
    }

    @Test
    fun theSettingIsOnByDefaultAndRemembered() {
        val store = MemoryPrefStore()
        assertTrue(AgentAlertSettings(store).enabled.value)
        AgentAlertSettings(store).set(false)
        assertFalse(AgentAlertSettings(store).enabled.value)
        val settings = AgentAlertSettings(store)
        settings.set(true)
        assertTrue(settings.enabled.value)
        assertTrue(AgentAlertSettings(store).enabled.value)
    }

    @Test
    fun eachPaneHasItsOwnTagAndItSurvivesAsSavedState() {
        val keys = listOf(
            AgentPaneKey(1, null, "w1:p1"), AgentPaneKey(1, "w1", "p1"), AgentPaneKey(1, "w1:p1", ""), AgentPaneKey(1, "", "w1:p1"),
            AgentPaneKey(2, null, "w1:p1"), AgentPaneKey(1, "d", "w1:p1"), AgentPaneKey(12, null, "w1:p1"),
        )
        assertEquals(keys.size, keys.map { it.tag }.toSet().size)
        assertEquals("agent:1:d:w1:p1", keys[0].tag)
        assertEquals("agent:1:s2:w1:p1", keys[1].tag)
        for (key in keys) assertEquals(key, AgentPaneKey.fromParts(key.toParts()))
        assertArrayEquals(arrayOf("1", "d", "w1:p1"), keys[0].toParts())
        assertNull(AgentPaneKey.fromParts(arrayOf("x", "d", "p")))
        assertNull(AgentPaneKey.fromParts(arrayOf("1", "q", "p")))
        assertNull(AgentPaneKey.fromParts(arrayOf("1", "d")))
    }

    @Test
    fun onlyTheAppsOwnTapIsHonoured() {
        assertEquals(AgentPaneKey(3, "work", "w1:p2"), agentOpenFrom(3, "work", "w1:p2", "secret", "secret"))
        assertEquals(AgentPaneKey(3, null, "w1:p2"), agentOpenFrom(3, null, "w1:p2", "secret", "secret"))
        assertNull(agentOpenFrom(3, null, "w1:p2", "guess", "secret")) // Another app's intent.
        assertNull(agentOpenFrom(3, null, "w1:p2", null, "secret"))
        assertNull(agentOpenFrom(3, null, "w1:p2", "secret", null)) // No notification was ever posted.
        assertNull(agentOpenFrom(0, null, "w1:p2", "secret", "secret"))
        assertNull(agentOpenFrom(3, null, null, "secret", "secret"))
        assertNull(agentOpenFrom(3, null, "", "secret", "secret"))
    }

    @Test
    fun aTapOpensAtOnceWaitsOrConnectsFirst() {
        assertEquals(AgentOpenStart.OPEN, agentOpenStart(HostState.Connected(0u)))
        assertEquals(AgentOpenStart.CONNECT, agentOpenStart(null))
        assertEquals(AgentOpenStart.CONNECT, agentOpenStart(HostState.Closed(CloseReason.Disconnected)))
        assertEquals(AgentOpenStart.CONNECT, agentOpenStart(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset")))))
        assertEquals(AgentOpenStart.WAIT, agentOpenStart(HostState.Connecting))
        assertEquals(AgentOpenStart.WAIT, agentOpenStart(HostState.Authenticating))
        assertEquals(AgentOpenStart.WAIT, agentOpenStart(HostState.AwaitingHostKeyDecision(PublicKeyInfo("a", "b", "c", ""), emptyList())))
        // One request at a time, taken once; a newer tap replaces one not yet taken.
        val requests = AgentOpenRequests()
        assertNull(requests.take())
        requests.request(AgentPaneKey(1, null, "w1:p1"))
        requests.request(AgentPaneKey(1, null, "w1:p2"))
        assertEquals(AgentPaneKey(1, null, "w1:p2"), requests.request.value)
        assertEquals(AgentPaneKey(1, null, "w1:p2"), requests.take())
        assertNull(requests.take())
        assertNull(requests.request.value)
    }

    @Test
    fun theHolderFeedsEveryLiveWatchStateAndNothingAfterTheWatchStopped() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val ports = mutableListOf<FakePort>()
        val listeners = mutableListOf<HostListener>()
        val holder = HostConnections({ _, listener ->
            listeners += listener
            FakePort().also { ports += it }
        }, FakeTrust(), dispatcher, dispatcher)
        holder.herdrObserver = alerts
        val host = testHost(1, "Workstation")
        holder.connect(host, byteArrayOf(1))
        listeners.last().onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        val first: HerdrListener = ports.last().watches.single().second
        // The first snapshot of the connection is a baseline; the live edge after it notifies once.
        first.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.BLOCKED, 4u))))
        first.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.WORKING, 5u))))
        first.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.BLOCKED, 6u))))
        runCurrent()
        assertEquals(listOf("post w1:p1 Needs input"), sink.events)
        assertEquals("Workstation", sink.posted.single().subText)
        // herdr went away and came back: a fresh baseline.
        first.onHerdrStateChanged(HerdrState.Unavailable(HerdrUnavailable.NotRunning, ""))
        first.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.DONE, 8u))))
        runCurrent()
        assertEquals(1, sink.posted.size)
        // The connection is lost: its watch is stopped and says nothing more.
        listeners.last().onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        runCurrent()
        first.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.BLOCKED, 9u))))
        runCurrent()
        assertEquals(1, sink.posted.size)
        // Reconnected: a new watch, whose first view (whatever happened meanwhile) is a baseline again.
        holder.connect(host, byteArrayOf(1))
        listeners.last().onHostStateChanged(HostState.Connected(0u))
        runCurrent()
        val second = ports.last().watches.single().second
        assertNotEquals(first, second)
        second.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.BLOCKED, 10u))))
        runCurrent()
        assertEquals(1, sink.posted.size)
        second.onHerdrStateChanged(HerdrState.Live(view(agent("w1:p1", AgentStatus.DONE, 11u))))
        runCurrent()
        assertEquals("post w1:p1 Done", sink.events.last())
        holder.dismissHost(1)
    }
}

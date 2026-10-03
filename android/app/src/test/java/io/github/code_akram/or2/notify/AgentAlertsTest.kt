package io.github.code_akram.or2.notify

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.connection.FakePort
import io.github.code_akram.or2.connection.FakeTrust
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentSession
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

/**
 * A sink that behaves like the system's notifications: what is up outlives the [AgentAlerts] that posted it (a second
 * instance over the same sink is a new process), and a cancel takes away only what is up. [events] records what was
 * posted and what was really taken away. With [lists] false, reading what is up fails (as the system's call may).
 */
class RecordingSink(var lists: Boolean = true) : AgentAlertSink {
    val events = mutableListOf<String>()
    val posted = mutableListOf<AgentAlert>()
    val up = linkedSetOf<AgentPaneKey>()
    override fun post(alert: AgentAlert) {
        posted += alert
        up += alert.key
        events += "post ${alert.key.paneId} ${alert.text}"
    }

    override fun cancel(key: AgentPaneKey) {
        if (up.remove(key)) events += "cancel ${key.paneId}"
    }

    override fun shown(): Set<AgentPaneKey> = if (lists) up.toSet() else emptySet()
}

@OptIn(ExperimentalCoroutinesApi::class)
class AgentAlertsTest {
    private val sink = RecordingSink()
    private var on = true
    private val alerts = AgentAlerts(sink) { on }
    private val watch = Any()

    /** An agent whose hooks reported [session] (Rust's `reply_identity` then names it), or none. */
    private fun agent(
        pane: String, status: AgentStatus, seq: ULong, name: String? = "Claude Code", terminal: String = "term_$pane",
        session: String? = "sess_$pane",
    ) = HerdrAgent(
        pane, "w1:t1", "w1", name, "claude", name, status, "/work", seq, terminal,
        session?.let { AgentIdentity(terminal, "claude", name, AgentSession("id", it)) },
    )

    private fun view(vararg agents: HerdrAgent, focused: String? = null) =
        HerdrView(1uL, focused, emptyList(), emptyList(), emptyList(), agents.toList())

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
        // Idle (not after Working), Unknown and Working edges never notify.
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 4u)))
        deliver(view(agent("w1:p1", AgentStatus.UNKNOWN, 5u)))
        assertEquals(2, sink.posted.size)
        // A sequence that went backwards (herdr restarted under the watch) is no advance.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 1u)))
        assertEquals(2, sink.posted.size)
    }

    @Test
    fun aTurnHerdrReportsAsIdleNotifiesDone() {
        // herdr reports a finished turn as Idle when the pane counts as seen (or2 focused it): Done all the same.
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 2u)))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 3u)))
        assertEquals(listOf("post w1:p1 Done"), sink.events)
        // Re-deliveries of that Idle post nothing more.
        repeat(2) { deliver(view(agent("w1:p1", AgentStatus.IDLE, 3u), focused = "w1:p9")) }
        assertEquals(1, sink.posted.size)
        // An Unknown between Working and Idle settles nothing: still the end of a turn.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 4u)))
        deliver(view(agent("w1:p1", AgentStatus.UNKNOWN, 5u)))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 6u)))
        assertEquals(listOf("post w1:p1 Done", "cancel w1:p1", "post w1:p1 Done"), sink.events)
    }

    @Test
    fun anIdleNotReachedFromWorkingNeverNotifies() {
        // Baselined Idle, then Unknown and Idle flapping: no turn finished.
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.UNKNOWN, 2u)))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 3u)))
        assertTrue(sink.events.isEmpty())
        // Done, then seen (Idle): the Done was the alert, and it stays up.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 4u)))
        deliver(view(agent("w1:p1", AgentStatus.DONE, 5u)))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 6u)))
        // Blocked, then the dialog dismissed (Idle): the Blocked was the alert.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 7u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 8u)))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 9u)))
        assertEquals(listOf("post w1:p1 Done", "cancel w1:p1", "post w1:p1 Needs input"), sink.events)
    }

    @Test
    fun aTurnWorkingAtTheBaselineThatEndsIdleNotifiesUnlessOnScreen() {
        // Working when the watch started: its end is seen live.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        alerts.screenChanged(OnScreen(1, TerminalTarget.Herdr(null, "w1:p2")))
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 2u), agent("w1:p2", AgentStatus.IDLE, 2u)))
        assertEquals(listOf("post w1:p1 Done"), sink.events)
        // A new watch (a reconnect) baselines again: an Idle it never saw Working is not a turn it saw end.
        val again = Any()
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 2u)), watch = again)
        deliver(view(agent("w1:p1", AgentStatus.IDLE, 3u)), watch = again)
        assertEquals(1, sink.posted.size)
    }

    @Test
    fun theNotificationSaysWhoAndWhatAndWhereNothingFromTheOutput() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u, name = "Codex")))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u, name = "Codex")))
        val posted = sink.posted.single()
        assertEquals(
            AgentAlert(AgentPaneKey(1, null, "w1:p1"), "Codex", "Needs input", "Workstation", agent = AgentIdentity("term_w1:p1", "claude", "Codex", AgentSession("id", "sess_w1:p1"))),
            posted.copy(nonce = null),
        )
        // Each post carries its own Reply capability.
        assertTrue(!posted.nonce.isNullOrEmpty())
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
        sink.up += AgentPaneKey(1, null, "w1:p7")
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
    fun aNewProcessReconcilesTheNotificationsTheOldOneLeftUp() {
        val p1 = AgentPaneKey(1, null, "w1:p1")
        val p2 = AgentPaneKey(1, null, "w1:p2")
        val p3 = AgentPaneKey(1, null, "w1:p3")
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u), agent("w1:p3", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.DONE, 2u), agent("w1:p3", AgentStatus.BLOCKED, 2u)))
        assertEquals(setOf(p1, p2, p3), sink.up)
        // The process died with them up; the next one starts from what the system still shows.
        val next = AgentAlerts(sink) { on }
        assertEquals(setOf(p1, p2, p3), next.active)
        // Its first view of the reconnected session is a baseline (nothing posts), yet it reconciles: p1 went back to
        // work and p2 is gone while the app was dead, so theirs go; p3 still needs input and stays.
        next.viewChanged(Any(), 1, "Workstation", null, view(agent("w1:p1", AgentStatus.WORKING, 3u), agent("w1:p3", AgentStatus.BLOCKED, 2u)))
        assertEquals(setOf(p3), sink.up)
        assertEquals(3, sink.posted.size)
        // The off switch in the new process takes away the rest.
        on = false
        next.enabledChanged()
        assertTrue(sink.up.isEmpty())
        assertTrue(next.active.isEmpty())
    }

    @Test
    fun aPaneSeenWorkingIsCancelledEvenWhenWhatIsUpCannotBeRead() {
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        sink.lists = false
        val next = AgentAlerts(sink) { on }
        assertTrue(next.active.isEmpty())
        // A baseline view: Working means nothing needs input, whatever this process thinks is up.
        next.viewChanged(Any(), 1, "Workstation", null, view(agent("w1:p1", AgentStatus.WORKING, 3u), agent("w1:p2", AgentStatus.WORKING, 1u)))
        assertTrue(sink.up.isEmpty())
        // The off switch asks the system too, for one this process never knew of.
        sink.lists = true
        val another = AgentPaneKey(2, "work", "w1:p4")
        sink.up += another
        on = false
        next.enabledChanged()
        assertTrue(sink.up.isEmpty())
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
        // A notification's tag reads back as its pane (a new process finds what is up); nothing else does.
        for (key in keys) assertEquals(key, AgentPaneKey.fromTag(key.tag))
        for (tag in listOf("agent:1:s5:w1:p1", "agent:1:s9:w1:p1", "agent:x:d:p", "agent:+1:d:p", "agent:1:q:p", "agent:1:s:p", "agent::d:p", "or2:1:d:p", "")) {
            assertNull(tag, AgentPaneKey.fromTag(tag))
        }
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
    fun aTapOpensWhateverTheSavedStateAndARecreationNeverRepeatsOne() {
        val pane = AgentPaneKey(1, null, "w1:p1")
        val other = AgentPaneKey(2, "work", "w1:p2")
        // A cold start (no saved state) on a tap: opened, once.
        val first = AgentTaps(null)
        assertEquals(pane, first.take(pane, "tap-1", fromHistory = false))
        assertNull(first.take(pane, "tap-1", fromHistory = false))
        // A rotation (or a restore after process death) hands the same intent to the new instance: not again.
        val rotated = AgentTaps(first.saved())
        assertNull(rotated.take(pane, "tap-1", fromHistory = false))
        // Android restored the task after killing the process and created the activity, with its saved state, for a
        // new tap (no live activity to get onNewIntent): opened.
        assertEquals(other, rotated.take(other, "tap-2", fromHistory = false))
        // Saved state from an activity started from the launcher (no tap taken), then a tap: opened.
        assertEquals(pane, AgentTaps(emptyArray()).take(pane, "tap-3", fromHistory = false))
        // Both taps survive the next recreation.
        val again = AgentTaps(rotated.saved())
        assertNull(again.take(pane, "tap-1", fromHistory = false))
        assertNull(again.take(other, "tap-2", fromHistory = false))
        // Relaunched from Recents with a tap as the task's intent: an old tap, not opened.
        assertNull(AgentTaps(null).take(pane, "tap-4", fromHistory = true))
        // Not a tap (no pane or no id): nothing.
        assertNull(AgentTaps(null).take(null, "tap-5", fromHistory = false))
        assertNull(AgentTaps(null).take(pane, null, fromHistory = false))
        // The remembered ids stay few: the oldest go first, except the first of all.
        val many = AgentTaps(null)
        repeat(100) { many.take(pane, "t$it", fromHistory = false) }
        assertTrue(many.saved().size <= 32)
        assertNull(AgentTaps(many.saved()).take(pane, "t99", fromHistory = false))
        assertNull(AgentTaps(many.saved()).take(pane, "t0", fromHistory = false)) // Maybe the intent that created it.
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
    fun aPaneThatNowHoldsAnotherAgentLosesItsNotification() {
        val key = AgentPaneKey(1, null, "w1:p1")
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u, terminal = "term_a")))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u, terminal = "term_a")))
        assertEquals(AgentIdentity("term_a", "claude", "Claude Code", AgentSession("id", "sess_w1:p1")), sink.posted.single().agent)
        // The same pane id, a new terminal (herdr restarted and numbered its panes again), still blocked with no new
        // edge: the notification was about an agent that is gone.
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u, terminal = "term_b")))
        assertFalse(key in sink.up)
        assertEquals(listOf("post w1:p1 Needs input", "cancel w1:p1"), sink.events)
        // Another kind of agent in the same terminal goes the same way; the same agent keeps its notification.
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u, terminal = "term_b")))
        deliver(view(agent("w1:p1", AgentStatus.DONE, 3u, terminal = "term_b")))
        assertTrue(key in sink.up)
        deliver(view(HerdrAgent("w1:p1", "w1:t1", "w1", "Codex", "codex", "Codex", AgentStatus.DONE, "/work", 3u, "term_b")))
        assertFalse(key in sink.up)
    }

    @Test
    fun anotherInstanceOfTheSameKindInTheSameTerminalLosesItsNotification() {
        // A Codex exits and another starts in the same terminal: the same terminal and kind, another session, or
        // none yet (its hooks have not reported one). Either way the notification was about the one that is gone.
        for (replacement in listOf("sess_next", null)) {
            val sink = RecordingSink()
            val alerts = AgentAlerts(sink) { on }
            val key = AgentPaneKey(1, null, "w1:p1")
            alerts.viewChanged(watch, 1, "Workstation", null, view(agent("w1:p1", AgentStatus.WORKING, 1u)))
            alerts.viewChanged(watch, 1, "Workstation", null, view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
            // The same instance keeps it.
            alerts.viewChanged(watch, 1, "Workstation", null, view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
            assertTrue(key in sink.up)
            alerts.viewChanged(watch, 1, "Workstation", null, view(agent("w1:p1", AgentStatus.BLOCKED, 2u, session = replacement)))
            assertFalse("$replacement", key in sink.up)
            assertEquals(listOf("post w1:p1 Needs input", "cancel w1:p1"), sink.events)
        }
    }

    @Test
    fun anAgentHerdrDoesNotIdentifyIsNotifiedWithoutAReply() {
        // A reported agent with no session and no herdr name: Rust gives it no `reply_identity`, so the alert names
        // no agent, and its notification has no Reply action (AgentNotifications adds one only with an agent).
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u, session = null)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u, session = null)))
        assertNull(sink.posted.single().agent)
        // Nothing absent is a wildcard: an alert's instance never matches an agent without one.
        val was = AgentIdentity("term_w1:p1", "claude", null, AgentSession("id", "s"))
        assertFalse(sameAgent(was, agent("w1:p1", AgentStatus.BLOCKED, 2u, session = null)))
        assertFalse(sameAgent(was.copy(agent = null), agent("w1:p1", AgentStatus.BLOCKED, 2u, session = "s")))
        assertFalse(sameAgent(was.copy(session = null), agent("w1:p1", AgentStatus.BLOCKED, 2u, session = "s")))
        assertTrue(sameAgent(was, agent("w1:p1", AgentStatus.BLOCKED, 2u, session = "s")))
        // An agent herdr started is named by its name.
        val started = AgentIdentity("term_w1:p1", "claude", "reviewer", null)
        assertTrue(sameAgent(started, agent("w1:p1", AgentStatus.BLOCKED, 2u, name = "reviewer", session = null)))
        assertFalse(sameAgent(started, agent("w1:p1", AgentStatus.BLOCKED, 2u, name = "other", session = null)))
        assertFalse(sameAgent(started, agent("w1:p1", AgentStatus.BLOCKED, 2u, name = null, session = null)))
    }

    @Test
    fun aReplyCapabilityIsTakenOnceOnlyWhileItsNotificationIsUpAndNeverAfterANewerPost() {
        val nonces = ReplyNonces(MemoryPrefStore())
        val alerts = AgentAlerts(sink, nonces) { on }
        val key = AgentPaneKey(1, null, "w1:p1")
        fun deliver(view: HerdrView) = alerts.viewChanged(watch, 1, "Workstation", null, view)
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 1u)))
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 2u)))
        val first = sink.posted.last().nonce
        assertTrue(alerts.admitReply(key, first))
        // Replayed: refused, and so is no capability at all, or another pane's.
        assertFalse(alerts.admitReply(key, first))
        assertFalse(alerts.admitReply(key, null))
        // The outcome re-posts with a new capability; the old one stays dead.
        alerts.replied(sink.posted.last().copy(outcome = "Not sent: the agent is gone", nonce = null))
        val second = sink.posted.last().nonce
        assertNotEquals(first, second)
        assertFalse(alerts.admitReply(key, first))
        assertFalse(alerts.admitReply(AgentPaneKey(1, null, "w1:p2"), second))
        // A newer edge replaces it again.
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 3u)))
        deliver(view(agent("w1:p1", AgentStatus.DONE, 4u)))
        val third = sink.posted.last().nonce
        assertFalse(alerts.admitReply(key, second))
        // A notification no longer up (dismissed by the user, so no cancel reached the app) takes no reply.
        sink.up -= key
        assertFalse(alerts.admitReply(key, third))
        // Cancelled by an edge: the capability is revoked, even if the notification were shown again.
        sink.up += key
        deliver(view(agent("w1:p1", AgentStatus.WORKING, 5u)))
        sink.up += key
        assertFalse(alerts.admitReply(key, third))
        // A new process keeps the capability of a notification the system still shows (the store outlives it).
        deliver(view(agent("w1:p1", AgentStatus.BLOCKED, 6u)))
        val fourth = sink.posted.last().nonce
        assertTrue(AgentAlerts(sink, nonces) { on }.admitReply(key, fourth))
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

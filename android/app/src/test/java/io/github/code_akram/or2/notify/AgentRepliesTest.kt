package io.github.code_akram.or2.notify

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.ReplyRoute
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A notification's Reply ([AgentReplyReceiver] hands it to [AgentReplies]): its capability taken once, not connected,
 * sent, not sent (the Reply action stays: every agent notification is built with it), and the text kept out of
 * everything but the quote.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class AgentRepliesTest {
    private val key = AgentPaneKey(7, "work", "w1:p1")
    private val claude = AgentIdentity("term_1", "claude")
    private val alert = AgentAlert(key, "Claude Code", "Needs input", "Workstation", agent = claude)
    private val posted = mutableListOf<AgentAlert>()
    private val sent = mutableListOf<Pair<AgentPaneKey, String>>()
    private val agents = mutableListOf<AgentIdentity>()

    /** What `HostConnections.replyToPane` does next. */
    private var answer: suspend () -> ReplyRoute = { ReplyRoute.TYPED }

    /** `AgentAlerts.admitReply`: every capability is good unless a test says otherwise. */
    private var admit: (AgentPaneKey, String?) -> Boolean = { _, _ -> true }

    private fun TestScope.replies() = AgentReplies(
        this,
        admit = { key, nonce -> admit(key, nonce) },
        send = { key, agent, text -> sent += key to text; agents += agent; answer() },
        post = { posted += it },
    )

    @Test
    fun aHostThatIsNotConnectedSaysSoAndKeepsTheReply() = runTest {
        answer = { throw HostException.NotConnected() }
        replies().reply(alert, "go on")
        assertEquals(listOf(key to "go on"), sent)
        val update = posted.single()
        assertEquals(alert.copy(outcome = "Not sent: Workstation is not connected"), update)
        assertNull("nothing was sent, so nothing is quoted", update.reply)
        // A connection that closed meanwhile says the same.
        answer = { throw HostException.Closed() }
        replies().reply(alert, "go on")
        assertEquals("Not sent: Workstation is not connected", posted.last().outcome)
    }

    @Test
    fun aSentReplyUpdatesTheNotificationToSentWithTheReplyQuoted() = runTest {
        for (route in ReplyRoute.entries) {
            answer = { route }
            replies().reply(alert, "yes\nand add a test")
            assertEquals(alert.copy(outcome = "Sent", reply = "yes\nand add a test"), posted.last())
        }
        assertEquals(2, sent.size)
        // The reply names the agent the notification was about.
        assertEquals(listOf(claude, claude), agents)
        // The update keeps what the alert said and its pane: the same notification, quieted.
        assertEquals(key, posted.last().key)
        assertEquals("Needs input", posted.last().text)
    }

    @Test
    fun aFailureShowsTheReasonAndKeepsTheReplyAction() = runTest {
        val reasons = listOf(
            HostException.PaneNotFound() to "Not sent: the agent is gone",
            HostException.TooLarge() to "Not sent: the reply is too long",
            HostException.NotInstalled("herdr") to "Not sent: herdr is not installed on Workstation",
            HostException.CommandFailed("the host did not answer in time") to "Not sent: the host did not answer in time",
            HostException.CommandFailed("x".repeat(200)) to "Not sent: " + "x".repeat(80),
            HostException.InvalidName() to "Not sent: herdr refused it",
        )
        for ((error, reason) in reasons) {
            answer = { throw error }
            replies().reply(alert, "hello")
            assertEquals(alert.copy(outcome = reason), posted.last())
        }
        // Each failure re-posts the pane's notification (built with its Reply action), never cancels it.
        assertEquals(reasons.size, posted.size)
        assertTrue(posted.all { it.key == key && it.reply == null })
    }

    @Test
    fun anEmptyReplyIsNotSentAndAHostThatNeverAnswersTimesOut() = runTest {
        for (blank in listOf(null, "", "  \n")) replies().reply(alert, blank)
        assertTrue(sent.isEmpty())
        assertTrue(posted.all { it.outcome == AgentReplies.NOT_SENT_EMPTY })
        answer = { CompletableDeferred<ReplyRoute>().await() }
        replies().reply(alert, "hello")
        assertEquals(AgentReplies.NOT_SENT_TIMEOUT, posted.last().outcome)
    }

    @Test
    fun aReplyThatNamesNoAgentIsNotSent() = runTest {
        replies().reply(alert.copy(agent = null), "hello")
        assertTrue(sent.isEmpty())
        assertEquals(AgentReplies.NOT_SENT_GONE, posted.single().outcome)
    }

    /**
     * Codex v0.1.2 P2 #3: a Reply PendingIntent kept by someone else (a notification listener) and sent again, or one
     * from before a newer post, sends nothing: the capability is taken once, before anything is sent.
     */
    @Test
    fun aReplayedOrStaleReplySendsNothing() = runTest {
        val sink = RecordingSink()
        val alerts = AgentAlerts(sink, ReplyNonces(MemoryPrefStore()))
        admit = alerts::admitReply
        val replies = AgentReplies(this, admit, { k, a, t -> sent += k to t; agents += a; answer() }, alerts::replied)
        val watch = Any()
        fun view(status: AgentStatus, seq: ULong) = HerdrView(
            1uL, 22u, null, emptyList(), emptyList(), emptyList(),
            listOf(HerdrAgent(key.paneId, "w1:t1", "w1", "Claude Code", "claude", "Claude Code", status, "/work", null, false, seq, "term_1")),
        )
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.WORKING, 1u))
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.BLOCKED, 2u))
        // What the Reply intent carries: the posted alert, its capability included.
        val intent = sink.posted.last()
        replies.reply(intent, "yes")
        assertEquals(listOf(key to "yes"), sent)
        assertEquals(listOf(claude), agents)
        val outcomes = sink.posted.size
        // Sent again (a replay): nothing is sent, nothing changes.
        replies.reply(intent, "rm -rf ~")
        assertEquals(1, sent.size)
        assertEquals(outcomes, sink.posted.size)
        // The outcome's re-post works once, with its own capability.
        replies.reply(sink.posted.last(), "and the tests")
        assertEquals(2, sent.size)
        // Taken away (the agent went back to work): its capability goes too.
        val last = sink.posted.last()
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.WORKING, 3u))
        sink.up += key
        replies.reply(last, "late")
        assertEquals(2, sent.size)
    }

    @Test
    fun launchTakesTheCapabilityBeforeItReturnsAndNoBroadcastWaitsForTheReply() = runTest {
        val taken = mutableListOf<String?>()
        admit = { _, nonce -> taken += nonce; true }
        val gate = CompletableDeferred<ReplyRoute>()
        answer = { gate.await() }
        // On the application's main-thread scope (immediate), as the receiver uses it.
        val scope = CoroutineScope(UnconfinedTestDispatcher(testScheduler))
        val replies = AgentReplies(scope, admit, { k, _, t -> sent += k to t; answer() }, { posted += it })
        replies.launch(alert.copy(nonce = "n1"), "hello")
        // Taken and sent before launch returned: the reply now only waits for the host.
        assertEquals(listOf<String?>("n1"), taken)
        assertEquals(listOf(key to "hello"), sent)
        assertTrue(posted.isEmpty())
        gate.complete(ReplyRoute.PROMPTED)
        advanceUntilIdle()
        assertEquals("Sent", posted.single().outcome)
        assertNull("the update carries no capability of its own", posted.single().nonce)
    }

    @Test
    fun theReplyIsNeverPrinted() = runTest {
        replies().reply(alert.copy(nonce = "secret-capability"), "a private reply")
        val update = posted.single()
        assertEquals("a private reply", update.reply)
        assertFalse(update.toString().contains("private"))
        assertTrue(update.toString().contains("Sent"))
        assertFalse(alert.copy(nonce = "secret-capability").toString().contains("secret"))
    }

    @Test
    fun aReplyIntentNamesItsPaneAndCapabilityByItsDataAndItsAgentByItsExtras() {
        val action = AgentNotifications.ACTION_REPLY
        val reply = agentReplyFrom(action, key.tag, "n1", "term_1", "claude", "Claude Code", "Needs input", "Workstation")
        assertEquals(alert.copy(nonce = "n1"), reply)
        assertNull(agentReplyFrom(AgentNotifications.ACTION_OPEN_AGENT, key.tag, "n1", "term_1", null, "t", "x", "h"))
        assertNull(agentReplyFrom(action, null, "n1", "term_1", null, "t", "x", "h"))
        assertNull(agentReplyFrom(action, "not-a-tag", "n1", "term_1", null, "t", "x", "h"))
        assertNull(agentReplyFrom(action, AgentPaneKey(0, null, "w1:p1").tag, "n1", "term_1", null, "t", "x", "h"))
        // What is only shown has defaults; no terminal names no agent (such a reply is not sent).
        assertEquals(
            AgentAlert(key, "Agent", "", "", agent = null, nonce = null),
            agentReplyFrom(action, key.tag, null, null, "claude", null, null, null),
        )
        assertEquals(AgentIdentity("term_1", null), agentReplyFrom(action, key.tag, "n", "term_1", null, null, null, null)?.agent)
    }

    @Test
    fun theOutcomeReplacesANotificationStillUpButNotOneTakenAway() {
        val sink = RecordingSink()
        val alerts = AgentAlerts(sink)
        val watch = Any()
        fun view(status: AgentStatus, seq: ULong) = HerdrView(
            1uL, 22u, null, emptyList(), emptyList(), emptyList(),
            listOf(HerdrAgent(key.paneId, "w1:t1", "w1", "Claude Code", "claude", "Claude Code", status, "/work", null, false, seq, "term_1")),
        )
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.WORKING, 1u))
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.BLOCKED, 2u))
        alerts.replied(alert.copy(outcome = "Sent", reply = "go"))
        assertEquals(alert.copy(outcome = "Sent", reply = "go"), sink.posted.last().copy(nonce = null))
        // The agent went back to work (the reply did it): the notification goes, and a late outcome stays away.
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.WORKING, 3u))
        assertFalse(key in sink.up)
        alerts.replied(alert.copy(outcome = "Not sent: the agent is gone"))
        assertFalse(key in sink.up)
        assertEquals(2, sink.posted.size)
        // One a dead process left up (the system still shows it) is replaced.
        val other = AgentPaneKey(7, null, "w2:p1")
        sink.up += other
        AgentAlerts(sink).replied(AgentAlert(other, "Codex", "Done", "Workstation", outcome = "Not sent: Workstation is not connected"))
        assertEquals(other, sink.posted.last().key)
    }
}

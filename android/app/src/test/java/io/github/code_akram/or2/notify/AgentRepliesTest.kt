package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.ReplyRoute
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A notification's Reply ([AgentReplyReceiver] hands it to [AgentReplies]): not connected, sent, not sent (the Reply
 * action stays: every agent notification is built with it), and the text kept out of everything but the quote.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class AgentRepliesTest {
    private val key = AgentPaneKey(7, "work", "w1:p1")
    private val alert = AgentAlert(key, "Claude Code", "Needs input", "Workstation")
    private val posted = mutableListOf<AgentAlert>()
    private val sent = mutableListOf<Pair<AgentPaneKey, String>>()

    /** What `HostConnections.replyToPane` does next. */
    private var answer: suspend () -> ReplyRoute = { ReplyRoute.TYPED }

    private fun TestScope.replies() = AgentReplies(this, { key, text -> sent += key to text; answer() }, { posted += it })

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
    fun launchFinishesTheBroadcastWhateverHappened() = runTest {
        var finished = 0
        answer = { throw HostException.PaneNotFound() }
        replies().launch(alert, "hello") { finished++ }
        advanceUntilIdle()
        assertEquals(1, finished)
        answer = { ReplyRoute.PROMPTED }
        replies().launch(alert, "hello") { finished++ }
        advanceUntilIdle()
        assertEquals(2, finished)
        assertEquals("Sent", posted.last().outcome)
    }

    @Test
    fun theReplyIsNeverPrinted() = runTest {
        replies().reply(alert, "a private reply")
        val update = posted.single()
        assertEquals("a private reply", update.reply)
        assertFalse(update.toString().contains("private"))
        assertTrue(update.toString().contains("Sent"))
    }

    @Test
    fun aReplyIntentNamesItsPaneByItsTagOnly() {
        val reply = agentReplyFrom(AgentNotifications.ACTION_REPLY, key.tag, "Claude Code", "Needs input", "Workstation")
        assertEquals(alert, reply)
        assertNull(agentReplyFrom(AgentNotifications.ACTION_OPEN_AGENT, key.tag, "t", "x", "h"))
        assertNull(agentReplyFrom(AgentNotifications.ACTION_REPLY, null, "t", "x", "h"))
        assertNull(agentReplyFrom(AgentNotifications.ACTION_REPLY, "not-a-tag", "t", "x", "h"))
        assertNull(agentReplyFrom(AgentNotifications.ACTION_REPLY, AgentPaneKey(0, null, "w1:p1").tag, "t", "x", "h"))
        // What is only shown has defaults.
        assertEquals(AgentAlert(key, "Agent", "", ""), agentReplyFrom(AgentNotifications.ACTION_REPLY, key.tag, null, null, null))
    }

    @Test
    fun theOutcomeReplacesANotificationStillUpButNotOneTakenAway() {
        val sink = RecordingSink()
        val alerts = AgentAlerts(sink)
        val watch = Any()
        fun view(status: AgentStatus, seq: ULong) = HerdrView(
            1uL, 22u, null, emptyList(), emptyList(), emptyList(),
            listOf(HerdrAgent(key.paneId, "w1:t1", "w1", "Claude Code", "claude", "Claude Code", status, "/work", null, false, seq)),
        )
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.WORKING, 1u))
        alerts.viewChanged(watch, key.hostId, "Workstation", key.session, view(AgentStatus.BLOCKED, 2u))
        alerts.replied(alert.copy(outcome = "Sent", reply = "go"))
        assertEquals(alert.copy(outcome = "Sent", reply = "go"), sink.posted.last())
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

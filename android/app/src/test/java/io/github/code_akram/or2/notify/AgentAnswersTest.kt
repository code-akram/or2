package io.github.code_akram.or2.notify

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentSession
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.PermissionAnswer
import io.github.code_akram.or2.ffi.PermissionPrompt
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Approve and Deny from a `Needs input` notification ([AgentAnswers]): the prompt found behind it (Claude Code only),
 * the actions it gains (the phone unlocked, the post's capability, the prompt's seq), the answer taken once and its
 * outcome in words.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class AgentAnswersTest {
    private val key = AgentPaneKey(7, "work", "w1:p1")
    private val claude = AgentIdentity("term_w1:p1", "claude", "Claude Code", AgentSession("id", "sess_w1:p1"))
    private val sink = RecordingSink()
    private val alerts = AgentAlerts(sink, ReplyNonces(MemoryPrefStore()))
    private val watch = Any()

    /** `HostConnections.permissionPrompt` calls, and what it answers next. */
    private val asked = mutableListOf<Pair<AgentPaneKey, AgentIdentity>>()
    private var prompt: suspend () -> PermissionPrompt? = { PermissionPrompt(4u) }

    /** `HostConnections.answerPermission` calls, and what it does next. */
    private val sent = mutableListOf<Triple<AgentPaneKey, ULong, PermissionAnswer>>()
    private var send: suspend () -> Unit = {}

    private fun TestScope.answers() = AgentAnswers(
        this,
        admit = alerts::admitReply,
        ask = { key, agent -> asked += key to agent; prompt() },
        send = { key, _, seq, answer -> sent += Triple(key, seq, answer); send() },
        found = alerts::permissionFound,
        post = alerts::replied,
    )

    private fun agent(status: AgentStatus, seq: ULong, kind: String = "claude", identity: AgentIdentity? = claude) = HerdrAgent(
        key.paneId, "w1:t1", "w1", "Claude Code", kind, "Claude Code", status, "/work", seq, "term_w1:p1",
        identity?.copy(agent = kind), "Fixing the build",
    )

    private fun deliver(agent: HerdrAgent) = alerts.viewChanged(
        watch, key.hostId, "Workstation", key.session,
        HerdrView(1uL, null, emptyList(), emptyList(), emptyList(), listOf(agent)),
    )

    /** A `Needs input` edge of [agent]'s kind: posted, then asked about with [answers] ([AgentAlerts.askPermission]). */
    private fun TestScope.needsInput(answers: AgentAnswers, kind: String = "claude", identity: AgentIdentity? = claude) {
        alerts.askPermission = { alert -> answers.check(alert) }
        deliver(agent(AgentStatus.WORKING, 3u, kind, identity))
        deliver(agent(AgentStatus.BLOCKED, 4u, kind, identity))
        testScheduler.advanceUntilIdle()
    }

    @Test
    fun aClaudeCodeAtAPermissionPromptGainsApproveAndDenyWithoutAlertingAgain() = runTest {
        needsInput(answers())
        assertEquals(listOf(key to claude), asked)
        val (first, second) = sink.posted
        assertEquals("Needs input · Fixing the build", first.text)
        assertNull(first.permission)
        // The same notification, re-posted: it says so and names the prompt by its seq, with a new capability.
        assertEquals(first.copy(text = "Needs permission · Fixing the build", permission = 4u, nonce = second.nonce), second)
        assertNotNull(second.nonce)
        assertNotEquals(first.nonce, second.nonce)
        assertEquals(listOf("Approve", "Deny", "Reply"), alertActions(second).map { it.label })
        assertEquals(listOf("Reply"), alertActions(first).map { it.label })
    }

    @Test
    fun approveAndDenyNeedTheUnlockedPhoneAndCarryTheCapabilityAndTheSeq() {
        val alert = AgentAlert(key, "Claude Code", "Needs permission", "Workstation", agent = claude, nonce = "n1", permission = 9u)
        val actions = alertActions(alert)
        assertEquals(
            listOf(
                AlertAction(AlertActionKind.APPROVE, "Approve", true, "n1", 9u),
                AlertAction(AlertActionKind.DENY, "Deny", true, "n1", 9u),
                AlertAction(AlertActionKind.REPLY, "Reply", false, "n1", null),
            ),
            actions,
        )
        // No prompt, or no agent instance: no Approve or Deny.
        assertEquals(listOf(AlertActionKind.REPLY), alertActions(alert.copy(permission = null)).map { it.kind })
        assertTrue(alertActions(alert.copy(agent = null)).isEmpty())
        // The capability never reaches a log.
        assertTrue("n1" !in actions.joinToString())
    }

    @Test
    fun anAnswerIntentNamesItsAnswerAlertSeqAndCapability() {
        val request = agentAnswerFrom(
            AgentNotifications.ACTION_APPROVE, key.tag, "n1", "term_w1:p1", "claude", "Claude Code", "id", "sess_w1:p1", 9,
            "Claude Code", "Needs permission", "Workstation",
        )
        assertEquals(PermissionAnswer.APPROVE, request?.answer)
        assertEquals(
            AgentAlert(key, "Claude Code", "Needs permission", "Workstation", agent = claude, nonce = "n1", permission = 9u),
            request?.alert,
        )
        val deny = agentAnswerFrom(
            AgentNotifications.ACTION_DENY, key.tag, "n1", "term_w1:p1", "claude", null, "id", "sess_w1:p1", 9, null, null, null,
        )
        assertEquals(PermissionAnswer.DENY, deny?.answer)
        // Another action, no seq, or not a pane: nothing.
        for (none in listOf(
            agentAnswerFrom(AgentNotifications.ACTION_REPLY, key.tag, "n1", "t", "claude", null, "id", "s", 9, null, null, null),
            agentAnswerFrom(AgentNotifications.ACTION_APPROVE, key.tag, "n1", "t", "claude", null, "id", "s", null, null, null, null),
            agentAnswerFrom(AgentNotifications.ACTION_APPROVE, key.tag, "n1", "t", "claude", null, "id", "s", -1, null, null, null),
            agentAnswerFrom(AgentNotifications.ACTION_APPROVE, "elsewhere", "n1", "t", "claude", null, "id", "s", 9, null, null, null),
        )) assertNull(none)
    }

    @Test
    fun noPromptAnotherKindOrAFailedAskLeavesTheNotificationAsItWas() = runTest {
        prompt = { null }
        needsInput(answers())
        assertEquals(1, sink.posted.size)
        // Codex's blocked states mix permission prompts with questions: not asked at all.
        needsInput(answers(), kind = "codex")
        assertEquals(1, asked.size)
        // An agent without a Reply identity cannot be named: not asked.
        needsInput(answers(), identity = null)
        assertEquals(1, asked.size)
        // Not connected (or any failure): nothing changes.
        for (failure in listOf(HostException.NotConnected(), HostException.PaneNotFound(), HostException.CommandFailed("x"))) {
            prompt = { throw failure }
            val before = sink.posted.size
            needsInput(answers())
            assertEquals(before + 1, sink.posted.size)
            assertNull(sink.posted.last().permission)
        }
    }

    @Test
    fun aPromptFoundAfterTheNotificationChangedIsNotShown() = runTest {
        val found = CompletableDeferred<PermissionPrompt?>()
        prompt = { found.await() }
        needsInput(answers())
        // The agent works again before herdr answered: its notification went.
        deliver(agent(AgentStatus.WORKING, 5u))
        found.complete(PermissionPrompt(4u))
        testScheduler.advanceUntilIdle()
        assertTrue(sink.posted.none { it.permission != null })
        assertTrue(key !in sink.up)
    }

    @Test
    fun anAnswerIsSentOnceAndTheOutcomeReplacesTheActions() = runTest {
        for ((choice, outcome) in listOf(PermissionAnswer.APPROVE to "Approved", PermissionAnswer.DENY to "Denied")) {
            sink.posted.clear()
            sent.clear()
            val answers = answers()
            needsInput(answers)
            val shown = sink.posted.last()
            answers.answer(shown, choice)
            assertEquals(listOf(Triple(key, 4uL, choice)), sent)
            val update = sink.posted.last()
            assertEquals(outcome, update.outcome)
            assertNull("Approve and Deny go", update.permission)
            assertEquals(listOf("Reply"), alertActions(update).map { it.label })
            // The same PendingIntent again (a replay, or the other button of the same post): nothing.
            answers.answer(shown, PermissionAnswer.APPROVE)
            assertEquals(1, sent.size)
            deliver(agent(AgentStatus.WORKING, 5u))
        }
    }

    @Test
    fun aFailedAnswerSaysWhyInWords() = runTest {
        val reasons = listOf(
            HostException.PromptChanged() to "The prompt changed. Open the pane.",
            HostException.PaneNotFound() to "Not answered: the agent is gone",
            HostException.NotConnected() to "Not answered: Workstation is not connected",
            HostException.Closed() to "Not answered: Workstation is not connected",
            HostException.NotInstalled("herdr") to "Not answered: herdr is not installed on Workstation",
            HostException.CommandFailed("x".repeat(200)) to "Not answered: " + "x".repeat(80),
            HostException.InvalidName() to "Not answered: herdr refused it",
        )
        for ((error, reason) in reasons) {
            send = { throw error }
            val answers = answers()
            needsInput(answers)
            answers.answer(sink.posted.last(), PermissionAnswer.APPROVE)
            assertEquals(reason, sink.posted.last().outcome)
            assertNull(sink.posted.last().permission)
            deliver(agent(AgentStatus.WORKING, 5u))
        }
        // A host that never answers.
        send = { CompletableDeferred<Unit>().await() }
        val answers = answers()
        needsInput(answers)
        answers.answer(sink.posted.last(), PermissionAnswer.DENY)
        assertEquals(AgentAnswers.NOT_ANSWERED_TIMEOUT, sink.posted.last().outcome)
    }

    @Test
    fun anAnswerWithoutAPromptOrAnAgentIsNotSent() = runTest {
        val answers = answers()
        needsInput(answers)
        val shown = sink.posted.last()
        answers.answer(shown.copy(permission = null), PermissionAnswer.APPROVE)
        assertTrue(sent.isEmpty())
        assertEquals(AgentAnswers.NOT_ANSWERED_OPEN_PANE, sink.posted.last().outcome)
    }
}

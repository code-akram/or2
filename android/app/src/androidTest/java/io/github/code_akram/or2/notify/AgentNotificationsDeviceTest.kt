package io.github.code_akram.or2.notify

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.pm.PackageManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentSession
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.service.ConnectionService
import io.github.code_akram.or2.service.ServiceRunState
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** The fixture's agent instance: its hooks reported a session. */
private val CLAUDE = AgentIdentity("term_1", "claude", "Claude Code", AgentSession("id", "sess_1"))

/**
 * Agent notifications on a device: the service creates the `agents` channel with its own, and a Blocked edge posts
 * one notification for its pane (title, text, host, a tap that opens or2), which opening the pane cancels. The
 * posting half runs only where the app already holds `POST_NOTIFICATIONS` (the test never grants it, as in
 * `ConnectionServiceDeviceTest`); without it, it is skipped with a message.
 */
@RunWith(AndroidJUnit4::class)
class AgentNotificationsDeviceTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val key = AgentPaneKey(9101, "devicetest", "w1:p1")

    @After
    fun tearDown() {
        manager.cancel(key.tag, AgentNotifications.NOTIFICATION_ID)
    }

    private fun await(what: String, timeoutMs: Long = 10_000, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (!condition()) {
            if (System.currentTimeMillis() > deadline) throw AssertionError("Timed out waiting for $what")
            Thread.sleep(50)
        }
    }

    private fun posted(): Notification? = manager.activeNotifications
        .firstOrNull { it.tag == key.tag && it.id == AgentNotifications.NOTIFICATION_ID }?.notification

    private fun view(status: AgentStatus, seq: ULong) = HerdrView(
        1uL, null, emptyList(), emptyList(), emptyList(),
        listOf(HerdrAgent(key.paneId, "w1:t1", "w1", "Claude Code", "claude", "Claude Code", status, "/work", seq, "term_1", CLAUDE)),
    )

    @Test
    fun theServiceCreatesTheAgentsChannel() {
        val begun = ServiceRunState.Process.begins.get()
        context.startForegroundService(ConnectionService.startIntent(context))
        await("the service to start") { ServiceRunState.Process.begins.get() > begun }
        val channel = manager.getNotificationChannel(AgentNotifications.CHANNEL_ID)
        assertNotNull("the agents channel", channel)
        assertEquals("Agents", channel.name.toString())
        await("the service to stop itself") { !ServiceRunState.Process.running }
    }

    /** Built, not posted: needs no `POST_NOTIFICATIONS`. */
    @Test
    fun anIdentifiedAgentsNotificationHasAReplyActionWithARemoteInput() {
        val notifications = AgentNotifications(context, MemoryPrefStore())
        val alert = AgentAlert(key, "Claude Code", "Needs input", "Device fixture", agent = CLAUDE, nonce = "n1")
        // An agent herdr does not identify (no session, no name): no Reply at all, only the tap that opens the pane.
        assertTrue(notifications.build(alert.copy(agent = null)).actions.isNullOrEmpty())
        for (shown in listOf(alert, alert.copy(outcome = "Not sent: Device fixture is not connected"), alert.copy(outcome = "Sent", reply = "go on"))) {
            val built = notifications.build(shown)
            val reply = built.actions.orEmpty().single()
            assertEquals("Reply", reply.title.toString())
            assertEquals(Notification.Action.SEMANTIC_ACTION_REPLY, reply.semanticAction)
            val input = reply.remoteInputs.single()
            assertEquals(AgentNotifications.KEY_REPLY, input.resultKey)
            assertEquals("Reply to Claude Code", input.label.toString())
            assertTrue(input.allowFreeFormInput)
            // RemoteInput needs a mutable PendingIntent; it is a broadcast to the app's own receiver.
            assertTrue(reply.actionIntent.isBroadcast)
            assertFalse(reply.actionIntent.isImmutable)
            assertEquals(context.packageName, reply.actionIntent.creatorPackage)
        }
        // The intent names the pane and the capability in its data, the agent in its extras, and is honoured;
        // anything else is not.
        val intent = AgentNotifications.replyIntent(context, alert)
        assertEquals(AgentReplyReceiver::class.java.name, intent.component?.className)
        assertEquals(alert, AgentNotifications.replyOf(intent))
        assertEquals("n1", intent.data?.fragment)
        // Each post's capability makes its own PendingIntent.
        val first = notifications.build(alert).actions.single().actionIntent
        val second = notifications.build(alert.copy(nonce = "n2")).actions.single().actionIntent
        assertNotEquals(first, second)
        assertNull(AgentNotifications.replyOf(AgentNotifications.openIntent(context, key, "token")))
        // After a reply the notification does not alert again, and a sent one quotes the reply.
        val sent = notifications.build(alert.copy(outcome = "Sent", reply = "go on"))
        assertTrue(sent.flags and Notification.FLAG_ONLY_ALERT_ONCE != 0)
        val messages = sent.extras.getParcelableArray(Notification.EXTRA_MESSAGES, android.os.Parcelable::class.java).orEmpty()
        assertEquals(2, messages.size)
        assertEquals("Not sent: Device fixture is not connected",
            notifications.build(alert.copy(outcome = "Not sent: Device fixture is not connected")).extras.getCharSequence(Notification.EXTRA_TEXT).toString())
    }

    /** Built, not posted: needs no `POST_NOTIFICATIONS`. */
    @Test
    fun aPermissionPromptsNotificationHasApproveAndDenyThatNeedTheUnlockedPhone() {
        val notifications = AgentNotifications(context, MemoryPrefStore())
        val alert = AgentAlert(key, "Claude Code", "Needs permission", "Device fixture", agent = CLAUDE, nonce = "n1", permission = 4u)
        val built = notifications.build(alert)
        val actions = built.actions.orEmpty()
        assertEquals(listOf("Approve", "Deny", "Reply"), actions.map { it.title.toString() })
        for (answer in actions.take(2)) {
            assertTrue(answer.isAuthenticationRequired)
            assertTrue(answer.remoteInputs.isNullOrEmpty())
            // A broadcast to the app's own receiver, immutable: nothing in it can be filled in.
            assertTrue(answer.actionIntent.isBroadcast)
            assertTrue(answer.actionIntent.isImmutable)
            assertEquals(context.packageName, answer.actionIntent.creatorPackage)
        }
        assertFalse(actions[2].isAuthenticationRequired)
        // Found behind a `Needs input` already shown: it does not alert again.
        assertTrue(built.flags and Notification.FLAG_ONLY_ALERT_ONCE != 0)
        // The intents name the answer, the pane, the capability and the seq, and are honoured; a Reply's is not one.
        for (answer in listOf(io.github.code_akram.or2.ffi.PermissionAnswer.APPROVE, io.github.code_akram.or2.ffi.PermissionAnswer.DENY)) {
            val intent = AgentNotifications.answerIntent(context, alert, answer)
            assertEquals(AgentReplyReceiver::class.java.name, intent.component?.className)
            assertEquals(AgentAnswerRequest(alert, answer), AgentNotifications.answerOf(intent))
            assertNull(AgentNotifications.replyOf(intent))
        }
        assertNull(AgentNotifications.answerOf(AgentNotifications.replyIntent(context, alert)))
        // Without a prompt, Reply alone.
        assertEquals(listOf("Reply"), notifications.build(alert.copy(permission = null)).actions.orEmpty().map { it.title.toString() })
    }

    /** Built, not posted: needs no `POST_NOTIFICATIONS`. */
    @Test
    fun anAgentWithoutReplyWhoseIntegrationIsMissingOffersEnableReplyThatOpensTheApp() {
        val store = MemoryPrefStore()
        val notifications = AgentNotifications(context, store)
        val alert = AgentAlert(key, "pi", "Needs input", "Device fixture", nonce = "n1", enableReply = "pi")
        val action = notifications.build(alert).actions.orEmpty().single()
        assertEquals(AgentNotifications.ENABLE_REPLY, action.title.toString())
        // No RemoteInput: it opens the app (an immutable activity intent of the app's), which asks before installing.
        assertTrue(action.remoteInputs.isNullOrEmpty())
        assertTrue(action.actionIntent.isActivity)
        assertTrue(action.actionIntent.isImmutable)
        assertEquals(context.packageName, action.actionIntent.creatorPackage)
        // An agent with Reply keeps Reply alone; nothing to offer is no action at all.
        assertEquals("Reply", notifications.build(alert.copy(agent = CLAUDE)).actions.single().title.toString())
        assertTrue(notifications.build(alert.copy(enableReply = null)).actions.isNullOrEmpty())
        // The intent carries the pane, the capability and the app's token, and is honoured; a tap's is not one.
        val intent = AgentNotifications.enableReplyIntent(context, alert, AgentNotifications.token(store))
        assertEquals(
            EnableReplyAsk(key, "n1", EnableReplyRequest(key.hostId, "Device fixture", "pi", "pi")),
            AgentNotifications.enableReplyOf(intent, store),
        )
        assertNull(AgentNotifications.enableReplyOf(AgentNotifications.openIntent(context, key, AgentNotifications.token(store)), store))
        assertNull(AgentNotifications.paneOf(intent, store))
        // Each post's capability makes its own PendingIntent.
        val second = notifications.build(alert.copy(nonce = "n2")).actions.single().actionIntent
        assertNotEquals(action.actionIntent, second)
    }

    @Test
    fun aBlockedEdgePostsOneNotificationThatOpeningThePaneCancels() {
        assumeTrue(
            "POST_NOTIFICATIONS is not granted to ${context.packageName} (the test never grants it); skipping the posted notification.",
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED &&
                manager.areNotificationsEnabled(),
        )
        val store = MemoryPrefStore()
        val alerts = AgentAlerts(AgentNotifications(context, store))
        val watch = Any()
        alerts.viewChanged(watch, key.hostId, "Device fixture", key.session, view(AgentStatus.WORKING, 1u))
        alerts.viewChanged(watch, key.hostId, "Device fixture", key.session, view(AgentStatus.BLOCKED, 2u))
        await("the notification") { posted() != null }
        val shown = posted()!!
        assertEquals(AgentNotifications.CHANNEL_ID, shown.channelId)
        assertEquals("Claude Code", shown.extras.getString(Notification.EXTRA_TITLE))
        assertEquals("Needs input", shown.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals("Device fixture", shown.extras.getCharSequence(Notification.EXTRA_SUB_TEXT).toString())
        assertNotNull(shown.contentIntent)
        assertTrue(shown.flags and Notification.FLAG_AUTO_CANCEL != 0)
        // The tap's intent carries the pane and the app's token, and is honoured.
        val intent = AgentNotifications.openIntent(context, key, AgentNotifications.token(store), tap = "tap-1")
        assertEquals(key, AgentNotifications.paneOf(intent, store))
        assertEquals("tap-1", AgentNotifications.tapOf(intent))
        // A new process (another sink) finds it among what the system shows.
        assertTrue(key in AgentNotifications(context, store).shown())
        alerts.opened(key)
        await("the notification to go") { posted() == null }
    }
}

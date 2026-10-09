package io.github.code_akram.or2.notify

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Person
import android.app.RemoteInput
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.net.Uri
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.R
import io.github.code_akram.or2.app.PrefStore
import io.github.code_akram.or2.ffi.PermissionAnswer
import java.util.UUID

/**
 * Posts agent alerts ([AgentAlertSink]) on the `agents` channel: one notification per pane (tag [AgentPaneKey.tag],
 * id [NOTIFICATION_ID]), whose tap opens [MainActivity] with the pane ([openIntent]) and whose Reply action sends
 * text to the agent ([replyIntent], [AgentReplies]); an agent without Reply whose herdr integration is missing gets
 * **Enable Reply** instead, which opens the app to its confirmation ([enableReplyIntent]). One whose agent waits at a
 * permission prompt also has **Approve** and **Deny** ([answerIntent], [AgentAnswers]). Nothing is posted without
 * `POST_NOTIFICATIONS`.
 */
class AgentNotifications(private val context: Context, private val store: PrefStore) : AgentAlertSink {
    private val manager get() = context.getSystemService(NotificationManager::class.java)

    override fun post(alert: AgentAlert) {
        if (context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        try {
            createChannel(manager)
            manager.notify(alert.key.tag, NOTIFICATION_ID, build(alert))
        } catch (_: SecurityException) {
            // The permission was taken away between the check and the post.
        }
    }

    override fun cancel(key: AgentPaneKey) = manager.cancel(key.tag, NOTIFICATION_ID)

    /** Our agent notifications up now (the `agents` channel, id [NOTIFICATION_ID], a pane tag), from the system. */
    override fun shown(): Set<AgentPaneKey> = try {
        manager.activeNotifications
            .filter { it.id == NOTIFICATION_ID && it.notification.channelId == CHANNEL_ID }
            .mapNotNullTo(mutableSetOf()) { shown -> shown.tag?.let(AgentPaneKey::fromTag) }
    } catch (_: RuntimeException) {
        emptySet()
    }

    /**
     * The notification for [alert]: its tap opens the pane, its Reply action takes a RemoteInput. After a reply
     * ([AgentAlert.outcome]) it shows the outcome, quotes a sent reply (MessagingStyle reply history) and does not
     * alert again.
     */
    internal fun build(alert: AgentAlert): Notification {
        val key = alert.key
        val open = PendingIntent.getActivity(
            context, key.tag.hashCode(), openIntent(context, key, token(store)),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val now = System.currentTimeMillis()
        val builder = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_or2)
            .setContentTitle(alert.title)
            .setContentText(alert.outcome ?: alert.text)
            .setSubText(alert.subText)
            .setCategory(Notification.CATEGORY_STATUS)
            .setShowWhen(true)
            .setWhen(now)
            .setAutoCancel(true)
            .setContentIntent(open)
        // Only an agent instance herdr identifies can be answered from here: any other is opened to reply, and one whose
        // integration is missing offers to set it up instead (in the app, after a confirmation). One at a permission
        // prompt can be approved or denied too, with the phone unlocked.
        for (action in alertActions(alert)) {
            builder.addAction(
                when (action.kind) {
                    AlertActionKind.APPROVE -> answerAction(alert, action, PermissionAnswer.APPROVE)
                    AlertActionKind.DENY -> answerAction(alert, action, PermissionAnswer.DENY)
                    AlertActionKind.REPLY -> replyAction(alert)
                    AlertActionKind.ENABLE_REPLY -> enableReplyAction(alert)
                },
            )
        }
        // An outcome, or the prompt found behind a `Needs input` just posted: the same notification, updated quietly.
        if (alert.outcome != null || alert.permission != null) builder.setOnlyAlertOnce(true)
        alert.reply?.let { reply ->
            val agent = Person.Builder().setName(alert.title).build()
            builder.setStyle(
                Notification.MessagingStyle(Person.Builder().setName(YOU).build())
                    .addMessage(alert.text, now, agent)
                    // A null sender is the user: the reply, quoted.
                    .addMessage(reply, now, null as Person?),
            )
        }
        return builder.build()
    }

    /**
     * The Reply action: a RemoteInput (`Reply to <agent>`) whose text reaches [AgentReplyReceiver]. RemoteInput needs a
     * mutable PendingIntent; it is explicit (the receiver's component, not exported) and names the pane and this post's
     * Reply capability in its data, which a fill-in cannot change, so nothing but the RemoteInput's text is taken from
     * the fill-in. The capability (a nonce, [ReplyNonces]) makes each post's PendingIntent distinct and is taken once
     * by the app; `FLAG_ONE_SHOT` lets the system send it once too.
     */
    private fun replyAction(alert: AgentAlert): Notification.Action {
        val reply = PendingIntent.getBroadcast(
            context, alert.key.tag.hashCode(), replyIntent(context, alert),
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_ONE_SHOT,
        )
        val input = RemoteInput.Builder(KEY_REPLY).setLabel("Reply to ${alert.title}").build()
        return Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_stat_or2), "Reply", reply)
            .addRemoteInput(input)
            .setSemanticAction(Notification.Action.SEMANTIC_ACTION_REPLY)
            .setAllowGeneratedReplies(false)
            .build()
    }

    /**
     * An **Approve** or **Deny** action ([action], from [alertActions]): a broadcast to [AgentReplyReceiver] (explicit,
     * not exported) that answers the prompt without opening the app. Immutable and one-shot; its data names the pane
     * and this post's capability (shared with the Reply action: one use by any of them), its extras the agent instance
     * and the prompt's seq. The system asks to unlock the phone first ([AlertAction.authenticationRequired]).
     */
    private fun answerAction(alert: AgentAlert, action: AlertAction, answer: PermissionAnswer): Notification.Action {
        val intent = PendingIntent.getBroadcast(
            context, alert.key.tag.hashCode(), answerIntent(context, alert, answer),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_ONE_SHOT,
        )
        return Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_stat_or2), action.label, intent)
            .setAuthenticationRequired(action.authenticationRequired)
            .setAllowGeneratedReplies(false)
            .build()
    }

    /**
     * The **Enable Reply** action: opens [MainActivity] to its confirmation ([enableReplyIntent]); nothing is installed
     * from the notification. Immutable and one-shot, its data carries this post's capability (the pane's nonce, taken
     * once by `AgentAlerts.admitEnableReply`) and the app's token is in its extras, as a tap's is.
     */
    private fun enableReplyAction(alert: AgentAlert): Notification.Action {
        val enable = PendingIntent.getActivity(
            context, alert.key.tag.hashCode(), enableReplyIntent(context, alert, token(store)),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_ONE_SHOT,
        )
        return Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_stat_or2), ENABLE_REPLY, enable).build()
    }

    companion object {
        const val CHANNEL_ID = "agents"

        /** The label of the notification's action, and of the Inbox row's. */
        const val ENABLE_REPLY = "Enable Reply"
        const val ACTION_ENABLE_REPLY = "io.github.code_akram.or2.action.ENABLE_REPLY"
        private const val ENABLE_SCHEME = "or2-agent-enable"
        private const val EXTRA_INTEGRATION = "io.github.code_akram.or2.extra.INTEGRATION"

        /** The id of every agent notification; the pane is in the tag. The service's notification is 1, untagged. */
        const val NOTIFICATION_ID = 2
        const val ACTION_OPEN_AGENT = "io.github.code_akram.or2.action.OPEN_AGENT"
        private const val EXTRA_HOST_ID = "io.github.code_akram.or2.extra.HOST_ID"
        private const val EXTRA_SESSION = "io.github.code_akram.or2.extra.SESSION"
        private const val EXTRA_PANE_ID = "io.github.code_akram.or2.extra.PANE_ID"
        private const val EXTRA_TOKEN = "io.github.code_akram.or2.extra.TOKEN"
        private const val EXTRA_TAP = "io.github.code_akram.or2.extra.TAP"
        private const val TOKEN_KEY = "agent_open_token"
        const val ACTION_REPLY = "io.github.code_akram.or2.action.REPLY_TO_AGENT"

        /** The RemoteInput's result key: the reply's text. */
        const val KEY_REPLY = "io.github.code_akram.or2.extra.REPLY"
        private const val REPLY_SCHEME = "or2-agent-reply"
        private const val EXTRA_TITLE = "io.github.code_akram.or2.extra.TITLE"
        private const val EXTRA_TEXT = "io.github.code_akram.or2.extra.TEXT"
        private const val EXTRA_HOST = "io.github.code_akram.or2.extra.HOST"
        private const val EXTRA_TERMINAL = "io.github.code_akram.or2.extra.TERMINAL"
        private const val EXTRA_AGENT = "io.github.code_akram.or2.extra.AGENT"
        private const val EXTRA_AGENT_NAME = "io.github.code_akram.or2.extra.AGENT_NAME"
        private const val EXTRA_SESSION_KIND = "io.github.code_akram.or2.extra.AGENT_SESSION_KIND"
        private const val EXTRA_SESSION_VALUE = "io.github.code_akram.or2.extra.AGENT_SESSION_VALUE"
        private const val YOU = "You"

        /**
         * The Reply action's intent: explicitly [AgentReplyReceiver]; its data is the pane's tag with this post's Reply
         * capability as the fragment (each post's pending intent distinct); the agent instance it is for (terminal,
         * kind, name and session, always set, so a fill-in's extras never replace them); and what the notification
         * showed (title, `Needs input`/`Done`, host), for the update.
         */
        fun replyIntent(context: Context, alert: AgentAlert): Intent = Intent(ACTION_REPLY)
            .setComponent(ComponentName(context, AgentReplyReceiver::class.java))
            .setData(Uri.fromParts(REPLY_SCHEME, alert.key.tag, alert.nonce))
            .putExtra(EXTRA_TERMINAL, alert.agent?.terminalId)
            .putExtra(EXTRA_AGENT, alert.agent?.agent)
            .putExtra(EXTRA_AGENT_NAME, alert.agent?.name)
            .putExtra(EXTRA_SESSION_KIND, alert.agent?.session?.kind)
            .putExtra(EXTRA_SESSION_VALUE, alert.agent?.session?.value)
            .putExtra(EXTRA_TITLE, alert.title)
            .putExtra(EXTRA_TEXT, alert.text)
            .putExtra(EXTRA_HOST, alert.subText)

        /** The alert a Reply intent is about ([replyIntent]), with its Reply capability, or null for any other intent. */
        fun replyOf(intent: Intent?): AgentAlert? {
            if (intent?.data?.scheme != REPLY_SCHEME) return null
            return agentReplyFrom(
                intent.action, intent.data?.schemeSpecificPart, intent.data?.fragment,
                intent.getStringExtra(EXTRA_TERMINAL), intent.getStringExtra(EXTRA_AGENT),
                intent.getStringExtra(EXTRA_AGENT_NAME), intent.getStringExtra(EXTRA_SESSION_KIND),
                intent.getStringExtra(EXTRA_SESSION_VALUE), intent.getStringExtra(EXTRA_TITLE), intent.getStringExtra(EXTRA_TEXT), intent.getStringExtra(EXTRA_HOST),
            )
        }

        const val ACTION_APPROVE = "io.github.code_akram.or2.action.APPROVE_AGENT"
        const val ACTION_DENY = "io.github.code_akram.or2.action.DENY_AGENT"
        private const val ANSWER_SCHEME = "or2-agent-answer"
        private const val EXTRA_SEQ = "io.github.code_akram.or2.extra.PERMISSION_SEQ"

        /**
         * An **Approve** or **Deny** action's intent: explicitly [AgentReplyReceiver], its action the answer; its data
         * the pane's tag with this post's capability as the fragment; the agent instance (as a Reply intent's), the
         * prompt's seq, and what the notification showed, for the update.
         */
        fun answerIntent(context: Context, alert: AgentAlert, answer: PermissionAnswer): Intent =
            Intent(if (answer == PermissionAnswer.APPROVE) ACTION_APPROVE else ACTION_DENY)
                .setComponent(ComponentName(context, AgentReplyReceiver::class.java))
                .setData(Uri.fromParts(ANSWER_SCHEME, alert.key.tag, alert.nonce))
                .putExtra(EXTRA_TERMINAL, alert.agent?.terminalId)
                .putExtra(EXTRA_AGENT, alert.agent?.agent)
                .putExtra(EXTRA_AGENT_NAME, alert.agent?.name)
                .putExtra(EXTRA_SESSION_KIND, alert.agent?.session?.kind)
                .putExtra(EXTRA_SESSION_VALUE, alert.agent?.session?.value)
                .putExtra(EXTRA_SEQ, alert.permission?.toLong() ?: -1L)
                .putExtra(EXTRA_TITLE, alert.title)
                .putExtra(EXTRA_TEXT, alert.text)
                .putExtra(EXTRA_HOST, alert.subText)

        /** The answer an Approve or Deny intent names ([answerIntent]), with its capability, or null for any other intent. */
        fun answerOf(intent: Intent?): AgentAnswerRequest? {
            if (intent?.data?.scheme != ANSWER_SCHEME) return null
            return agentAnswerFrom(
                intent.action, intent.data?.schemeSpecificPart, intent.data?.fragment,
                intent.getStringExtra(EXTRA_TERMINAL), intent.getStringExtra(EXTRA_AGENT),
                intent.getStringExtra(EXTRA_AGENT_NAME), intent.getStringExtra(EXTRA_SESSION_KIND),
                intent.getStringExtra(EXTRA_SESSION_VALUE), intent.getLongExtra(EXTRA_SEQ, -1L),
                intent.getStringExtra(EXTRA_TITLE), intent.getStringExtra(EXTRA_TEXT), intent.getStringExtra(EXTRA_HOST),
            )
        }

        /** The reply's text in a Reply intent's RemoteInput results, or null. */
        fun replyTextOf(intent: Intent): String? =
            RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY_REPLY)?.toString()

        /** The `agents` channel ("Agents", high importance); creating it again is harmless. */
        fun createChannel(manager: NotificationManager) {
            val channel = NotificationChannel(CHANNEL_ID, "Agents", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "An agent needs input or has finished."
            }
            manager.createNotificationChannel(channel)
        }

        /**
         * The tap's intent: [MainActivity] (brought to the front when it runs) with the pane. Its data URI is the
         * pane's tag, so each pane's pending intent is distinct whatever its request code. [tap] names this one tap (each
         * post makes a new one, replacing the pane's pending intent's extras), so a recreation that hands the activity
         * the same intent again is told from a new tap ([AgentTaps]).
         */
        fun openIntent(
            context: Context, key: AgentPaneKey, token: String, tap: String = UUID.randomUUID().toString(),
        ): Intent = Intent(context, MainActivity::class.java)
            .setAction(ACTION_OPEN_AGENT)
            .setData(Uri.fromParts("or2-agent", key.tag, null))
            .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            .putExtra(EXTRA_HOST_ID, key.hostId)
            .putExtra(EXTRA_SESSION, key.session)
            .putExtra(EXTRA_PANE_ID, key.paneId)
            .putExtra(EXTRA_TOKEN, token)
            .putExtra(EXTRA_TAP, tap)

        /**
         * The pane a notification's tap names, or null for any other intent. The activity is exported (the launcher
         * starts it), so a tap carries the app's own random token and an intent from elsewhere is ignored.
         */
        fun paneOf(intent: Intent?, store: PrefStore): AgentPaneKey? {
            if (intent?.action != ACTION_OPEN_AGENT) return null
            return agentOpenFrom(
                intent.getLongExtra(EXTRA_HOST_ID, 0), intent.getStringExtra(EXTRA_SESSION), intent.getStringExtra(EXTRA_PANE_ID),
                intent.getStringExtra(EXTRA_TOKEN), store.getString(TOKEN_KEY),
            )
        }

        /**
         * The **Enable Reply** action's intent: [MainActivity] (brought to the front when it runs); its data is the pane's
         * tag with this post's capability as the fragment (each post's pending intent distinct); the app's [token]; the
         * integration to install; and what the confirmation names (the agent's label, the host's).
         */
        fun enableReplyIntent(context: Context, alert: AgentAlert, token: String): Intent =
            Intent(context, MainActivity::class.java)
                .setAction(ACTION_ENABLE_REPLY)
                .setData(Uri.fromParts(ENABLE_SCHEME, alert.key.tag, alert.nonce))
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
                .putExtra(EXTRA_TOKEN, token)
                .putExtra(EXTRA_INTEGRATION, alert.enableReply)
                .putExtra(EXTRA_TITLE, alert.title)
                .putExtra(EXTRA_HOST, alert.subText)

        /**
         * The Enable Reply an intent asks for ([enableReplyIntent]), with its pane and capability, or null for any other
         * intent. The activity is exported, so it must carry the app's own token, as a tap does.
         */
        fun enableReplyOf(intent: Intent?, store: PrefStore): EnableReplyAsk? {
            if (intent?.action != ACTION_ENABLE_REPLY || intent.data?.scheme != ENABLE_SCHEME) return null
            return enableReplyFrom(
                intent.data?.schemeSpecificPart, intent.data?.fragment, intent.getStringExtra(EXTRA_INTEGRATION),
                intent.getStringExtra(EXTRA_TITLE), intent.getStringExtra(EXTRA_HOST),
                intent.getStringExtra(EXTRA_TOKEN), store.getString(TOKEN_KEY),
            )
        }

        /** The id of the tap [intent] carries ([openIntent]), or null. */
        fun tapOf(intent: Intent?): String? = intent?.getStringExtra(EXTRA_TAP)

        /** The app's token for its notification taps, made once. */
        fun token(store: PrefStore): String = store.getString(TOKEN_KEY) ?: UUID.randomUUID().toString().also { store.putString(TOKEN_KEY, it) }
    }
}

/** A notification's **Enable Reply**: the pane it came from, its capability, and what to ask. */
data class EnableReplyAsk(val key: AgentPaneKey, val nonce: String?, val request: EnableReplyRequest)

/**
 * An Enable Reply intent's ask from its parts ([AgentNotifications.enableReplyIntent]): the token must be the app's own,
 * the tag a pane's, and the integration one of [HerdrIntegrations.IDS]; else null.
 */
fun enableReplyFrom(
    tag: String?, nonce: String?, integration: String?, title: String?, host: String?, token: String?, expected: String?,
): EnableReplyAsk? {
    if (expected == null || token != expected || tag == null) return null
    val key = AgentPaneKey.fromTag(tag)?.takeIf { it.hostId > 0 && it.paneId.isNotEmpty() } ?: return null
    if (integration == null || integration !in HerdrIntegrations.IDS) return null
    return EnableReplyAsk(key, nonce, EnableReplyRequest(key.hostId, host.orEmpty(), title.orEmpty().ifEmpty { "agent" }, integration))
}

/** A notification tap's pane from its extras: the token must be the app's own, and host and pane must be named. */
fun agentOpenFrom(hostId: Long, session: String?, paneId: String?, token: String?, expected: String?): AgentPaneKey? {
    if (expected == null || token != expected || hostId <= 0 || paneId.isNullOrEmpty()) return null
    return AgentPaneKey(hostId, session, paneId)
}

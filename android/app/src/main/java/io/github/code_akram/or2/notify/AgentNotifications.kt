package io.github.code_akram.or2.notify

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.R
import io.github.code_akram.or2.app.PrefStore
import java.util.UUID

/**
 * Posts agent alerts ([AgentAlertSink]) on the `agents` channel: one notification per pane (tag [AgentPaneKey.tag],
 * id [NOTIFICATION_ID]), whose tap opens [MainActivity] with the pane ([openIntent]). Nothing is posted without
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

    private fun build(alert: AgentAlert): Notification {
        val key = alert.key
        val open = PendingIntent.getActivity(
            context, key.tag.hashCode(), openIntent(context, key, token(store)),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_or2)
            .setContentTitle(alert.title)
            .setContentText(alert.text)
            .setSubText(alert.subText)
            .setCategory(Notification.CATEGORY_STATUS)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setAutoCancel(true)
            .setContentIntent(open)
            .build()
    }

    companion object {
        const val CHANNEL_ID = "agents"

        /** The id of every agent notification; the pane is in the tag. The service's notification is 1, untagged. */
        const val NOTIFICATION_ID = 2
        const val ACTION_OPEN_AGENT = "io.github.code_akram.or2.action.OPEN_AGENT"
        private const val EXTRA_HOST_ID = "io.github.code_akram.or2.extra.HOST_ID"
        private const val EXTRA_SESSION = "io.github.code_akram.or2.extra.SESSION"
        private const val EXTRA_PANE_ID = "io.github.code_akram.or2.extra.PANE_ID"
        private const val EXTRA_TOKEN = "io.github.code_akram.or2.extra.TOKEN"
        private const val EXTRA_TAP = "io.github.code_akram.or2.extra.TAP"
        private const val TOKEN_KEY = "agent_open_token"

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

        /** The id of the tap [intent] carries ([openIntent]), or null. */
        fun tapOf(intent: Intent?): String? = intent?.getStringExtra(EXTRA_TAP)

        /** The app's token for its notification taps, made once. */
        fun token(store: PrefStore): String = store.getString(TOKEN_KEY) ?: UUID.randomUUID().toString().also { store.putString(TOKEN_KEY, it) }
    }
}

/** A notification tap's pane from its extras: the token must be the app's own, and host and pane must be named. */
fun agentOpenFrom(hostId: Long, session: String?, paneId: String?, token: String?, expected: String?): AgentPaneKey? {
    if (expected == null || token != expected || hostId <= 0 || paneId.isNullOrEmpty()) return null
    return AgentPaneKey(hostId, session, paneId)
}

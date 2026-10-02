package io.github.code_akram.or2.notify

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import io.github.code_akram.or2.app.Or2Application

/**
 * An agent notification's Reply ([AgentNotifications.replyIntent]): explicit and not exported, so only the
 * notification's own PendingIntent reaches it. The pane comes from the intent's data, the text from the RemoteInput;
 * [AgentReplies] sends it over the host's live connection (never connecting from here) and updates the notification.
 * The text is never logged.
 */
class AgentReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val alert = AgentNotifications.replyOf(intent) ?: return
        val app = context.applicationContext as? Or2Application ?: return
        val text = AgentNotifications.replyTextOf(intent)
        val pending = goAsync()
        app.agentReplies.launch(alert, text) { pending.finish() }
    }
}

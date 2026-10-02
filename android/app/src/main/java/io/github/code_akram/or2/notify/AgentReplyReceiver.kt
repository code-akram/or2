package io.github.code_akram.or2.notify

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import io.github.code_akram.or2.app.Or2Application

/**
 * An agent notification's Reply ([AgentNotifications.replyIntent]): explicit and not exported, so only the
 * notification's own PendingIntent reaches it. The pane and the Reply capability come from the intent's data, the text
 * from the RemoteInput; [AgentReplies] takes the capability, sends the reply over the host's live connection (never
 * connecting from here) and updates the notification. The text is never logged.
 *
 * The broadcast finishes at once (no `goAsync`): the reply runs on the application's scope, so a host that answers
 * slowly (Rust's bound is 30 s, longer than a broadcast may be held) can never make the receiver time out. The process
 * stays alive for it, because a reply goes only over a live connection, and `ConnectionService` runs in the foreground
 * while any is open; with none, the reply fails as `not connected` before this returns.
 */
class AgentReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val alert = AgentNotifications.replyOf(intent) ?: return
        val app = context.applicationContext as? Or2Application ?: return
        app.agentReplies.launch(alert, AgentNotifications.replyTextOf(intent))
    }
}

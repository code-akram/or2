package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.ReplyRoute
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/*
 * Replying to an agent from its notification (v0.1.2, contracts.md "Reply from a notification"): the notification's
 * Reply action ([AgentNotifications]) hands the typed text to [AgentReplyReceiver], which hands it here. Plain JVM
 * logic; the receiver and the notification are the Android ends.
 */

/**
 * Sends a notification's reply over the host's live connection and updates the notification with what came of it.
 *
 * - The host must be connected now ([send] throws [HostException.NotConnected] otherwise): a reply never connects from
 *   the background. That says `Not sent: <host> is not connected`.
 * - Sent: the notification shows `Sent` with the reply quoted (MessagingStyle reply history) and does not alert.
 * - Not sent: it shows the reason, and keeps its Reply action (every agent notification has one) for another try.
 *
 * The text is never logged and not kept: it lives only in this call and, once sent, in the notification's quote.
 */
class AgentReplies(
    private val scope: CoroutineScope,
    /** `HostConnections.replyToPane` for the pane: throws [HostException.NotConnected] without a live connection. */
    private val send: suspend (key: AgentPaneKey, text: String) -> ReplyRoute,
    /** `AgentAlerts.replied`: replaces the pane's notification while it is up. */
    private val post: (AgentAlert) -> Unit,
    /** A safety bound over Rust's own query timeout (30 s), inside a receiver's 60 s. */
    private val timeoutMs: Long = REPLY_TIMEOUT_MS,
) {
    /** [reply] on [scope]; [done] runs once it has finished, whatever happened (the receiver's `goAsync` finish). */
    fun launch(alert: AgentAlert, text: String?, done: () -> Unit) {
        scope.launch {
            try {
                reply(alert, text)
            } finally {
                done()
            }
        }
    }

    /** Sends [text] to [alert]'s pane and posts the outcome as [alert]'s update. */
    suspend fun reply(alert: AgentAlert, text: String?) {
        val failure = when {
            text.isNullOrBlank() -> NOT_SENT_EMPTY
            else -> try {
                if (withTimeoutOrNull(timeoutMs) { send(alert.key, text) } == null) NOT_SENT_TIMEOUT else null
            } catch (e: HostException) {
                notSent(e, alert.subText)
            }
        }
        post(if (failure == null) alert.copy(outcome = SENT, reply = text) else alert.copy(outcome = failure, reply = null))
    }

    companion object {
        const val SENT = "Sent"
        const val NOT_SENT_EMPTY = "Not sent: the reply is empty"
        const val NOT_SENT_TIMEOUT = "Not sent: the host did not answer"
        const val REPLY_TIMEOUT_MS = 45_000L

        /** The longest reason shown after `Not sent: `. */
        private const val MAX_REASON = 80

        /** What a failed reply's notification says; [host] is the host's label. */
        fun notSent(error: HostException, host: String): String = "Not sent: " + when (error) {
            is HostException.NotConnected, is HostException.Closed -> "$host is not connected"
            is HostException.PaneNotFound -> "the agent is gone"
            is HostException.TooLarge -> "the reply is too long"
            is HostException.NotInstalled -> "${error.program} is not installed on $host"
            is HostException.CommandFailed -> error.reason.take(MAX_REASON)
            else -> "herdr refused it"
        }
    }
}

/**
 * The pane and the alert a reply intent names ([AgentNotifications.replyIntent]), or null for any other intent. The
 * pane comes from the intent's data ([tag]), which a RemoteInput's fill-in cannot change; [title], [text] and [host]
 * are only shown.
 */
fun agentReplyFrom(action: String?, tag: String?, title: String?, text: String?, host: String?): AgentAlert? {
    if (action != AgentNotifications.ACTION_REPLY || tag == null) return null
    val key = AgentPaneKey.fromTag(tag) ?: return null
    if (key.hostId <= 0 || key.paneId.isEmpty()) return null
    return AgentAlert(key, title.orEmpty().ifEmpty { "Agent" }, text.orEmpty(), host.orEmpty())
}

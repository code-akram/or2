package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.AgentIdentity
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
 * - The reply must carry its notification's current Reply capability ([admit], [AgentAlerts.admitReply]), which it
 *   uses up before anything else: a replayed or stale PendingIntent does nothing at all.
 * - The host must be connected now ([send] throws [HostException.NotConnected] otherwise): a reply never connects from
 *   the background. That says `Not sent: <host> is not connected`.
 * - Sent: the notification shows `Sent` with the reply quoted (MessagingStyle reply history) and does not alert.
 * - Not sent: it shows the reason, and keeps its Reply action (every agent notification has one, with a new
 *   capability) for another try.
 *
 * The text is never logged and not kept: it lives only in this call and, once sent, in the notification's quote.
 */
class AgentReplies(
    /** The application's scope: a reply outlives the broadcast that brought it (see [AgentReplyReceiver]). */
    private val scope: CoroutineScope,
    /** `AgentAlerts.admitReply`: whether the reply's capability is the pane's current one, using it up. */
    private val admit: (key: AgentPaneKey, nonce: String?) -> Boolean,
    /** `HostConnections.replyToPane` for the agent: throws [HostException.NotConnected] without a live connection. */
    private val send: suspend (key: AgentPaneKey, agent: AgentIdentity, text: String) -> ReplyRoute,
    /** `AgentAlerts.replied`: replaces the pane's notification while it is up. */
    private val post: (AgentAlert) -> Unit,
    /** A safety bound over Rust's own query timeout (30 s). */
    private val timeoutMs: Long = REPLY_TIMEOUT_MS,
) {
    /**
     * [reply] on [scope], started at once: on a main-thread scope it takes the capability before this returns. The
     * caller (a broadcast) does not wait for it.
     */
    fun launch(alert: AgentAlert, text: String?) {
        scope.launch { reply(alert, text) }
    }

    /** Sends [text] to [alert]'s agent and posts the outcome as [alert]'s update; nothing for a stale capability. */
    suspend fun reply(alert: AgentAlert, text: String?) {
        if (!admit(alert.key, alert.nonce)) return
        val agent = alert.agent
        val failure = when {
            text.isNullOrBlank() -> NOT_SENT_EMPTY
            agent == null -> NOT_SENT_GONE
            else -> try {
                if (withTimeoutOrNull(timeoutMs) { send(alert.key, agent, text) } == null) NOT_SENT_TIMEOUT else null
            } catch (e: HostException) {
                notSent(e, alert.subText)
            }
        }
        val update = alert.copy(nonce = null)
        post(if (failure == null) update.copy(outcome = SENT, reply = text) else update.copy(outcome = failure, reply = null))
    }

    companion object {
        const val SENT = "Sent"
        const val NOT_SENT_EMPTY = "Not sent: the reply is empty"
        const val NOT_SENT_TIMEOUT = "Not sent: the host did not answer"
        const val NOT_SENT_GONE = "Not sent: the agent is gone"
        const val REPLY_TIMEOUT_MS = 45_000L

        /** The longest reason shown after `Not sent: `. */
        private const val MAX_REASON = 80

        /** What a failed reply's notification says; [host] is the host's label. */
        fun notSent(error: HostException, host: String): String = when (error) {
            is HostException.NotConnected, is HostException.Closed -> "Not sent: $host is not connected"
            is HostException.PaneNotFound -> NOT_SENT_GONE
            is HostException.TooLarge -> "Not sent: the reply is too long"
            is HostException.NotInstalled -> "Not sent: ${error.program} is not installed on $host"
            is HostException.CommandFailed -> "Not sent: " + error.reason.take(MAX_REASON)
            else -> "Not sent: herdr refused it"
        }
    }
}

/**
 * The pane, the agent and the alert a reply intent names ([AgentNotifications.replyIntent]), with its Reply capability
 * [nonce], or null for any other intent. The pane and the capability come from the intent's data ([tag], [nonce]),
 * which a RemoteInput's fill-in cannot change; the agent ([terminal], [kind]) from extras the intent always sets;
 * [title], [text] and [host] are only shown. An intent without a terminal names no agent: its reply is not sent.
 */
fun agentReplyFrom(
    action: String?, tag: String?, nonce: String?, terminal: String?, kind: String?,
    title: String?, text: String?, host: String?,
): AgentAlert? {
    if (action != AgentNotifications.ACTION_REPLY || tag == null) return null
    val key = AgentPaneKey.fromTag(tag) ?: return null
    if (key.hostId <= 0 || key.paneId.isEmpty()) return null
    val agent = terminal?.takeIf { it.isNotEmpty() }?.let { AgentIdentity(it, kind) }
    return AgentAlert(key, title.orEmpty().ifEmpty { "Agent" }, text.orEmpty(), host.orEmpty(), agent = agent, nonce = nonce)
}

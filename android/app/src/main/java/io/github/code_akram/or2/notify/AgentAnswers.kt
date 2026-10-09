package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.PermissionAnswer
import io.github.code_akram.or2.ffi.PermissionPrompt
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/*
 * Approving or denying a permission prompt from its notification (contracts.md, "Answer: approve or deny a permission
 * prompt"): a `Needs input` notification of Claude Code is asked about (`permission_prompt`), and one at a yes/no
 * permission prompt gains **Approve** and **Deny** ([AgentNotifications]), which [AgentReplyReceiver] hands here. Plain
 * JVM logic; the receiver and the notification are the Android ends.
 */

/**
 * Finds the permission prompt behind a `Needs input` notification, and answers it from the notification's actions.
 *
 * - [check]: asked once per `Needs input` post, over the host's live connection only (nothing connects from here); a
 *   prompt found makes the notification say `Needs permission` with **Approve** and **Deny** ([found],
 *   `AgentAlerts.permissionFound`). Any failure (not connected, not a prompt, a slow host) leaves it as it was.
 * - [answer]: the action must carry its notification's current capability ([admit], the Reply capability: one use, by
 *   any of the notification's actions), which it uses up first. The answer names the prompt by its seq; Rust checks
 *   everything again and sends one key, or nothing. The notification then shows the outcome (`Approved`, `Denied`, or
 *   why not) without alerting, keeps its Reply action and drops **Approve** and **Deny**.
 */
class AgentAnswers(
    /** The application's scope: an answer outlives the broadcast that brought it (see [AgentReplyReceiver]). */
    private val scope: CoroutineScope,
    /** `AgentAlerts.admitReply`: whether the action's capability is the pane's current one, using it up. */
    private val admit: (key: AgentPaneKey, nonce: String?) -> Boolean,
    /** `HostConnections.permissionPrompt` for the agent: throws [HostException.NotConnected] without a live connection. */
    private val ask: suspend (key: AgentPaneKey, agent: AgentIdentity) -> PermissionPrompt?,
    /** `HostConnections.answerPermission`: throws [HostException.NotConnected] without a live connection. */
    private val send: suspend (key: AgentPaneKey, agent: AgentIdentity, seq: ULong, answer: PermissionAnswer) -> Unit,
    /** `AgentAlerts.permissionFound`: the alert's notification gains Approve and Deny for the prompt at that seq. */
    private val found: (alert: AgentAlert, seq: ULong) -> Unit,
    /** `AgentAlerts.replied`: replaces the pane's notification with the outcome while it is up. */
    private val post: (AgentAlert) -> Unit,
    /** A safety bound over Rust's own query timeout (30 s). */
    private val timeoutMs: Long = AgentReplies.REPLY_TIMEOUT_MS,
) {
    /** [ask] on [scope], for an alert just posted; the caller does not wait for it. */
    fun check(alert: AgentAlert) {
        scope.launch { ask(alert) }
    }

    /** Whether [alert]'s agent waits at a permission prompt; if so its notification gains Approve and Deny. */
    suspend fun ask(alert: AgentAlert) {
        val agent = alert.agent ?: return
        val prompt = try {
            withTimeoutOrNull(timeoutMs) { ask(alert.key, agent) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // Not connected, gone, or herdr could not tell: the notification stays as it was (Reply, the tap).
            null
        }
        prompt?.let { found(alert, it.stateChangeSeq) }
    }

    /** [answer] on [scope], started at once (it takes the capability before this returns on a main-thread scope). */
    fun launch(alert: AgentAlert, choice: PermissionAnswer) {
        scope.launch { answer(alert, choice) }
    }

    /** Answers [alert]'s prompt with [choice] and posts the outcome as [alert]'s update; nothing for a stale capability. */
    suspend fun answer(alert: AgentAlert, choice: PermissionAnswer) {
        if (!admit(alert.key, alert.nonce)) return
        val agent = alert.agent
        val seq = alert.permission
        val outcome = if (agent == null || seq == null) {
            NOT_ANSWERED_OPEN_PANE
        } else {
            try {
                if (withTimeoutOrNull(timeoutMs) { send(alert.key, agent, seq, choice) } == null) NOT_ANSWERED_TIMEOUT else answered(choice)
            } catch (e: HostException) {
                notAnswered(e, alert.subText)
            }
        }
        post(alert.copy(nonce = null, permission = null, reply = null, outcome = outcome))
    }

    companion object {
        const val APPROVED = "Approved"
        const val DENIED = "Denied"
        const val PROMPT_CHANGED = "The prompt changed. Open the pane."
        const val NOT_ANSWERED_GONE = "Not answered: the agent is gone"
        const val NOT_ANSWERED_TIMEOUT = "Not answered: the host did not answer"
        const val NOT_ANSWERED_OPEN_PANE = "Not answered: open the pane to answer"

        /** The longest reason shown after `Not answered: `. */
        private const val MAX_REASON = 80

        /** What an answered prompt's notification says. */
        fun answered(choice: PermissionAnswer): String = when (choice) {
            PermissionAnswer.APPROVE -> APPROVED
            PermissionAnswer.DENY -> DENIED
        }

        /** What a prompt's notification says when its answer was not sent; [host] is the host's label. */
        fun notAnswered(error: HostException, host: String): String = when (error) {
            is HostException.PromptChanged -> PROMPT_CHANGED
            is HostException.NotConnected, is HostException.Closed -> "Not answered: $host is not connected"
            is HostException.PaneNotFound -> NOT_ANSWERED_GONE
            is HostException.NotInstalled -> "Not answered: ${error.program} is not installed on $host"
            is HostException.CommandFailed -> "Not answered: " + error.reason.take(MAX_REASON)
            else -> "Not answered: herdr refused it"
        }
    }
}

/** What one action of an agent notification is ([alertActions]). */
enum class AlertActionKind { APPROVE, DENY, REPLY, ENABLE_REPLY }

/**
 * One action of an agent notification, as plain data: [AgentNotifications.build] makes it an Android action. Approve
 * and Deny need the phone unlocked ([authenticationRequired]); every action but Enable Reply carries the post's
 * capability ([nonce]); Approve and Deny also name the prompt ([seq]).
 */
data class AlertAction(
    val kind: AlertActionKind,
    val label: String,
    val authenticationRequired: Boolean,
    val nonce: String?,
    val seq: ULong?,
) {
    /** Never the capability. */
    override fun toString() =
        "AlertAction(kind=$kind, label=$label, authenticationRequired=$authenticationRequired, " +
            "nonce=${if (nonce == null) "null" else "…"}, seq=$seq)"
}

/**
 * The actions of [alert]'s notification, in order: **Approve** and **Deny** while its agent waits at a permission
 * prompt ([AgentAlert.permission]); **Reply** for an agent instance herdr identifies, else **Enable Reply** when its
 * integration is missing.
 */
fun alertActions(alert: AgentAlert): List<AlertAction> = buildList {
    if (alert.agent != null && alert.permission != null) {
        add(AlertAction(AlertActionKind.APPROVE, APPROVE, true, alert.nonce, alert.permission))
        add(AlertAction(AlertActionKind.DENY, DENY, true, alert.nonce, alert.permission))
    }
    if (alert.agent != null) {
        add(AlertAction(AlertActionKind.REPLY, REPLY, false, alert.nonce, null))
    } else if (alert.enableReplyRequest != null) {
        add(AlertAction(AlertActionKind.ENABLE_REPLY, AgentNotifications.ENABLE_REPLY, false, alert.nonce, null))
    }
}

private const val APPROVE = "Approve"
private const val DENY = "Deny"
private const val REPLY = "Reply"

/** An Approve or Deny from a notification: the alert it came from (its prompt's seq and capability) and the answer. */
data class AgentAnswerRequest(val alert: AgentAlert, val answer: PermissionAnswer)

/**
 * The answer an Approve or Deny intent names ([AgentNotifications.answerIntent]), or null for any other intent: the
 * action names the answer, and the rest is read as a Reply intent's ([agentReplyFrom]) plus the prompt's [seq]. One
 * without a seq names no prompt: null.
 */
fun agentAnswerFrom(
    action: String?, tag: String?, nonce: String?, terminal: String?, kind: String?,
    name: String?, sessionKind: String?, sessionValue: String?, seq: Long?,
    title: String?, text: String?, host: String?,
): AgentAnswerRequest? {
    val answer = when (action) {
        AgentNotifications.ACTION_APPROVE -> PermissionAnswer.APPROVE
        AgentNotifications.ACTION_DENY -> PermissionAnswer.DENY
        else -> return null
    }
    if (seq == null || seq < 0) return null
    val alert = agentReplyFrom(
        AgentNotifications.ACTION_REPLY, tag, nonce, terminal, kind, name, sessionKind, sessionValue, title, text, host,
    ) ?: return null
    return AgentAnswerRequest(alert.copy(permission = seq.toULong()), answer)
}

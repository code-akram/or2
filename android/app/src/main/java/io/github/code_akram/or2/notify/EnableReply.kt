package io.github.code_akram.or2.notify

import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrIntegrationState
import io.github.code_akram.or2.ffi.HostException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withTimeoutOrNull

/*
 * Enable Reply from the phone (v0.1.3, contracts.md "Lane App: Enable Reply from the phone"): an agent herdr reports
 * without a session (no `reply_identity`) gets no Reply, because herdr's integration for its kind is missing on the
 * host. Its Inbox row and its notification offer **Enable Reply**: one confirmation, then
 * `HostConnection.install_herdr_integration(id)`, then the outcome. Plain JVM logic; the Inbox row, the notification
 * and the dialog are the Android ends.
 */

/** herdr 0.9.3's agent integrations, and the agent kinds herdr reports that map to them. */
object HerdrIntegrations {
    /** The ids `install_herdr_integration` takes (Rust holds the same allowlist and refuses any other). */
    val IDS: List<String> = listOf(
        "pi", "omp", "claude", "codex", "copilot", "devin", "droid", "kimi", "opencode", "kilo", "hermes", "qodercli",
        "qwen", "cursor", "mastracode", "antigravity-cli", "grok", "letta",
    )

    /** Kinds named after the agent's executable where it differs from the integration's id. */
    private val EXECUTABLES = mapOf("cursor-agent" to "cursor", "agy" to "antigravity-cli", "antigravity_cli" to "antigravity-cli")

    /** The integration for an agent of [kind] (`HerdrAgent.agent`), or null: no kind, or one without (`amp`, `gemini`). */
    fun forKind(kind: String?): String? {
        val name = kind?.trim()?.lowercase()?.takeIf { it.isNotEmpty() } ?: return null
        return EXECUTABLES[name] ?: name.takeIf { it in IDS }
    }
}

/**
 * The integration to offer for [agent] (its id), or null for nothing new. Offered only for an agent herdr reports
 * without a session (no `reply_identity`) whose kind has an integration that [integrations] (the host's
 * `herdr integration status`, null until read) lists as not installed or outdated. One that is current but still
 * reports no session (Codex with herdr 0.9.3, whose session report herdr refuses; or an agent started before the
 * install) gets nothing new, as does any host whose integrations are not known.
 */
fun enableReplyFor(agent: HerdrAgent, integrations: Map<String, HerdrIntegrationState>?): String? {
    if (agent.replyIdentity != null) return null
    val id = HerdrIntegrations.forKind(agent.agent) ?: return null
    val state = integrations?.get(id) ?: return null
    return id.takeIf { state != HerdrIntegrationState.CURRENT }
}

/** One Enable Reply asked for (an Inbox row, a notification's action): the host, the agent's label and the integration. */
data class EnableReplyRequest(val hostId: Long, val hostLabel: String, val agent: String, val integration: String) {
    /** The confirmation, asked once. */
    val question: String
        get() = "or2 installs herdr's $integration integration on $hostLabel. Restart $agent afterwards."

    /** The outcome of an install that went through. */
    val done: String get() = "Done. Restart $agent to reply to it."

    /** What the message says while the install runs. */
    val progress: String get() = "Installing herdr's $integration integration on $hostLabel…"
}

/**
 * Runs a confirmed Enable Reply: [install] (`HostConnections.installHerdrIntegration`, over the host's live connection
 * only) bounded by [timeoutMs] above Rust's own, and returns the outcome to show: [EnableReplyRequest.done] or why not.
 */
suspend fun enableReplyOutcome(
    request: EnableReplyRequest,
    timeoutMs: Long = ENABLE_REPLY_TIMEOUT_MS,
    install: suspend (hostId: Long, integration: String) -> Unit,
): String = try {
    if (withTimeoutOrNull(timeoutMs) { install(request.hostId, request.integration) } == null) NOT_ENABLED_TIMEOUT else request.done
} catch (error: HostException) {
    notEnabled(error, request.hostLabel)
}

/** A safety bound over Rust's query timeout (30 s; the install itself is bounded by the 10 s exec timeout). */
const val ENABLE_REPLY_TIMEOUT_MS = 45_000L
const val NOT_ENABLED_TIMEOUT = "Not enabled: the host did not answer"

/** The longest reason shown after `Not enabled: `. */
private const val MAX_REASON = 80

/** What a failed Enable Reply says; [host] is the host's label. */
fun notEnabled(error: HostException, host: String): String = when (error) {
    is HostException.NotConnected, is HostException.Closed -> "Not enabled: $host is not connected"
    is HostException.NotInstalled -> "Not enabled: ${error.program} is not installed on $host"
    is HostException.InvalidName -> "Not enabled: herdr has no such integration"
    is HostException.CommandFailed -> "Not enabled: " + error.reason.take(MAX_REASON)
    else -> "Not enabled: herdr refused it"
}

/**
 * The Enable Reply waiting for its confirmation, from an Inbox row or a notification's action, for the dialog over
 * whatever is on screen. One at a time: a newer request replaces one not yet answered. Kept by the application, so a
 * recreated activity still asks.
 */
class EnableReplyRequests {
    private val mutableRequest = MutableStateFlow<EnableReplyRequest?>(null)
    val request: StateFlow<EnableReplyRequest?> = mutableRequest.asStateFlow()

    fun request(request: EnableReplyRequest) {
        mutableRequest.value = request
    }

    /** Takes the pending request (the dialog was answered); null when there is none. */
    fun take(): EnableReplyRequest? = mutableRequest.value.also { mutableRequest.value = null }
}

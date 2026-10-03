package io.github.code_akram.or2.inbox

import androidx.compose.ui.graphics.Color
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.HerdrSessionWatch
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.combineEach
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrIntegrationState
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.notify.EnableReplyRequest
import io.github.code_akram.or2.notify.enableReplyFor
import io.github.code_akram.or2.session.hostStateMessage
import io.github.code_akram.or2.ui.Or2Colors
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/** What the inbox shows about a host's connection. */
enum class LinkStatus(val label: String) {
    NOT_CONNECTED("Not connected"),
    CONNECTING("Connecting"),
    NEEDS_HOST_KEY("Waiting for host-key decision"),
    CONNECTED("Connected"),
    FAILED("Connection failed"),

    /** A host the user marked as sleeping (a laptop) whose connection is gone: muted, not a failure. */
    ASLEEP("Asleep"),
    ;

    /** A tap may start a connection: nothing is connecting or connected. */
    val canConnect get() = this == NOT_CONNECTED || this == FAILED || this == ASLEEP
}

/**
 * [sleeps] is the host's own flag: when its connection ended because the host stopped answering
 * ([isSleepFailure]) it reads [LinkStatus.ASLEEP], not [LinkStatus.FAILED]. A rejected key or host
 * key is no sleep, whatever the flag says.
 */
fun linkStatus(state: HostState?, sleeps: Boolean = false): LinkStatus = when (state) {
    null -> LinkStatus.NOT_CONNECTED
    HostState.Connecting, HostState.Authenticating -> LinkStatus.CONNECTING
    is HostState.AwaitingHostKeyDecision -> LinkStatus.NEEDS_HOST_KEY
    is HostState.Connected -> LinkStatus.CONNECTED
    is HostState.Closed -> when (val reason = state.reason) {
        is CloseReason.Failed -> if (sleeps && isSleepFailure(reason.failure)) LinkStatus.ASLEEP else LinkStatus.FAILED
        else -> LinkStatus.NOT_CONNECTED
    }
}

/** The host went quiet (connection lost, unreachable, timed out): what a sleeping host looks like from here. */
fun isSleepFailure(failure: SessionFailure): Boolean =
    failure is SessionFailure.ConnectionLost || failure is SessionFailure.Unreachable || failure is SessionFailure.TimedOut

/**
 * The connection in words, for a host whose [link] was read from [state] (null: it has no connection): an asleep or
 * unconnected host says just that, any other state explains itself (a failure its cause).
 */
fun linkMessage(link: LinkStatus, state: HostState?): String =
    if (link == LinkStatus.ASLEEP || state == null) link.label else hostStateMessage(state)

/** The colour of a host's status dot; null where none is drawn (not connected, asleep). */
fun linkStatusColor(link: LinkStatus): Color? = when (link) {
    LinkStatus.NOT_CONNECTED, LinkStatus.ASLEEP -> null
    LinkStatus.CONNECTING -> Or2Colors.Accent
    LinkStatus.NEEDS_HOST_KEY -> Or2Colors.Attention
    LinkStatus.CONNECTED -> Or2Colors.Done
    LinkStatus.FAILED -> Or2Colors.Danger
}

/**
 * An agent's label: herdr's display name, else its name, else its agent id, else [paneAgent] (the agent herdr names
 * on the pane itself); null when none is set.
 */
fun agentLabel(agent: HerdrAgent?, paneAgent: String? = null): String? =
    listOfNotNull(agent?.displayAgent, agent?.name, agent?.agent, paneAgent).firstOrNull { it.isNotBlank() }

/** "Claude Code" style name for an inbox row or an alert: [agentLabel], else a placeholder. */
fun agentName(agent: HerdrAgent): String = agentLabel(agent) ?: "agent"

/** One agent row of the inbox. */
data class InboxItem(
    val hostId: Long,
    val hostLabel: String,
    /** The name `watch_herdr` and `TerminalTarget.Herdr` take: null for the default session. */
    val session: String?,
    val sessionName: String,
    val paneId: String,
    val agentName: String,
    val status: AgentStatus,
    val workspaceLabel: String?,
    val tabLabel: String?,
    val cwd: String?,
    /**
     * herdr's integration to offer (**Enable Reply**, [enableReplyFor]): the agent has no session, so no Reply, and its
     * kind's integration is not installed or outdated on the host. Null: nothing new.
     */
    val enableReply: String? = null,
) {
    /** What the row's **Enable Reply** asks to confirm, or null when it has none. */
    val enableReplyRequest: EnableReplyRequest? get() = enableReply?.let { EnableReplyRequest(hostId, hostLabel, agentName, it) }
}

/** The items of one status, in display order. */
data class InboxGroup(val status: AgentStatus, val items: List<InboxItem>)

/** Where agents come from: one live view of one herdr session on one host. */
data class InboxSource(
    val hostId: Long,
    val hostLabel: String,
    val session: String?,
    val sessionName: String,
    val view: HerdrView,
    /** herdr's integrations on the host (`ActiveHost.integrations`), null while unknown: see [enableReplyFor]. */
    val integrations: Map<String, HerdrIntegrationState>? = null,
)

/** Blocked agents need the user first; then work in flight, finished work, and idle panes. */
val INBOX_STATUS_ORDER = listOf(AgentStatus.BLOCKED, AgentStatus.WORKING, AgentStatus.DONE, AgentStatus.IDLE, AgentStatus.UNKNOWN)

fun statusLabel(status: AgentStatus) = when (status) {
    AgentStatus.BLOCKED -> "Blocked"
    AgentStatus.WORKING -> "Working"
    AgentStatus.DONE -> "Done"
    AgentStatus.IDLE -> "Idle"
    AgentStatus.UNKNOWN -> "Unknown"
}

/**
 * Agents across all sources, grouped by status in [INBOX_STATUS_ORDER] (empty groups omitted).
 * Within a group: host label, session, workspace, tab, pane, so a refresh never reshuffles rows
 * of the same status.
 */
fun buildInbox(sources: List<InboxSource>): List<InboxGroup> {
    class Row(val item: InboxItem, val workspaceNumber: UInt, val tabNumber: UInt)
    val rows = sources.flatMap { source ->
        val workspaces = source.view.workspaces.associateBy { it.workspaceId }
        val tabs = source.view.tabs.associateBy { it.tabId }
        source.view.agents.map { agent ->
            Row(
                InboxItem(
                    source.hostId, source.hostLabel, source.session, source.sessionName, agent.paneId, agentName(agent),
                    agent.status, workspaces[agent.workspaceId]?.label, tabs[agent.tabId]?.label, agent.cwd,
                    enableReplyFor(agent, source.integrations),
                ),
                workspaces[agent.workspaceId]?.number ?: UInt.MAX_VALUE, tabs[agent.tabId]?.number ?: UInt.MAX_VALUE,
            )
        }
    }
    val order = compareBy<Row>({ it.item.hostLabel.lowercase() }, { it.item.hostId }, { it.item.sessionName },
        { it.workspaceNumber }, { it.tabNumber }, { it.item.paneId })
    return INBOX_STATUS_ORDER.mapNotNull { status ->
        rows.filter { it.item.status == status }.sortedWith(order).map { it.item }.takeIf { it.isNotEmpty() }
            ?.let { InboxGroup(status, it) }
    }
}

/** A host as the inbox lists it, with what is known about its connection and herdr. */
data class InboxHostRow(
    val host: Host,
    val link: LinkStatus,
    /** The connection's state in words; a failure explains itself. */
    val message: String,
    /** herdr in one line once connected (not installed, no running sessions, N agents ...). */
    val herdrNote: String?,
    val agentCount: Int,
)

data class InboxState(val hosts: List<InboxHostRow>, val groups: List<InboxGroup>) {
    val agentCount get() = groups.sumOf { it.items.size }
}

fun herdrNote(caps: HostCapabilities?, capsError: String?, watches: List<Pair<String, HerdrState>>): String? {
    if (capsError != null) return "Could not query the host: $capsError"
    if (caps == null) return "Checking the host…"
    if (caps.herdr == null) return "herdr is not installed"
    if (watches.isEmpty()) return "No running herdr sessions"
    val unavailable = watches.mapNotNull { (_, state) -> state as? HerdrState.Unavailable }
    return when {
        unavailable.size == watches.size -> unavailable.first().let { first ->
            when (first.reason) {
                HerdrUnavailable.NotRunning -> "herdr is not running"
                is HerdrUnavailable.IncompatibleProtocol -> "herdr protocol ${first.reason.protocol} is not supported"
                HerdrUnavailable.NotInstalled -> "herdr is not installed"
                // The core's own explanation, when it has one, beats a bare "unavailable".
                HerdrUnavailable.Failed -> first.message.trim().takeIf { it.isNotEmpty() }
                    ?.let { "herdr is unavailable: $it" } ?: "herdr is unavailable"
            }
        }
        else -> null
    }
}

/** Live views of one connection's watches. */
private fun ActiveHost.liveViews(host: Host): Flow<List<Triple<HerdrSessionWatch, HerdrState, InboxSource?>>> =
    watches.combineEach { watch ->
        watch.state.map { state ->
            Triple(watch, state, (state as? HerdrState.Live)?.let { InboxSource(host.id, host.label, watch.session, watch.name, it.view) })
        }
    }

/**
 * The inbox for the hosts flagged `showInInbox`, following their connections, capability probes
 * and herdr watches. A host's label comes from [hosts] (current), not from the connection.
 */
fun HostConnections.inbox(hosts: Flow<List<Host>>): Flow<InboxState> =
    combine(hosts, this.hosts) { list, active -> list.filter { it.showInInbox }.map { it to active[it.id] } }
        .combineEach { (host, active) -> hostFlow(host, active) }
        .map { parts -> InboxState(parts.map { it.first }, buildInbox(parts.flatMap { it.second })) }

private fun hostFlow(host: Host, active: ActiveHost?): Flow<Pair<InboxHostRow, List<InboxSource>>> {
    if (active == null) {
        return flowOf(InboxHostRow(host, LinkStatus.NOT_CONNECTED, linkMessage(LinkStatus.NOT_CONNECTED, null), null, 0) to emptyList())
    }
    return combine(
        active.state, active.capabilities, active.capabilitiesError, active.liveViews(host), active.integrations,
    ) { state, caps, capsError, views, integrations ->
        val link = linkStatus(state, host.sleeps)
        val sources = if (link == LinkStatus.CONNECTED) views.mapNotNull { it.third?.copy(integrations = integrations) } else emptyList()
        val note = if (link == LinkStatus.CONNECTED) herdrNote(caps, capsError, views.map { it.first.name to it.second }) else null
        InboxHostRow(host, link, linkMessage(link, state), note, sources.sumOf { it.view.agents.size }) to sources
    }
}

/** The state of every connection, keyed by host id; hosts without a connection are absent. */
fun HostConnections.hostStates(): Flow<Map<Long, HostState>> =
    hosts.map { it.values }.combineEach { a -> a.state.map { a.host.id to it } }.map { it.toMap() }

/**
 * The live view of every herdr watch of every connection, keyed by host id and session (null: the default session),
 * whatever the host's inbox flag; a watch that is not live is absent.
 */
fun HostConnections.herdrViews(): Flow<Map<Pair<Long, String?>, HerdrView>> =
    hosts.map { it.values }.combineEach { a -> a.liveViews(a.host) }.map { parts ->
        parts.flatMap { views -> views.mapNotNull { it.third } }.associate { (it.hostId to it.session) to it.view }
    }

/** A host-key decision the user has not made yet. */
data class PendingHostKey(val active: ActiveHost, val prompt: HostState.AwaitingHostKeyDecision)

/** Connections waiting for a host-key decision, in host-id order. */
fun HostConnections.pendingHostKeys(): Flow<List<PendingHostKey>> =
    hosts.map { it.values.sortedBy { a -> a.host.id } }
        .combineEach { a -> a.state.map { s -> (s as? HostState.AwaitingHostKeyDecision)?.let { PendingHostKey(a, it) } } }
        .map { it.filterNotNull() }

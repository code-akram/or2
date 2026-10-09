package io.github.code_akram.or2.service

import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.combineEach
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.inbox.herdrViews
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map

/** A host connection as the service sees it: open (not closed) or not. */
data class OpenHost(val id: Long, val label: String, val open: Boolean)

/** A terminal as the service sees it. [open] is false once it has closed. */
data class OpenTerminal(val hostId: Long, val hostLabel: String, val open: Boolean)

/** One host in the notification: its label and how many sessions are open on it. */
data class HostEntry(val hostId: Long, val label: String, val sessions: Int)

/**
 * The agents of every live herdr watch, across hosts: how many need input (`Blocked`) and how many are working. Other
 * statuses are not counted.
 */
data class AgentSummary(val needsInput: Int = 0, val working: Int = 0) {
    /** `1 needs input · 2 working`; null when no agent needs input or works. */
    val text: String?
        get() = listOfNotNull(
            needsInput.takeIf { it > 0 }?.let { "$it needs input" },
            working.takeIf { it > 0 }?.let { "$it working" },
        ).joinToString(" · ").ifEmpty { null }
}

/** The agents of [views] (every live herdr view, one per watched session) counted by status. */
fun agentSummary(views: Collection<HerdrView>): AgentSummary {
    val agents = views.flatMap { it.agents }
    return AgentSummary(agents.count { it.status == AgentStatus.BLOCKED }, agents.count { it.status == AgentStatus.WORKING })
}

/**
 * Everything the foreground service needs to know: which hosts and sessions are open, and what their agents do. A host
 * counts while its connection is not closed, and also while a mosh session of it is open after
 * the SSH connection was lost (mosh sessions outlive their host connection).
 */
data class ServiceSnapshot(val hosts: List<HostEntry>, val agents: AgentSummary = AgentSummary()) {
    val sessions get() = hosts.sumOf { it.sessions }

    /** Nothing is open: the service has no reason to run. */
    val idle get() = hosts.isEmpty()
}

fun serviceSnapshot(hosts: Collection<OpenHost>, terminals: Collection<OpenTerminal>): ServiceSnapshot {
    val openTerminals = terminals.filter { it.open }
    val entries = LinkedHashMap<Long, HostEntry>()
    for (host in hosts) {
        if (host.open) entries[host.id] = HostEntry(host.id, host.label, openTerminals.count { it.hostId == host.id })
    }
    for (terminal in openTerminals) {
        // A live session of a host whose own connection is gone still keeps the service up.
        entries.getOrPut(terminal.hostId) { HostEntry(terminal.hostId, terminal.hostLabel, openTerminals.count { it.hostId == terminal.hostId }) }
    }
    return ServiceSnapshot(entries.values.sortedBy { it.label.lowercase() })
}

/**
 * What the ongoing notification says. While an agent needs input it asks to be a Live Update ([promoted], Android 16:
 * promoted to the status bar's chip, with [shortCriticalText] such as `1 input`); otherwise it is the ordinary ongoing
 * notification.
 */
data class NotificationContent(
    val title: String,
    val text: String,
    val lines: List<String>,
    val promoted: Boolean = false,
    val shortCriticalText: String? = null,
)

fun notificationContent(snapshot: ServiceSnapshot): NotificationContent {
    val hosts = snapshot.hosts
    val sessions = snapshot.sessions
    val title = when {
        hosts.isEmpty() -> "or2"
        hosts.size == 1 -> "Connected to ${hosts[0].label}"
        else -> "Connected to ${hosts.size} hosts"
    }
    val open = when {
        hosts.isEmpty() -> "Starting…"
        sessions == 0 -> "No open sessions"
        sessions == 1 -> "1 open session"
        else -> "$sessions open sessions"
    }
    // The agents first: what may need the user.
    val summary = snapshot.agents.text.takeIf { hosts.isNotEmpty() }
    val text = listOfNotNull(summary, open).joinToString(" · ")
    val needsInput = snapshot.agents.needsInput.takeIf { hosts.isNotEmpty() } ?: 0
    val lines = hosts.map { host ->
        host.label + when (host.sessions) {
            0 -> ""
            1 -> " · 1 session"
            else -> " · ${host.sessions} sessions"
        }
    }
    return NotificationContent(
        title, text, lines,
        promoted = needsInput > 0,
        shortCriticalText = if (needsInput > 0) "$needsInput input" else null,
    )
}

/**
 * The service's view of [HostConnections]: changes whenever a host or terminal opens, closes or is replaced, and
 * whenever the count of agents that need input or work changes (from the herdr watches the connections already
 * have). The application collects it once and shares it (`Or2Application.serviceSnapshots`).
 */
fun HostConnections.serviceSnapshots(): Flow<ServiceSnapshot> {
    val hostViews = hosts.map { it.values }.combineEach { a -> a.state.map { OpenHost(a.host.id, a.host.label, it !is HostState.Closed) } }
    val terminalViews = terminals.combineEach { t -> t.state.map { OpenTerminal(t.host.id, t.host.label, it !is SessionState.Closed) } }
    val agents = herdrViews().map { agentSummary(it.values) }.distinctUntilChanged()
    return combine(hostViews, terminalViews, agents) { h, t, a -> serviceSnapshot(h, t).copy(agents = a) }.distinctUntilChanged()
}

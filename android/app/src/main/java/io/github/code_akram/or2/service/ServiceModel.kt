package io.github.code_akram.or2.service

import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/** A host connection as the service sees it: open (not closed) or not. */
data class OpenHost(val id: Long, val label: String, val open: Boolean)

/** A terminal as the service sees it. [open] is false once it has closed. */
data class OpenTerminal(val hostId: Long, val hostLabel: String, val open: Boolean)

/** One host in the notification: its label and how many sessions are open on it. */
data class HostEntry(val hostId: Long, val label: String, val sessions: Int)

/**
 * Everything the foreground service needs to know: which hosts and sessions are open. A host
 * counts while its connection is not closed, and also while a mosh session of it is open after
 * the SSH connection was lost (mosh sessions outlive their host connection).
 */
data class ServiceSnapshot(val hosts: List<HostEntry>) {
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

/** What the ongoing notification says. */
data class NotificationContent(val title: String, val text: String, val lines: List<String>)

fun notificationContent(snapshot: ServiceSnapshot): NotificationContent {
    val hosts = snapshot.hosts
    val sessions = snapshot.sessions
    val title = when {
        hosts.isEmpty() -> "or2"
        hosts.size == 1 -> "Connected to ${hosts[0].label}"
        else -> "Connected to ${hosts.size} hosts"
    }
    val text = when {
        hosts.isEmpty() -> "Starting…"
        sessions == 0 -> "No open sessions"
        sessions == 1 -> "1 open session"
        else -> "$sessions open sessions"
    }
    val lines = hosts.map { host ->
        host.label + when (host.sessions) {
            0 -> ""
            1 -> " · 1 session"
            else -> " · ${host.sessions} sessions"
        }
    }
    return NotificationContent(title, text, lines)
}

/** The service's view of [HostConnections]: changes whenever a host or terminal opens, closes or is replaced. */
@OptIn(ExperimentalCoroutinesApi::class)
fun HostConnections.serviceSnapshots(): Flow<ServiceSnapshot> {
    val hostViews = hosts.flatMapLatest { active ->
        if (active.isEmpty()) flowOf(emptyList())
        else combine(active.values.map { a -> a.state.map { OpenHost(a.host.id, a.host.label, it !is HostState.Closed) } }) { it.toList() }
    }
    val terminalViews = terminals.flatMapLatest { list ->
        if (list.isEmpty()) flowOf(emptyList())
        else combine(list.map { t -> t.state.map { OpenTerminal(t.host.id, t.host.label, it !is SessionState.Closed) } }) { it.toList() }
    }
    return combine(hostViews, terminalViews) { h, t -> serviceSnapshot(h, t) }.distinctUntilChanged()
}

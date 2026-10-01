package io.github.code_akram.or2.inbox

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus

/** Test tag of an agent row: host, session (`-` for the default one) and pane identify it. */
fun inboxItemTag(item: InboxItem) = "inbox-item:${item.hostId}:${item.session ?: "-"}:${item.paneId}"

/**
 * The start destination: every agent across the hosts flagged for the inbox, blocked first, plus
 * each host's connection status with its connect action. Stateless: the caller supplies the
 * assembled [state] and handles the actions.
 */
@Composable
fun InboxScreen(
    state: InboxState,
    busy: Boolean,
    connectAll: () -> Unit,
    connect: (Host) -> Unit,
    openHost: (Host) -> Unit,
    openAgent: (InboxItem) -> Unit,
    modifier: Modifier = Modifier,
    openTerminals: @Composable () -> Unit = {},
) {
    val connectable = state.hosts.filter {
        it.host.keyId != null && (it.link == LinkStatus.NOT_CONNECTED || it.link == LinkStatus.FAILED)
    }
    LazyColumn(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
                Text(
                    if (state.agentCount == 0) "Inbox" else "Inbox · ${state.agentCount} agent${if (state.agentCount == 1) "" else "s"}",
                    style = MaterialTheme.typography.titleLarge,
                )
                if (connectable.size > 1) {
                    Button(onClick = connectAll, enabled = !busy, modifier = Modifier.testTag("inbox-connect-all")) { Text("Connect all") }
                }
            }
        }
        if (state.hosts.isEmpty()) item {
            Text("No hosts show agents here. Add a host on the Hosts tab and leave \"Show agents in the inbox\" on.")
        }
        items(state.hosts, key = { "host:${it.host.id}" }) { row ->
            HostStatusRow(row, busy, { connect(row.host) }, { openHost(row.host) })
        }
        item { openTerminals() }
        if (state.hosts.isNotEmpty() && state.groups.isEmpty() && state.hosts.any { it.link == LinkStatus.CONNECTED }) item {
            Text("No agents are running on the connected hosts.", modifier = Modifier.testTag("inbox-empty"))
        }
        state.groups.forEach { group ->
            item(key = "group:${group.status}") {
                Text("${statusLabel(group.status)} · ${group.items.size}", style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.padding(top = 8.dp).testTag("inbox-group:${group.status}"))
            }
            items(group.items, key = { inboxItemTag(it) }) { item -> AgentRow(item, openAgent) }
        }
    }
}

@Composable
private fun HostStatusRow(row: InboxHostRow, busy: Boolean, connect: () -> Unit, open: () -> Unit) {
    Card(Modifier.fillMaxWidth().testTag("inbox-host:${row.host.id}"),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Row(Modifier.padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(row.host.label, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(row.message, style = MaterialTheme.typography.bodySmall,
                    color = if (row.link == LinkStatus.FAILED) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                row.herdrNote?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            }
            if (row.link == LinkStatus.NOT_CONNECTED || row.link == LinkStatus.FAILED) {
                TextButton(onClick = connect, enabled = !busy && row.host.keyId != null,
                    modifier = Modifier.testTag("inbox-connect:${row.host.id}")) { Text(if (row.link == LinkStatus.FAILED) "Retry" else "Unlock") }
            }
            TextButton(onClick = open, modifier = Modifier.testTag("inbox-open:${row.host.id}")) { Text("Open") }
        }
    }
}

@Composable
private fun AgentRow(item: InboxItem, open: (InboxItem) -> Unit) {
    Card(Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag(inboxItemTag(item)).clickable { open(item) }) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
                Text(item.agentName, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f))
                StatusChip(item.status)
            }
            Text(item.hostLabel + (if (item.session != null) " · ${item.sessionName}" else ""), style = MaterialTheme.typography.bodySmall)
            val place = listOfNotNull(item.workspaceLabel, item.tabLabel).joinToString(" / ")
            if (place.isNotEmpty()) Text(place, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            item.cwd?.let {
                Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1, overflow = TextOverflow.StartEllipsis)
            }
        }
    }
}

@Composable
fun StatusChip(status: AgentStatus, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    val (container, content) = when (status) {
        AgentStatus.BLOCKED -> scheme.errorContainer to scheme.onErrorContainer
        AgentStatus.WORKING -> scheme.primaryContainer to scheme.onPrimaryContainer
        AgentStatus.DONE -> scheme.tertiaryContainer to scheme.onTertiaryContainer
        AgentStatus.IDLE, AgentStatus.UNKNOWN -> scheme.surfaceVariant to scheme.onSurfaceVariant
    }
    Surface(color = container, contentColor = content, shape = RoundedCornerShape(8.dp), modifier = modifier.testTag("status-chip")) {
        Text(statusLabel(status), style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp))
    }
}

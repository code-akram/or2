package io.github.code_akram.or2.inbox

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ui.ActionCard
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.EmptyState
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.drawCornerGlow
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TopBar

/** The dot column of agent rows and host rows: both start their text at the same x. */
private val LeadingSlot = 26.dp

/** Test tag of an agent row: host, session (`-` for the default one) and pane identify it. */
fun inboxItemTag(item: InboxItem) = "inbox-item:${item.hostId}:${item.session ?: "-"}:${item.paneId}"

fun statusColor(status: AgentStatus): Color = when (status) {
    AgentStatus.BLOCKED -> Or2Colors.Attention
    AgentStatus.WORKING -> Or2Colors.Working
    AgentStatus.DONE -> Or2Colors.Done
    AgentStatus.IDLE, AgentStatus.UNKNOWN -> Or2Colors.Idle
}

private fun linkColor(link: LinkStatus): Color? = when (link) {
    LinkStatus.NOT_CONNECTED -> null
    LinkStatus.CONNECTING -> Or2Colors.Accent
    LinkStatus.NEEDS_HOST_KEY -> Or2Colors.Attention
    LinkStatus.CONNECTED -> Or2Colors.Done
    LinkStatus.FAILED -> Or2Colors.Danger
}

/**
 * Every agent across the hosts flagged for the inbox, blocked first and tinted, under sticky
 * status headers; below them each host's connection status with its connect action. Stateless:
 * the caller supplies the assembled [state] and handles the actions.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun InboxScreen(
    state: InboxState,
    busy: Boolean,
    connectAll: () -> Unit,
    connect: (Host) -> Unit,
    openHost: (Host) -> Unit,
    openAgent: (InboxItem) -> Unit,
    modifier: Modifier = Modifier,
    openHome: () -> Unit = {},
    openKeys: () -> Unit = {},
    addHost: () -> Unit = {},
) {
    val connectable = state.hosts.filter {
        it.host.keyId != null && (it.link == LinkStatus.NOT_CONNECTED || it.link == LinkStatus.FAILED)
    }
    val anyConnected = state.hosts.any { it.link == LinkStatus.CONNECTED }
    Column(modifier.fillMaxSize()) {
        TopBar(endPadding = Or2Dimens.Gutter, actions = {
            IconAction(Or2Icons.Home, "Home", openHome, Modifier.testTag("nav-home"))
            IconAction(Or2Icons.Key, "SSH keys", openKeys, Modifier.testTag("nav-keys"))
        })
        LazyColumn(
            Modifier.fillMaxSize().testTag("inbox-list").drawBehind {
                // Opaque sticky headers need a flat surface to sit on, so the list area is plain
                // `background` plus the corner glow (the top glow ends above it).
                drawRect(Or2Colors.Background)
                drawCornerGlow()
            }, contentPadding = PaddingValues(horizontal = Or2Dimens.Gutter),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (state.hosts.isEmpty()) {
                item(key = "empty-hosts") {
                    Column(Modifier.padding(top = 96.dp).testTag("inbox-no-hosts"), verticalArrangement = Arrangement.spacedBy(32.dp)) {
                        EmptyState(
                            Or2Icons.Inbox, "No agents to watch",
                            "No hosts show agents here. Add a host and leave \"Show agents in the inbox\" on.",
                        )
                        ActionCard("First step", "Add a host", "Hostname, user and the key to use.", meta = "~3 min · needs hostname + key",
                            onClick = addHost)
                    }
                }
            } else if (state.groups.isEmpty()) {
                item(key = "empty-agents") {
                    EmptyState(
                        Or2Icons.Inbox, "No agent events yet",
                        if (anyConnected) "No agents are running on the connected hosts.\nWhen an agent asks for approval, a question,\nor finishes a task, it shows up here."
                        else "Connect a host to see its agents.\nWhen an agent asks for approval, a question,\nor finishes a task, it shows up here.",
                        Modifier.padding(top = 72.dp, bottom = 16.dp).testTag("inbox-empty"),
                    )
                }
            }
            state.groups.forEach { group ->
                stickyHeader(key = "group:${group.status}") {
                    Row(
                        Modifier.fillMaxWidth().background(Or2Colors.Background).padding(top = 16.dp, bottom = 8.dp)
                            .testTag("inbox-group:${group.status}"),
                    ) {
                        StatusDot(statusColor(group.status), Modifier.align(Alignment.CenterVertically))
                        Spacer(Modifier.width(10.dp))
                        Text(
                            "${statusLabel(group.status)} · ${group.items.size}".uppercase(), style = Or2Type.SectionHeader,
                            color = Or2Colors.TextMuted,
                        )
                    }
                }
                items(group.items, key = { inboxItemTag(it) }) { item -> AgentRow(item, openAgent) }
            }
            if (state.hosts.isNotEmpty()) {
                item(key = "hosts-header") {
                    Row(Modifier.fillMaxWidth().padding(top = Or2Dimens.SectionGap - 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.weight(1f)) { SectionHeader("Hosts", topGap = 0.dp) }
                        if (connectable.size > 1) {
                            PillButton("Connect all", connectAll, Modifier.testTag("inbox-connect-all"), enabled = !busy)
                        }
                    }
                }
                item(key = "hosts") {
                    GroupCard {
                        state.hosts.forEachIndexed { index, row ->
                            if (index > 0) GroupDivider(inset = 42.dp)
                            HostStatusRow(row, busy, { connect(row.host) }, { openHost(row.host) })
                        }
                    }
                }
            }
            item(key = "end") {
                Spacer(Modifier.height(32.dp))
                BottomInsetSpacer() // The list scrolls under the gesture bar, never cut above it.
            }
        }
    }
}

@Composable
private fun HostStatusRow(row: InboxHostRow, busy: Boolean, connect: () -> Unit, open: () -> Unit) {
    val failed = row.link == LinkStatus.FAILED
    Box(Modifier.fillMaxWidth().testTag("inbox-host:${row.host.id}")) {
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = open).testTag("inbox-open:${row.host.id}")
            .padding(start = Or2Dimens.Gutter, end = 8.dp, top = 12.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(LeadingSlot), contentAlignment = Alignment.CenterStart) {
            linkColor(row.link)?.let { StatusDot(it) }
        }
        Column(Modifier.weight(1f)) {
            Text(row.host.label, style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(row.message, style = Or2Type.Secondary, color = if (failed) Or2Colors.Danger else Or2Colors.TextMuted)
            // herdr's own explanation, muted: why there are no agents here.
            row.herdrNote?.let { Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.testTag("inbox-herdr-note:${row.host.id}")) }
        }
        if (row.link == LinkStatus.NOT_CONNECTED || failed) {
            PillButton(if (failed) "Retry" else "Unlock", connect, Modifier.testTag("inbox-connect:${row.host.id}"),
                enabled = !busy && row.host.keyId != null)
        }
    }
    }
}

@Composable
private fun AgentRow(item: InboxItem, open: (InboxItem) -> Unit) {
    val blocked = item.status == AgentStatus.BLOCKED
    Or2Card(
        Modifier.testTag(inboxItemTag(item)), onClick = { open(item) },
        color = if (blocked) Or2Colors.AttentionSurface else Or2Colors.Surface,
        border = if (blocked) BorderStroke(1.dp, Or2Colors.AttentionBorder) else null,
    ) {
        Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(LeadingSlot), contentAlignment = Alignment.CenterStart) {
                StatusDot(statusColor(item.status), pulsing = item.status == AgentStatus.WORKING)
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(item.agentName, style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                val place = listOfNotNull(item.workspaceLabel, item.tabLabel).joinToString(" / ")
                Text(
                    listOf(item.hostLabel + (if (item.session != null) " · ${item.sessionName}" else ""), place).filter { it.isNotEmpty() }.joinToString(" · "),
                    style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                item.cwd?.let {
                    Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.StartEllipsis)
                }
            }
            Spacer(Modifier.width(12.dp))
            Text(statusLabel(item.status), style = Or2Type.Secondary, color = if (blocked) Or2Colors.Attention else Or2Colors.TextMuted,
                modifier = Modifier.testTag("status-chip"))
        }
    }
}

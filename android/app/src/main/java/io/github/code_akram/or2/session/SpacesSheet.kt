package io.github.code_akram.or2.session

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.app.HerdrFocus
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.inbox.AgentTitle
import io.github.code_akram.or2.inbox.agentLabel
import io.github.code_akram.or2.inbox.agentTitle
import io.github.code_akram.or2.inbox.statusColor
import io.github.code_akram.or2.inbox.statusLabel
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.StatusDot

/** An agent's pane under its tab in the Spaces sheet: its label ([agentLabel]), title ([agentTitle]) and status. */
data class SpaceAgent(val paneId: String, val label: String, val title: String?, val status: AgentStatus, val current: Boolean)

/** One herdr tab: `tab <label>`, whether herdr has it focused, and its panes that hold an agent. */
data class SpaceTab(val tabId: String, val label: String, val current: Boolean, val agents: List<SpaceAgent>)

/** One herdr space (workspace) with its tabs. */
data class Space(val workspaceId: String, val label: String, val tabs: List<SpaceTab>)

/**
 * The Spaces sheet's content from a herdr session's live [view]: each space in herdr's order (its number), its tabs
 * in herdr's order, and under each tab the agents of its panes in herdr's pane order. Spaces without tabs are left
 * out. The focused tab (herdr's `focused_tab_id`, else the focused pane's tab) and the focused pane are `current`.
 */
fun spacesOf(view: HerdrView): List<Space> {
    val panes = view.panes.associateBy { it.paneId }
    val paneOrder = view.panes.withIndex().associate { (index, pane) -> pane.paneId to index }
    val focusedPane = view.focusedPaneId
    val focusedTab = view.focusedTabId ?: view.agents.firstOrNull { it.paneId == focusedPane }?.tabId
    return view.workspaces.sortedBy { it.number }.mapNotNull { workspace ->
        val tabs = view.tabs.filter { it.workspaceId == workspace.workspaceId }.sortedBy { it.number }.map { tab ->
            val agents = view.agents.filter { it.tabId == tab.tabId }
                .sortedWith(compareBy({ paneOrder[it.paneId] ?: Int.MAX_VALUE }, { it.paneId }))
                .map { agent ->
                    val label = agentLabel(agent, panes[agent.paneId]?.agent) ?: "agent"
                    SpaceAgent(agent.paneId, label, agentTitle(agent, label), agent.status, agent.paneId == focusedPane)
                }
            SpaceTab(tab.tabId, tab.label.ifBlank { tab.number.toString() }, tab.tabId == focusedTab, agents)
        }
        tabs.takeIf { it.isNotEmpty() }?.let { Space(workspace.workspaceId, workspace.label.ifBlank { workspace.number.toString() }, it) }
    }
}

/** The row text of a tab: `tab 1`, `tab ui`. */
fun tabRowLabel(tab: SpaceTab): String = "tab ${tab.label}"

/**
 * The Spaces sheet (the terminal header's blue disc, herdr terminals only): the herdr session's spaces from its live
 * [view] (null: none yet), each a small header with its label over one card of its tabs (`tab <label>`) and, under
 * each tab, its agents (status dot and word in the Inbox's colours, label, title); the focused tab and pane marked
 * `● Current`. A tap on a tab or an agent is [focus] (the caller closes the sheet and focuses it through herdr; the
 * terminal shows it as herdr draws it). Nothing else: herdr creates, renames and closes spaces and tabs. [session] is
 * the herdr session's name, muted under the title; null for the default session.
 */
@Composable
fun SpacesSheet(session: String?, view: HerdrView?, focus: (HerdrFocus) -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = "Spaces", subtitle = session, modifier = Modifier.testTag("spaces-sheet")) {
        val spaces = view?.let(::spacesOf)
        when {
            spaces == null -> Muted("Waiting for herdr…", Modifier.testTag("spaces-waiting"))
            spaces.isEmpty() -> Muted("No spaces.", Modifier.testTag("spaces-empty"))
            else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                spaces.forEach { space -> SpaceCard(space, focus) }
            }
        }
    }
}

@Composable
private fun Muted(text: String, modifier: Modifier = Modifier) {
    Text(text, style = Or2Type.Body, color = Or2Colors.TextMuted, modifier = modifier.padding(horizontal = 4.dp, vertical = 6.dp))
}

/** A space: its label as a small muted header (in herdr's own case), then one card of its tabs and their agents. */
@Composable
private fun SpaceCard(space: Space, focus: (HerdrFocus) -> Unit) {
    Column(Modifier.testTag("space:${space.workspaceId}")) {
        Text(
            space.label, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 4.dp, bottom = 4.dp).testTag("space-label:${space.workspaceId}"),
        )
        GroupCard {
            space.tabs.forEachIndexed { index, tab ->
                if (index > 0) GroupDivider()
                TabRow(tab) { focus(HerdrFocus.Tab(tab.tabId)) }
                tab.agents.forEach { agent ->
                    GroupDivider(inset = AgentInset)
                    AgentRow(agent) { focus(HerdrFocus.Pane(agent.paneId)) }
                }
            }
        }
    }
}

/** Agent rows sit under their tab, indented by this much. */
private val AgentInset = Or2Dimens.Gutter + 12.dp

@Composable
private fun TabRow(tab: SpaceTab, open: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = Or2Dimens.RowMin).clickable(role = Role.Button, onClick = open)
            .testTag("space-tab:${tab.tabId}").padding(horizontal = Or2Dimens.Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(tabRowLabel(tab), style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f))
        if (tab.current) CurrentMark(Modifier.testTag("space-tab-current:${tab.tabId}"))
    }
}

/** An agent's pane: its status dot, label and title, the status word at the right, and `● Current` under it when focused. */
@Composable
private fun AgentRow(agent: SpaceAgent, open: () -> Unit) {
    val blocked = agent.status == AgentStatus.BLOCKED
    Row(
        Modifier.fillMaxWidth().heightIn(min = if (agent.title != null) Or2Dimens.RowMinSubtitle else Or2Dimens.RowMin)
            .clickable(role = Role.Button, onClick = open).testTag("space-agent:${agent.paneId}")
            .padding(start = AgentInset, end = Or2Dimens.Gutter, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(16.dp), contentAlignment = Alignment.CenterStart) {
            StatusDot(statusColor(agent.status), pulsing = agent.status == AgentStatus.WORKING)
        }
        Column(Modifier.weight(1f)) {
            Text(agent.label, style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            agent.title?.let { AgentTitle(it, Modifier.testTag("space-agent-title:${agent.paneId}")) }
        }
        Spacer(Modifier.width(8.dp))
        Column(horizontalAlignment = Alignment.End) {
            Text(
                statusLabel(agent.status), style = Or2Type.Secondary, color = if (blocked) Or2Colors.Attention else Or2Colors.TextMuted,
                modifier = Modifier.testTag("space-agent-status:${agent.paneId}"),
            )
            if (agent.current) CurrentMark(Modifier.padding(top = 2.dp).testTag("space-agent-current:${agent.paneId}"))
        }
    }
}

/** `● Current`: herdr has it focused, so the terminal shows it (the Terminals sheet's mark). */
@Composable
private fun CurrentMark(modifier: Modifier = Modifier, color: Color = Or2Colors.Accent) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        StatusDot(color)
        Spacer(Modifier.width(6.dp))
        Text("Current", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
    }
}

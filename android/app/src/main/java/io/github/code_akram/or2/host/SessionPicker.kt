package io.github.code_akram.or2.host

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.connection.UDP_BLOCKED_LINE
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.inbox.AgentTitle
import io.github.code_akram.or2.inbox.agentLabel
import io.github.code_akram.or2.inbox.agentTitle
import io.github.code_akram.or2.inbox.statusColor
import io.github.code_akram.or2.inbox.statusLabel
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.Segmented
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.StatusDot

/** The tmux session list of a connected host as the picker last learned it. */
sealed interface TmuxList {
    data object Loading : TmuxList
    data class Loaded(val sessions: List<TmuxSession>) : TmuxList
    data class Failed(val message: String) : TmuxList
}

/** Project paths/history contribution on this connection, independent of the capability probe. */
sealed interface DirectoryList {
    data object Loading : DirectoryList
    data class Loaded(val paths: List<String>) : DirectoryList
    data class Failed(val message: String) : DirectoryList
}

/** Mirrors the Rust rules: nonempty, at most 128 bytes, no control characters, `\`, `:` or `.`. */
fun tmuxNameError(name: String): String? = when {
    name.isEmpty() -> "Enter a session name."
    name.toByteArray(Charsets.UTF_8).size > 128 -> "Use at most 128 bytes."
    name.any { it.isISOControl() } -> "Remove control characters."
    name.any { it == '\\' || it == ':' || it == '.' } -> "Remove backslash, colon and dot characters."
    else -> null
}

enum class PickerTab(val label: String) { HERDR("herdr"), TMUX("tmux"), DIRS("Dirs") }

/**
 * The herdr and tmux sessions of one host that already have an open terminal in or2: the picker marks their rows
 * `● Open`, and choosing one switches to that terminal. [herdr] holds session names, null for herdr's default session.
 */
data class OpenSessions(val herdr: Set<String?> = emptySet(), val tmux: Set<String> = emptySet()) {
    /** Whether [session]'s row is marked: the default session counts by name too (an agent's terminal may name it). */
    fun has(session: HerdrSessionInfo): Boolean = session.name in herdr || (session.isDefault && null in herdr)

    fun has(session: TmuxSession): Boolean = session.name in tmux

    companion object {
        /** The marks for the [targets] of a host's open terminals (not closed, not being closed); a shell marks nothing. */
        fun of(targets: List<TerminalTarget>) = OpenSessions(
            herdr = targets.filterIsInstance<TerminalTarget.Herdr>().map { it.session }.toSet(),
            tmux = targets.filterIsInstance<TerminalTarget.Tmux>().map { it.sessionName }.toSet(),
        )
    }
}

/**
 * One agent under its herdr session in the picker: [label] ([agentLabel]), its status, its pane's cwd and the task it is
 * on ([title], [agentTitle]).
 */
data class PickerAgent(val paneId: String, val label: String, val status: AgentStatus, val cwd: String?, val title: String? = null)

/** The agents of one herdr workspace, under its [label] (null: herdr names no workspace for them). */
data class PickerWorkspace(val label: String?, val agents: List<PickerAgent>)

/**
 * One herdr session as the picker lists it. [workspaces] is null when the app has no live view of it (its host's
 * sessions are not watched, it is not running, its watch has not answered yet): then it is one row, as before agents
 * were listed. A live session with no agents has an empty list.
 */
data class PickerHerdrSession(val info: HerdrSessionInfo, val workspaces: List<PickerWorkspace>?) {
    /** What opens it (`TerminalTarget.Herdr`, `openAgent`): null for herdr's default session, never its listed name. */
    val session: String? get() = if (info.isDefault) null else info.name

    /** Its name as listed; the default session has no `(default)` suffix. */
    val label: String get() = info.name

    val live: Boolean get() = workspaces != null

    /** A live view means it runs, whatever the (cached) capability probe said. */
    val running: Boolean get() = live || info.running
}

/**
 * The herdr tab's sessions from the host's [listed] sessions and its live [views] (keyed as `herdrViews()` keys them:
 * null for the default session): the live ones first with their agents, then the others as single rows, each part in
 * the listing's order.
 */
fun pickerHerdrSessions(listed: List<HerdrSessionInfo>, views: Map<String?, HerdrView>): List<PickerHerdrSession> {
    val sessions = listed.map { info ->
        PickerHerdrSession(info, views[if (info.isDefault) null else info.name]?.let(::pickerWorkspaces))
    }
    return sessions.filter { it.live } + sessions.filterNot { it.live }
}

/**
 * A live view's agents grouped by workspace in herdr's workspace order (workspaces without agents left out), each
 * group by tab, then pane, so a new view never reshuffles the rows; agents in no listed workspace come last.
 */
fun pickerWorkspaces(view: HerdrView): List<PickerWorkspace> {
    val tabs = view.tabs.associateBy { it.tabId }
    val panes = view.panes.associateBy { it.paneId }
    fun rows(agents: List<HerdrAgent>) = agents
        .sortedWith(compareBy<HerdrAgent>({ tabs[it.tabId]?.number ?: UInt.MAX_VALUE }, { it.paneId }))
        .map { agent ->
            val pane = panes[agent.paneId]
            val label = agentLabel(agent, pane?.agent) ?: "agent"
            PickerAgent(agent.paneId, label, agent.status, agent.cwd ?: pane?.cwd, agentTitle(agent, label))
        }
    val workspaces = view.workspaces.sortedBy { it.number }
    val grouped = workspaces.mapNotNull { workspace ->
        view.agents.filter { it.workspaceId == workspace.workspaceId }.takeIf { it.isNotEmpty() }
            ?.let { PickerWorkspace(workspace.label.takeIf { label -> label.isNotBlank() }, rows(it)) }
    }
    val known = workspaces.map { it.workspaceId }.toSet()
    val stray = view.agents.filter { it.workspaceId !in known }
    return if (stray.isEmpty()) grouped else grouped + PickerWorkspace(null, rows(stray))
}

/**
 * The session picker over Home (a host card's header opens it): a segmented control (herdr, tmux, Dirs) with a "Shell" pill
 * (the `>_` glyph) that opens a plain shell, and one grouped list below. The herdr tab lists each running session's
 * agents from the host's live views ([herdrViews], by session: null for the default one) under the session's own row (its name and agent count);
 * tapping an agent is [openAgent]. A session that already has an open terminal is marked `● Open` ([open]): choosing
 * it switches to that terminal. The tmux tab is read again whenever it is shown ([tmuxShown]) and has Refresh, with a
 * spinner beside it while [refreshing]. Hosts without tmux or herdr, failed listings and errors are explained in muted
 * text, never hidden; so is mosh's UDP being blocked ([udpBlocked]), under the tabs. While [gate] is set (the host is
 * not connected yet) the sheet shows it instead: the host's progress, or why it is not connected with [gateAction]'s
 * pill; the lists follow in the same sheet once the gate is null. Dirs has recent project paths from the connection's
 * live herdr directories plus its independent history read; a tap opens a new shell there, with its own Refresh.
 */
@Composable
fun SessionPickerSheet(
    caps: HostCapabilities?,
    capsError: String?,
    tmux: TmuxList,
    open: OpenSessions,
    openShell: () -> Unit,
    openTmux: (String) -> Unit,
    openHerdr: (session: String?) -> Unit,
    refresh: () -> Unit,
    dismiss: () -> Unit,
    initialTab: PickerTab? = null,
    gate: PickerGate? = null,
    gateAction: (GateAction) -> Unit = {},
    /** The host's name: nothing else on screen says which host the sheet is for. */
    title: String? = null,
    udpBlocked: Boolean = false,
    herdrViews: Map<String?, HerdrView> = emptyMap(),
    openAgent: (session: String?, paneId: String) -> Unit = { _, _ -> },
    refreshing: Boolean = false,
    tmuxShown: () -> Unit = {},
    directories: DirectoryList = DirectoryList.Loading,
    openDirectory: (String) -> Unit = {},
    refreshDirectories: () -> Unit = {},
    readingDirectories: Boolean = false,
) {
    var chosen by remember { mutableStateOf(initialTab) }
    val tab = chosen ?: if (caps != null && caps.herdr == null && caps.tmux != null) PickerTab.TMUX else PickerTab.HERDR
    val lists = gate == null
    val shown by rememberUpdatedState(tmuxShown)
    LaunchedEffect(tab, lists) { if (lists && tab == PickerTab.TMUX) shown() }
    // A fixed minimum height keeps the segmented control where the thumb left it when the tab changes.
    val minHeight = (LocalConfiguration.current.screenHeightDp * 0.4f).dp
    Or2Sheet(dismiss, title = null, done = null, scrollable = false) {
        Column(Modifier.imePadding().heightIn(min = minHeight).testTag("session-picker")) {
            if (gate != null) {
                GatePane(gate, gateAction)
                return@Column
            }
            if (title != null) {
                Text(
                    title, style = Or2Type.Secondary, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(start = Or2Dimens.Gutter + 4.dp, end = Or2Dimens.Gutter, bottom = 8.dp).testTag("picker-title"),
                )
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                Segmented(
                    PickerTab.entries.map { it.label }, tab.ordinal, { chosen = PickerTab.entries[it] },
                    tagPrefix = "picker-tab",
                )
                Spacer(Modifier.weight(1f))
                PillButton("Shell", openShell, Modifier.testTag("host-shell"), icon = Or2Icons.Terminal, iconFirst = true)
            }
            if (udpBlocked) {
                // Why this host's terminals use SSH: said once, here, and nowhere on the terminals themselves.
                Text(
                    UDP_BLOCKED_LINE, style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                    modifier = Modifier.padding(start = Or2Dimens.Gutter + 4.dp, end = Or2Dimens.Gutter, top = 8.dp).testTag("picker-udp-blocked"),
                )
            }
            Column(
                Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(Or2Dimens.Gutter)
                    .testTag("picker-list"),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                when (tab) {
                    PickerTab.HERDR -> HerdrList(caps, capsError, herdrViews, open, openHerdr, openAgent)
                    PickerTab.DIRS -> {
                        DirectoryPane(directories, openDirectory)
                        RefreshRow(refreshDirectories, readingDirectories)
                    }
                    PickerTab.TMUX -> {
                        TmuxPane(caps, capsError, tmux, open, openTmux)
                        // Only here: the herdr tab is live already. It re-reads tmux and re-probes (new herdr sessions).
                        RefreshRow(refresh, refreshing)
                    }
                }
                Spacer(Modifier.height(12.dp))
            }
        }
    }
}

/**
 * The picker before its host is connected, laid out like the host's Home card: the host's name, then its
 * progress (mono `accent`, an accent spinner in the icon slot) or why it is not connected (`danger` for a
 * failure, muted otherwise; what each address did in muted mono) with one compact pill to move on.
 */
@Composable
private fun GatePane(gate: PickerGate, act: (GateAction) -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter + 4.dp, vertical = 8.dp).testTag("picker-gate"),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.padding(top = 2.dp).size(Or2Dimens.Spinner), contentAlignment = Alignment.Center) {
            if (gate is PickerGate.Connecting && gate.spinning) {
                Spinner(Modifier.testTag("picker-spinner"), size = Or2Dimens.Spinner)
            } else {
                Icon(Or2Icons.Server, null, Modifier.size(Or2Dimens.Spinner), tint = Or2Colors.TextMuted)
            }
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(gate.host, style = Or2Type.CardTitle, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            when (gate) {
                is PickerGate.Connecting -> Text(
                    gate.progress, style = Or2Type.Mono, color = if (gate.spinning) Or2Colors.Accent else Or2Colors.Attention, maxLines = 1,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }.testTag("picker-progress"),
                )
                is PickerGate.Stopped -> {
                    Text(
                        gate.reason, style = Or2Type.Secondary, color = if (gate.failed) Or2Colors.Danger else Or2Colors.TextMuted,
                        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }.testTag("picker-reason"),
                    )
                    gate.detail?.let {
                        Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.testTag("picker-detail"))
                    }
                    PillButton(
                        gate.action.label, { act(gate.action) }, Modifier.padding(top = 8.dp).testTag("picker-retry"),
                        enabled = gate.enabled, compact = true,
                    )
                }
            }
        }
    }
}

@Composable
private fun Muted(text: String, modifier: Modifier = Modifier, color: Color = Or2Colors.TextMuted) {
    Text(text, style = Or2Type.Body, color = color, modifier = modifier.padding(horizontal = 4.dp, vertical = 6.dp))
}

/** A sheet row: raised so it reads against the sheet; tag on the outer box, the click target inside. */
@Composable
private fun SheetRow(
    tag: String, title: String, subtitle: String?, onClick: () -> Unit, openTag: String, enabled: Boolean = true,
    marker: @Composable (() -> Unit)? = null,
) {
    Box(Modifier.testTag(tag)) {
        ListRow(title, subtitle = subtitle, subtitleMono = true, enabled = enabled, onClick = onClick, trailing = marker,
            modifier = Modifier.testTag(openTag))
    }
}

@Composable
private fun Marker(
    color: Color, text: String, modifier: Modifier = Modifier, pulsing: Boolean = false, textColor: Color = Or2Colors.TextMuted,
) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        StatusDot(color, pulsing = pulsing)
        Spacer(Modifier.width(6.dp))
        Text(text, style = Or2Type.Secondary, color = textColor)
    }
}

/** `● Open`: this session already has a terminal in or2, and choosing it switches there. */
@Composable
private fun OpenMarker(tag: String) = Marker(Or2Colors.Accent, "Open", Modifier.testTag(tag))

/**
 * The herdr tab: each live session (see [pickerHerdrSessions]) as one card: its own row (its name, its agent count),
 * then its agents by workspace; then the sessions without a live view as single rows, in one card.
 */
@Composable
private fun HerdrList(
    caps: HostCapabilities?, capsError: String?, views: Map<String?, HerdrView>, open: OpenSessions,
    choose: (String?) -> Unit, openAgent: (String?, String) -> Unit,
) {
    when {
        // No Refresh on this tab: the tmux tab's re-probes the host.
        capsError != null -> Muted("Could not query the host. Refresh it from the tmux tab.")
        caps == null -> Muted("Checking the host…")
        caps.herdr == null -> Muted("herdr is not installed on this host.", Modifier.testTag("herdr-missing"))
        caps.herdrSessions.isEmpty() -> Muted("No herdr sessions.")
        else -> {
            val sessions = pickerHerdrSessions(caps.herdrSessions, views)
            val (live, plain) = sessions.partition { it.live }
            live.forEachIndexed { index, session ->
                LiveSession(session, open.has(session.info), first = index == 0, { choose(session.session) }) { pane ->
                    openAgent(session.session, pane)
                }
            }
            if (plain.isNotEmpty()) GroupCard(Modifier.padding(top = if (live.isEmpty()) 0.dp else 8.dp), color = Or2Colors.SurfaceRaisedRow) {
                plain.forEachIndexed { index, session ->
                    if (index > 0) GroupDivider()
                    val isOpen = open.has(session.info)
                    SheetRow(
                        "herdr:${session.label}", session.label,
                        // The state is the marker at the right, never a second caption line as well.
                        null,
                        // The default session is opened without a name, never by its listed name.
                        { choose(session.session) }, "herdr-open:${session.label}",
                        enabled = session.running || isOpen,
                        marker = {
                            when {
                                isOpen -> OpenMarker("open-mark:herdr:${session.label}")
                                session.running -> Marker(Or2Colors.Done, "Running")
                                else -> Marker(Or2Colors.Subtle, "Not running")
                            }
                        },
                    )
                }
            }
        }
    }
}

/**
 * A running herdr session with a live view, as one card: the session's own row, named as tmux names its sessions (its
 * name, then how many agents it has, in mono: the session's terminal as it is, `● Open` when it has one), then its
 * agents under small muted workspace headers.
 */
@Composable
private fun LiveSession(session: PickerHerdrSession, isOpen: Boolean, first: Boolean, whole: () -> Unit, agent: (String) -> Unit) {
    Column(Modifier.padding(top = if (first) 0.dp else 8.dp).testTag("herdr-session:${session.label}")) {
        GroupCard(color = Or2Colors.SurfaceRaisedRow) {
            val workspaces = session.workspaces.orEmpty()
            val agents = workspaces.sumOf { it.agents.size }
            SheetRow(
                "herdr:${session.label}", session.label,
                when (agents) {
                    0 -> "no agents"
                    1 -> "1 agent"
                    else -> "$agents agents"
                },
                whole, "herdr-open:${session.label}",
                marker = if (isOpen) ({ OpenMarker("open-mark:herdr:${session.label}") }) else null,
            )
            workspaces.forEach { workspace ->
                GroupDivider()
                workspace.label?.let {
                    Text(
                        it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(start = Or2Dimens.Gutter, end = Or2Dimens.Gutter, top = 8.dp)
                            .testTag("herdr-workspace:${session.label}:$it"),
                    )
                }
                workspace.agents.forEachIndexed { index, item ->
                    if (index > 0) GroupDivider()
                    AgentRow(session.label, item) { agent(item.paneId) }
                }
            }
        }
    }
}

/** An agent: its label, the task it is on and its pane's cwd (muted, the path's end kept), its status dot and word at the right. */
@Composable
private fun AgentRow(session: String, agent: PickerAgent, open: () -> Unit) {
    val blocked = agent.status == AgentStatus.BLOCKED
    Row(
        Modifier.fillMaxWidth().heightIn(min = if (agent.cwd != null) Or2Dimens.RowMinSubtitle else Or2Dimens.RowMin)
            .clickable(role = Role.Button, onClick = open).testTag("herdr-agent:$session:${agent.paneId}")
            .padding(horizontal = Or2Dimens.Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(agent.label, style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            agent.title?.let { AgentTitle(it, Modifier.testTag("herdr-agent-title:$session:${agent.paneId}")) }
            agent.cwd?.let {
                Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.StartEllipsis)
            }
        }
        Spacer(Modifier.width(8.dp))
        Marker(
            statusColor(agent.status), statusLabel(agent.status), Modifier.testTag("herdr-agent-status:$session:${agent.paneId}"),
            pulsing = agent.status == AgentStatus.WORKING, textColor = if (blocked) Or2Colors.Attention else Or2Colors.TextMuted,
        )
    }
}

/** Refresh (the tmux tab's last row), with a spinner beside it while a re-read or a re-probe runs. */
@Composable
private fun RefreshRow(refresh: () -> Unit, refreshing: Boolean) {
    Row(
        // The icon starts at the rows' text inset (their 12 dp padding inside the card, less the glyph's own margin).
        Modifier.clickable(role = Role.Button, onClick = refresh)
            .padding(start = Or2Dimens.Gutter - 2.dp, end = Or2Dimens.Gutter, top = 8.dp, bottom = 8.dp).testTag("host-refresh"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Or2Icons.Refresh, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.TextMuted)
        Spacer(Modifier.width(6.dp))
        Text("Refresh", style = Or2Type.Body, color = Or2Colors.TextMuted)
        if (refreshing) {
            Spacer(Modifier.width(8.dp))
            Spinner(Modifier.testTag("refresh-spinner"), size = 14.dp)
        }
    }
}

@Composable
private fun TmuxPane(caps: HostCapabilities?, capsError: String?, tmux: TmuxList, open: OpenSessions, choose: (String) -> Unit) {
    when {
        capsError != null -> Muted("Could not query the host. Try Refresh.")
        caps == null -> Muted("Checking the host…")
        caps.tmux == null -> Muted("tmux is not installed on this host.", Modifier.testTag("tmux-missing"))
        else -> {
            when (tmux) {
                // Until the first answer: a spinner where the list will be (a later read keeps the list, see RefreshRow).
                TmuxList.Loading -> Box(
                    Modifier.fillMaxWidth().heightIn(min = Or2Dimens.RowMin).padding(horizontal = Or2Dimens.Gutter),
                    contentAlignment = Alignment.CenterStart,
                ) { Spinner(Modifier.testTag("tmux-spinner")) }
                is TmuxList.Failed -> Muted(tmux.message, color = Or2Colors.Danger)
                is TmuxList.Loaded -> {
                    if (tmux.sessions.isEmpty()) Muted("No tmux sessions yet.")
                    else GroupCard(color = Or2Colors.SurfaceRaisedRow) {
                        tmux.sessions.forEachIndexed { index, session ->
                            if (index > 0) GroupDivider()
                            SheetRow(
                                "tmux:${session.name}", session.name,
                                "${session.windows} window${if (session.windows == 1u) "" else "s"}" +
                                    if (session.attachedClients > 0u) " · ${session.attachedClients} attached" else "",
                                { choose(session.name) }, "tmux-attach:${session.name}",
                                marker = when {
                                    open.has(session) -> ({ OpenMarker("open-mark:tmux:${session.name}") })
                                    session.attachedClients > 0u -> ({ Marker(Or2Colors.Attention, "Attached") })
                                    else -> null
                                },
                            )
                        }
                    }
                }
            }
            NewTmuxSession(choose)
        }
    }
}

@Composable
private fun DirectoryPane(directories: DirectoryList, open: (String) -> Unit) {
    when (directories) {
        DirectoryList.Loading -> Spinner(Modifier.testTag("directories-spinner"))
        is DirectoryList.Failed -> Muted(directories.message, Modifier.testTag("directories-error"), color = Or2Colors.Danger)
        is DirectoryList.Loaded -> {
            if (directories.paths.isEmpty()) {
                Muted("No project directories in live herdr panes or Claude Code/Codex history.", Modifier.testTag("directories-empty"))
            } else {
                Muted("Open a shell in a project directory.")
                GroupCard(color = Or2Colors.SurfaceRaisedRow) {
                    directories.paths.forEachIndexed { index, path ->
                        if (index > 0) GroupDivider()
                        SheetRow(
                            "directory:$index", path.trimEnd('/').substringAfterLast('/').ifEmpty { "/" }, path,
                            { open(path) }, "directory-open:$index",
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun NewTmuxSession(open: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    // Show the rule only once something is typed, so the empty field is not an error.
    val error = if (name.isEmpty()) null else tmuxNameError(name)
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Or2Field(name, { name = it }, label = "New session", placeholder = "session-name", errorText = error, tag = "tmux-new-name")
        PillButton("Create and attach", { open(name); name = "" }, Modifier.testTag("tmux-new"), enabled = tmuxNameError(name) == null,
            icon = Or2Icons.Plus)
    }
}

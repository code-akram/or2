package io.github.code_akram.or2.home

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.pair.AddHostChooser
import io.github.code_akram.or2.session.TransportBadge
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.ActionCard
import io.github.code_akram.or2.ui.AttentionCard
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.Or2BottomInsets
import io.github.code_akram.or2.ui.EmptyState
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.StatusChip
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.scrolledUnder

/** An open terminal as the SESSIONS section shows it; [preview] draws its live thumbnail. */
class HomeSession(
    val id: Long, val hostLabel: String, val title: String, val detail: String, val transport: Transport,
    val preview: @Composable (Modifier) -> Unit,
)

/** The "Resume" card: where the user was ([title], over [transport]) when the connection went away. */
class HomeResume(val title: String, val detail: String)

/**
 * The start screen: only trailing icon buttons on top (agents inbox, keys, about), then SESSIONS (live
 * thumbnails of open terminals; tap resumes), CONNECTIONS (host cards: a tap opens the host screen ([openHost]),
 * the `>_` button the session picker over Home ([openSessions]); long press for options)
 * and status chips, and a FAB that adds a host ([addHost] opens the add-host sheet). Without hosts, CONNECTIONS
 * holds the same add-host chooser inline: [easyPair] and [manualHost] are its two cards. Stateless: the caller
 * supplies everything.
 */
@Composable
fun HomeScreen(
    sessions: List<HomeSession>,
    hosts: List<HostCard>,
    keyCount: Int,
    blocked: Int,
    working: Int,
    canConnectAll: Boolean,
    busy: Boolean,
    openSession: (HomeSession) -> Unit,
    openHost: (Host) -> Unit,
    openSessions: (Host) -> Unit,
    addHost: () -> Unit,
    easyPair: () -> Unit,
    manualHost: () -> Unit,
    editHost: (Host) -> Unit,
    connectHost: (Host) -> Unit,
    disconnectHost: (Host) -> Unit,
    deleteHost: (Host) -> Unit,
    openInbox: () -> Unit,
    openKeys: () -> Unit,
    connectAll: () -> Unit,
    modifier: Modifier = Modifier,
    openAbout: () -> Unit = {},
    openSettings: () -> Unit = {},
    resume: HomeResume? = null,
    onResume: () -> Unit = {},
    /** The battery exemption was declined: a small, dismissible card offers it again, blocking nothing. */
    batteryCard: Boolean = false,
    allowBattery: () -> Unit = {},
    dismissBattery: () -> Unit = {},
    /**
     * A host is connected and neither the connection notification nor agent alerts can show (no
     * `POST_NOTIFICATIONS`): a small, dismissible card offers it, in context. Nothing asks for it on connect.
     */
    notificationCard: Boolean = false,
    allowNotifications: () -> Unit = {},
    dismissNotifications: () -> Unit = {},
) {
    var options by remember { mutableStateOf<HostCard?>(null) }
    var deleting by remember { mutableStateOf<Host?>(null) }
    val scroll = rememberScrollState()
    Box(modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            TopBar(scrolled = scroll.scrolledUnder(), actions = {
                Box {
                    IconAction(
                        Or2Icons.Inbox,
                        if (blocked > 0) "Agents inbox, $blocked ${if (blocked == 1) "needs" else "need"} attention" else "Agents inbox",
                        openInbox, Modifier.testTag("nav-inbox"),
                    )
                    if (blocked > 0) {
                        StatusDot(Or2Colors.Attention, Modifier.align(Alignment.TopEnd).padding(top = 9.dp, end = 9.dp).testTag("inbox-badge"))
                    }
                }
                IconAction(Or2Icons.Key, "SSH keys", openKeys, Modifier.testTag("nav-keys"))
                IconAction(Or2Icons.Settings, "Settings", openSettings, Modifier.testTag("nav-settings"))
                IconAction(Or2Icons.Info, "About or2", openAbout, Modifier.testTag("nav-about"))
            })
            Column(Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("home-list")) {
                if (resume != null) {
                    ActionCard("Resume", resume.title, "Unlocks if needed, then returns to this terminal.", meta = resume.detail,
                        icon = Or2Icons.Terminal, onClick = onResume, modifier = Modifier.padding(top = Or2Dimens.Gutter).testTag("home-resume"))
                }
                if (batteryCard) {
                    NoticeCard("Background connections may drop", "battery", allowBattery, dismissBattery, Modifier.padding(top = Or2Dimens.Gutter))
                }
                if (notificationCard) {
                    NoticeCard("Show connection and agent notifications", "notification", allowNotifications, dismissNotifications,
                        Modifier.padding(top = if (batteryCard) 8.dp else Or2Dimens.Gutter))
                }
                if (sessions.isNotEmpty()) {
                    SectionHeader("Sessions", topGap = 8.dp)
                    SessionRow(sessions, openSession)
                }
                SectionHeader("Connections", hint = if (hosts.isNotEmpty()) "Long press for options." else null,
                    topGap = if (sessions.isEmpty()) 16.dp else Or2Dimens.SectionGap)
                if (hosts.isEmpty()) {
                    EmptyConnections(easyPair, manualHost)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        hosts.forEach { card ->
                            HostCardView(card, onClick = { openHost(card.host) }, onLongClick = { options = card },
                                openSessions = { openSessions(card.host) })
                        }
                        if (keyCount == 0) {
                            AttentionCard("Add an SSH key", "Hosts need a key before they can connect.", onClick = openKeys,
                                modifier = Modifier.testTag("home-add-key"))
                        }
                    }
                    Row(Modifier.padding(top = 8.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (blocked > 0) StatusChip("Needs attention: $blocked", Or2Colors.Attention, Modifier.testTag("chip-attention"), onClick = openInbox)
                        if (working > 0) StatusChip("Working: $working", Or2Colors.Working, Modifier.testTag("chip-working"), onClick = openInbox)
                        if (canConnectAll && !busy) StatusChip("Connect all", Or2Colors.Accent, Modifier.testTag("home-connect-all"), onClick = connectAll)
                    }
                }
                Spacer(Modifier.height(Or2Dimens.Fab + 32.dp))
                BottomInsetSpacer()
            }
        }
        Box(
            Modifier.align(Alignment.BottomEnd).windowInsetsPadding(Or2BottomInsets).padding(end = 16.dp, bottom = 8.dp).size(Or2Dimens.Fab).clip(Or2Shapes.Circle)
                .background(Or2Colors.Accent).clickable(role = Role.Button, onClick = addHost)
                .semantics { contentDescription = "Add host" }.testTag("home-add-host"),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Or2Icons.Plus, null, Modifier.size(24.dp), tint = Or2Colors.Background)
        }
    }
    options?.let { card ->
        HostOptionsSheet(
            card, busy, connect = { options = null; connectHost(card.host) }, edit = { options = null; editHost(card.host) },
            disconnect = { options = null; disconnectHost(card.host) }, delete = { options = null; deleting = card.host },
            dismiss = { options = null },
        )
    }
    deleting?.let { host ->
        Or2Dialog(
            onDismiss = { deleting = null }, title = "Delete host?",
            confirm = { TextAction("Delete", { deleteHost(host); deleting = null }, color = Or2Colors.Danger, modifier = Modifier.testTag("delete-confirm")) },
            dismiss = { TextAction("Cancel", { deleting = null }, color = Or2Colors.Text) },
        ) { Text("Delete ${host.label} and its trusted host keys? This does not delete your SSH key.") }
    }
}

/** A host card's long-press options: Connect or Disconnect, Edit and Delete, under the host's name. */
@Composable
fun HostOptionsSheet(
    card: HostCard, busy: Boolean, connect: () -> Unit, edit: () -> Unit, disconnect: () -> Unit, delete: () -> Unit, dismiss: () -> Unit,
) {
    val host = card.host
    Or2Sheet(dismiss, title = host.label, done = "Done", modifier = Modifier.testTag("host-options-sheet")) {
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter)) {
            GroupCard(color = Or2Colors.SurfaceRaisedRow) {
                val live = !card.link.canConnect
                if (!live) {
                    ListRow("Connect", icon = Or2Icons.Power, enabled = !busy && host.keyId != null,
                        subtitle = if (host.keyId == null) "Select a key first" else null,
                        modifier = Modifier.testTag("option-connect"), onClick = connect)
                    GroupDivider(inset = 44.dp)
                }
                ListRow("Edit", icon = Or2Icons.Pencil, modifier = Modifier.testTag("option-edit"), onClick = edit)
                if (live) {
                    GroupDivider(inset = 44.dp)
                    ListRow("Disconnect", icon = Or2Icons.Power, modifier = Modifier.testTag("option-disconnect"), onClick = disconnect)
                }
                GroupDivider(inset = 44.dp)
                ListRow("Delete", icon = Or2Icons.Trash, titleColor = Or2Colors.Danger, enabled = !busy,
                    modifier = Modifier.testTag("option-delete"), onClick = delete)
            }
        }
    }
}

/**
 * A one-line, dismissible offer above SESSIONS: "Background connections may drop" (the battery exemption was
 * declined; Allow opens the system's request) and "Show connection and agent notifications" (Allow asks for the
 * permission, or opens the app's notification settings once Android no longer asks). Tagged `home-<tag>-card`,
 * `<tag>-card-allow` and `<tag>-card-dismiss`.
 */
@Composable
private fun NoticeCard(text: String, tag: String, allow: () -> Unit, dismiss: () -> Unit, modifier: Modifier = Modifier) {
    Or2Card(modifier.fillMaxWidth().testTag("home-$tag-card"), color = Or2Colors.SurfaceRaised) {
        Row(Modifier.padding(start = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Text(text, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.weight(1f).padding(vertical = 8.dp))
            TextAction("Allow", allow, modifier = Modifier.testTag("$tag-card-allow"))
            IconAction(Or2Icons.Close, "Dismiss", dismiss, Modifier.testTag("$tag-card-dismiss"), tint = Or2Colors.TextMuted)
        }
    }
}

/**
 * No hosts yet: the illustration, then the add-host chooser the "+" sheet shows too. Both of its paths can make the
 * key on the phone, so there is no separate key step.
 */
@Composable
private fun EmptyConnections(easyPair: () -> Unit, manualHost: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(top = 24.dp).testTag("home-empty"), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        EmptyState(
            Or2Icons.Server, "No connections yet",
            "Add a host to attach to its tmux sessions\nand watch its herdr agents here.",
        )
        AddHostChooser(easyPair, manualHost, tagPrefix = "home-add-host")
    }
}

/**
 * A server icon with its status dot, name, mono `user@host:port` (or progress, or the failure), and the session
 * button at the end. The card body ([onClick]) opens the host screen; the button ([openSessions]) opens the
 * session picker over Home.
 */
@Composable
fun HostCardView(card: HostCard, onClick: () -> Unit, onLongClick: () -> Unit, openSessions: () -> Unit, modifier: Modifier = Modifier) {
    val host = card.host
    val status = card.status
    // The dot's meaning is said as well as coloured: the merged card reads "Connected" after the name.
    Or2Card(
        modifier.testTag("host:${host.id}").semantics { stateDescription = hostStateDescription(status) },
        onClick = onClick, onLongClick = onLongClick,
    ) {
        // No vertical padding on the row: the text column carries it, so the 48 dp button fits the card's own height.
        Row(Modifier.padding(start = 14.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(Or2Dimens.Spinner), contentAlignment = Alignment.Center) {
                if (status.spinning) {
                    // The same 24 dp slot as the server icon, so the glyph does not jump between states.
                    Spinner(Modifier.testTag("host-spinner:${host.id}"), size = Or2Dimens.Spinner)
                } else {
                    Icon(Or2Icons.Server, null, Modifier.size(Or2Dimens.Spinner), tint = Or2Colors.TextMuted)
                    dotColor(status.dot)?.let {
                        StatusDot(it, Modifier.align(Alignment.TopEnd).offset(x = 3.dp, y = (-3).dp).testTag("host-dot:${host.id}"))
                    }
                }
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f).padding(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(host.label, style = Or2Type.CardTitle, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                when {
                    status.progress != null ->
                        Text(status.progress, style = Or2Type.Mono, color = Or2Colors.Accent, maxLines = 1, modifier = Modifier.testTag("host-progress:${host.id}"))
                    status.failure != null ->
                        Text(status.failure, style = Or2Type.Secondary, color = Or2Colors.Danger, maxLines = 2, modifier = Modifier.testTag("host-failure:${host.id}"))
                    status.asleep ->
                        Text("Asleep", style = Or2Type.Secondary, color = Or2Colors.TextMuted, maxLines = 1, modifier = Modifier.testTag("host-asleep:${host.id}"))
                    else ->
                        Text(hostAddressLine(host), style = Or2Type.Mono, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                status.detail?.let {
                    Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 4, modifier = Modifier.testTag("host-detail-lines:${host.id}"))
                }
            }
            Spacer(Modifier.width(2.dp))
            SessionButton(host, openSessions)
        }
    }
}

/**
 * The card's session button: the `>_` prompt glyph in `accent` on a 32 dp `accentMuted` disc, centred in a 48 dp
 * touch box. Its own click target inside the card, so a tap on it never opens the host screen.
 */
@Composable
private fun SessionButton(host: Host, onClick: () -> Unit) {
    Box(
        Modifier.size(48.dp).clip(Or2Shapes.Circle).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = "Open a session on ${host.label}" }.testTag("host-session:${host.id}"),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(32.dp).clip(Or2Shapes.Circle).background(Or2Colors.AccentMuted), contentAlignment = Alignment.Center) {
            Icon(Or2Icons.Terminal, null, Modifier.size(18.dp), tint = Or2Colors.Accent)
        }
    }
}

private fun hostStateDescription(status: HostCardStatus): String = when {
    status.progress != null -> status.progress
    status.failure != null -> "Connection failed"
    status.asleep -> "Asleep"
    else -> when (status.dot) {
        HostDot.NONE -> "Not connected"
        HostDot.CONNECTED -> "Connected"
        HostDot.ATTENTION -> "Needs attention"
        HostDot.CONNECTING -> "Connecting"
        HostDot.FAILED -> "Connection failed"
    }
}

private fun dotColor(dot: HostDot): Color? = when (dot) {
    HostDot.NONE -> null
    HostDot.CONNECTED -> Or2Colors.Done
    HostDot.ATTENTION -> Or2Colors.Attention
    HostDot.CONNECTING -> Or2Colors.Accent
    HostDot.FAILED -> Or2Colors.Danger
}

/** Thumbnails about 38 % of the width each, scrolling sideways. */
@Composable
private fun SessionRow(sessions: List<HomeSession>, open: (HomeSession) -> Unit) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val cardWidth = maxWidth * 0.38f
        Row(Modifier.horizontalScroll(rememberScrollState()).testTag("sessions-row"), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            sessions.forEach { SessionCard(it, cardWidth) { open(it) } }
        }
    }
}

@Composable
private fun SessionCard(session: HomeSession, width: Dp, onClick: () -> Unit) {
    Column(Modifier.width(width).testTag("session-card:${session.id}")) {
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f).clip(Or2Shapes.Thumbnail).background(Or2Colors.TerminalBackground)
                .clickable(role = Role.Button, onClickLabel = "Resume ${session.title}", onClick = onClick),
        ) {
            // Inset like Moshi's thumbnails, so the rounded corners never slice glyphs or the cursor.
            session.preview(Modifier.fillMaxSize().padding(start = 6.dp, end = 6.dp, top = 26.dp, bottom = 6.dp))
            Row(
                Modifier.fillMaxWidth().padding(6.dp), verticalAlignment = Alignment.Top,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    session.hostLabel, style = Or2Type.Pill, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false).clip(Or2Shapes.Pill).background(Or2Colors.ToolbarPill)
                        .padding(horizontal = 6.dp, vertical = 2.dp).testTag("session-host:${session.id}"),
                )
                Spacer(Modifier.width(4.dp))
                TransportBadge(session.transport, Modifier.testTag("session-transport:${session.id}"), small = true)
            }
        }
        Text(session.title, style = Or2Type.Body, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 6.dp, start = 4.dp))
        Text(session.detail, style = Or2Type.MonoSmall, color = Or2Colors.Accent, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 4.dp))
    }
}

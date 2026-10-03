package io.github.code_akram.or2.home

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.draw.alpha
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
import io.github.code_akram.or2.pair.AddHostChooser
import io.github.code_akram.or2.session.CloseShellDialog
import io.github.code_akram.or2.session.TransportBadge
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.ActionCard
import io.github.code_akram.or2.ui.AttentionCard
import io.github.code_akram.or2.ui.Badge
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.EmptyState
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2BottomInsets
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
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.scrolledUnder

/**
 * An open terminal as its host's card shows it: [detail] is the line under its title ([sessionDetail], possibly
 * empty), [closed] marks one that has closed (its final frame stays until it is closed here), [closeAsks] whether its
 * `×` asks first (an open shell: `closeAsks`), and [preview] draws its live thumbnail.
 */
class HomeSession(
    val id: Long, val title: String, val detail: String, val transport: Transport, val closed: Boolean = false,
    val closeAsks: Boolean = false, val preview: @Composable (Modifier) -> Unit,
)

/** The "Resume" card: where the user was ([title], over [transport]) when the connection went away. */
class HomeResume(val title: String, val detail: String)

/**
 * The start screen and the one place for hosts and their terminals: only trailing icon buttons on top (agents inbox,
 * keys, settings, about), the notices and the Resume card, then HOSTS (with `Connect all` at the header's end when
 * more than one host can connect) and a FAB that adds a host ([addHost] opens the add-host sheet).
 *
 * Each host card: a header (status, name, address or progress or failure) whose tap opens the session picker over
 * Home ([openPicker]; long press is the menu too), a trailing `⋯` for the host menu (Connect or Disconnect, Edit,
 * Delete with its confirm), and below, when the host has open terminals, their live thumbnails in a row: a tap shows
 * that terminal ([openSession]), its `×` closes it ([closeSession], after [CloseShellDialog] for an open shell).
 * Without hosts, HOSTS holds the add-host chooser inline: [easyPair] and [manualHost] are its two cards. Stateless:
 * the caller supplies everything.
 */
@Composable
fun HomeScreen(
    hosts: List<HostCard>,
    keyCount: Int,
    blocked: Int,
    canConnectAll: Boolean,
    busy: Boolean,
    openPicker: (Host) -> Unit,
    openSession: (HomeSession) -> Unit,
    closeSession: (HomeSession) -> Unit,
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
    var closing by remember { mutableStateOf<HomeSession?>(null) }
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
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
                    SectionHeader("Hosts", Modifier.weight(1f), topGap = 16.dp)
                    if (hosts.isNotEmpty() && canConnectAll && !busy) {
                        // A compact text action on the header's line (its touch target grows to 48 dp).
                        Text(
                            "Connect all", style = Or2Type.Chip, color = Or2Colors.Accent, maxLines = 1,
                            modifier = Modifier.padding(bottom = 2.dp).clip(Or2Shapes.Pill)
                                .clickable(role = Role.Button, onClick = connectAll)
                                .padding(horizontal = 8.dp, vertical = 4.dp).testTag("home-connect-all"),
                        )
                    }
                }
                if (hosts.isEmpty()) {
                    EmptyHosts(easyPair, manualHost)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        hosts.forEach { card ->
                            HostCardView(
                                card, openPicker = { openPicker(card.host) }, openMenu = { options = card },
                                openSession = openSession,
                                closeSession = { session -> if (session.closeAsks) closing = session else closeSession(session) },
                            )
                        }
                        if (keyCount == 0) {
                            AttentionCard("Add an SSH key", "Hosts need a key before they can connect.", onClick = openKeys,
                                modifier = Modifier.testTag("home-add-key"))
                        }
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
        DeleteHostDialog(host, delete = { deleteHost(host); deleting = null }, dismiss = { deleting = null })
    }
    closing?.let { session ->
        CloseShellDialog(close = { closing = null; closeSession(session) }, dismiss = { closing = null })
    }
}

/**
 * "Delete host?": the one confirmation of deleting a host, from Home's host menu and from the host form. It says what
 * goes (the host and its trusted host keys) and what stays (the SSH key).
 */
@Composable
fun DeleteHostDialog(host: Host, delete: () -> Unit, dismiss: () -> Unit) {
    Or2Dialog(
        onDismiss = dismiss, title = "Delete host?",
        confirm = { TextAction("Delete", delete, color = Or2Colors.Danger, modifier = Modifier.testTag("delete-confirm")) },
        dismiss = { TextAction("Cancel", dismiss, color = Or2Colors.Text) },
    ) { Text("Delete ${host.label} and its trusted host keys? This does not delete your SSH key.") }
}

/** A host card's menu (its `⋯`, or a long press): Connect or Disconnect, Edit and Delete, under the host's name. */
@Composable
fun HostOptionsSheet(
    card: HostCard, busy: Boolean, connect: () -> Unit, edit: () -> Unit, disconnect: () -> Unit, delete: () -> Unit, dismiss: () -> Unit,
) {
    val host = card.host
    Or2Sheet(dismiss, title = host.label, done = "Done", modifier = Modifier.testTag("host-options-sheet")) {
        Column {
            GroupCard {
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
 * A one-line, dismissible offer above HOSTS: "Background connections may drop" (the battery exemption was
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
private fun EmptyHosts(easyPair: () -> Unit, manualHost: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(top = 24.dp).testTag("home-empty"), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        EmptyState(
            Or2Icons.Server, "No hosts yet",
            "Add a host to attach to its tmux sessions\nand watch its herdr agents here.",
        )
        AddHostChooser(easyPair, manualHost, tagPrefix = "home-add-host")
    }
}

/**
 * One host: a header row (the server icon with its status dot, the name, the mono `user@host:port` in use or the
 * progress or failure in its place, and a trailing `⋯`) and, when the host has open terminals, their thumbnails in a
 * row below. The header ([openPicker], tagged `host:<id>`) opens the session picker; `⋯` ([openMenu], also a long
 * press on the header) the host menu.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun HostCardView(
    card: HostCard, openPicker: () -> Unit, openMenu: () -> Unit, openSession: (HomeSession) -> Unit,
    closeSession: (HomeSession) -> Unit, modifier: Modifier = Modifier,
) {
    val host = card.host
    val status = card.status
    Or2Card(modifier.testTag("host-card:${host.id}")) {
        // No vertical padding on the row: the text column carries it, so the 44 dp menu button fits the header's height.
        // The dot's meaning is said as well as coloured: the merged header reads "Connected" after the name.
        Row(
            Modifier.fillMaxWidth()
                .combinedClickable(role = Role.Button, onClickLabel = "Open a session", onClick = openPicker, onLongClick = openMenu)
                .semantics { stateDescription = hostStateDescription(status) }.testTag("host:${host.id}")
                .padding(start = 14.dp, end = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
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
                        Text(card.address, style = Or2Type.Mono, color = Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                status.detail?.let {
                    Text(it, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 4, modifier = Modifier.testTag("host-detail-lines:${host.id}"))
                }
            }
            IconAction(Or2Icons.More, "Options for ${host.label}", openMenu, Modifier.testTag("host-menu:${host.id}"), tint = Or2Colors.TextMuted)
        }
        if (card.terminals.isNotEmpty()) TerminalRow(host.id, card.terminals, openSession, closeSession)
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

/** A host's terminals inside its card: thumbnails about 38 % of the card's width each, scrolling sideways. */
@Composable
private fun TerminalRow(hostId: Long, sessions: List<HomeSession>, open: (HomeSession) -> Unit, close: (HomeSession) -> Unit) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val cardWidth = maxWidth * 0.38f
        Row(
            Modifier.horizontalScroll(rememberScrollState()).padding(start = Or2Dimens.Gutter, end = Or2Dimens.Gutter, bottom = Or2Dimens.Gutter)
                .testTag("host-terminals:$hostId"),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            sessions.forEach { session -> SessionCard(session, cardWidth, { open(session) }, { close(session) }) }
        }
    }
}

/**
 * One terminal's thumbnail: the live preview on the terminal's background with the transport pill (or `Closed`) at
 * the top left and a small `×` (a 24 dp disc in a 40 dp touch box) at the top right, then the title and the detail.
 */
@Composable
private fun SessionCard(session: HomeSession, width: Dp, onClick: () -> Unit, onClose: () -> Unit) {
    Column(Modifier.width(width).testTag("session-card:${session.id}")) {
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f).clip(Or2Shapes.Thumbnail).background(Or2Colors.TerminalBackground)
                .clickable(role = Role.Button, onClickLabel = "Show ${session.title}", onClick = onClick),
        ) {
            // Inset like Moshi's thumbnails, so the rounded corners never slice glyphs or the cursor.
            session.preview(
                Modifier.fillMaxSize().padding(start = 6.dp, end = 6.dp, top = 26.dp, bottom = 6.dp)
                    .then(if (session.closed) Modifier.alpha(0.5f) else Modifier),
            )
            if (session.closed) {
                Badge("Closed", Or2Colors.SurfaceTrack, Or2Colors.TextMuted,
                    Modifier.align(Alignment.TopStart).padding(6.dp).testTag("session-closed:${session.id}"))
            } else {
                TransportBadge(session.transport, Modifier.align(Alignment.TopStart).padding(6.dp).testTag("session-transport:${session.id}"))
            }
            Box(
                Modifier.align(Alignment.TopEnd).size(40.dp).clip(Or2Shapes.Circle)
                    .clickable(role = Role.Button, onClick = onClose)
                    .semantics { contentDescription = "Close ${session.title}" }.testTag("session-close:${session.id}"),
                contentAlignment = Alignment.Center,
            ) {
                Box(Modifier.size(24.dp).clip(Or2Shapes.Circle).background(Or2Colors.ToolbarPill), contentAlignment = Alignment.Center) {
                    Icon(Or2Icons.Close, null, Modifier.size(14.dp), tint = Or2Colors.Text)
                }
            }
        }
        Text(session.title, style = Or2Type.Body, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 6.dp, start = 4.dp))
        Text(session.detail, style = Or2Type.MonoSmall, color = Or2Colors.Accent, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 4.dp))
    }
}

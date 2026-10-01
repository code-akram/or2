package io.github.code_akram.or2.host

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
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
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.hostStateMessage
import io.github.code_akram.or2.ui.AttentionCard
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.Segmented
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TopBar

/** The tmux session list of a connected host as the screen last learned it. */
sealed interface TmuxList {
    data object Loading : TmuxList
    data class Loaded(val sessions: List<TmuxSession>) : TmuxList
    data class Failed(val message: String) : TmuxList
}

/** Mirrors the Rust rules: nonempty, at most 128 bytes, no control characters, `\`, `:` or `.`. */
fun tmuxNameError(name: String): String? = when {
    name.isEmpty() -> "Enter a session name."
    name.toByteArray(Charsets.UTF_8).size > 128 -> "Use at most 128 bytes."
    name.any { it.isISOControl() } -> "Remove control characters."
    name.any { it == '\\' || it == ':' || it == '.' } -> "Remove backslash, colon and dot characters."
    else -> null
}

/** An open terminal on this host, for resuming from the host screen. */
data class HostTerminalItem(val id: Long, val title: String, val closed: Boolean)

/**
 * One host: its connection and, once connected, the session picker sheet (herdr, tmux or a
 * recent session, or "Skip" for a plain shell). Stateless: the caller owns the connection and
 * supplies what it knows. The sheet opens by itself when the host connects, once per connection:
 * [pickerOffered] (kept by the caller, so it survives leaving and re-entering this screen, e.g. Back
 * from a terminal) says it already did, and [setPickerOffered] records it, or resets it when the
 * connection ends.
 */
@Composable
fun HostScreen(
    host: Host,
    hostState: HostState?,
    caps: HostCapabilities?,
    capsError: String?,
    tmux: TmuxList,
    busy: Boolean,
    connect: () -> Unit,
    disconnect: () -> Unit,
    approve: (HostState.AwaitingHostKeyDecision) -> Unit,
    reject: () -> Unit,
    openShell: () -> Unit,
    openTmux: (String) -> Unit,
    openHerdr: (session: String?) -> Unit,
    refresh: () -> Unit,
    modifier: Modifier = Modifier,
    terminals: List<HostTerminalItem> = emptyList(),
    resume: (Long) -> Unit = {},
    back: () -> Unit = {},
    edit: () -> Unit = {},
    pickerOffered: Boolean = false,
    setPickerOffered: (Boolean) -> Unit = {},
) {
    val link = linkStatus(hostState, host.sleeps)
    var pickerOpen by rememberSaveable { mutableStateOf(false) }
    val offered by rememberUpdatedState(pickerOffered)
    LaunchedEffect(link) {
        // The picker opens when the host becomes connected, and stays closed once dismissed or
        // once the user has been through it (Back from a terminal must not cover the page again).
        if (link == LinkStatus.CONNECTED) {
            if (!offered) {
                pickerOpen = true
                setPickerOffered(true)
            }
        } else {
            setPickerOffered(false)
        }
    }
    Column(modifier.fillMaxSize()) {
        TopBar(title = host.label, back = back, actions = {
            IconAction(Or2Icons.Pencil, "Edit host", edit, Modifier.testTag("host-edit"), enabled = !busy)
        })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("host-detail"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            StatusCard(host, hostState, link)
            if (host.keyId == null) {
                AttentionCard("Select a key", "Edit this host and choose the SSH key it signs in with.", onClick = edit)
            }
            when (link) {
                LinkStatus.NOT_CONNECTED, LinkStatus.FAILED, LinkStatus.ASLEEP ->
                    PrimaryButton(if (host.keyId == null) "Select a key first" else "Unlock and connect", connect,
                        Modifier.testTag("host-connect"), enabled = !busy && host.keyId != null)
                LinkStatus.CONNECTED -> {
                    PrimaryButton("Open a session", { pickerOpen = true }, Modifier.testTag("host-open-picker"))
                    GroupCard {
                        ListRow("Disconnect", icon = Or2Icons.Power, modifier = Modifier.testTag("host-disconnect"), onClick = disconnect)
                    }
                }
                else -> PillButton("Cancel", disconnect, Modifier.testTag("host-disconnect"))
            }
            if (terminals.isNotEmpty()) {
                Column(Modifier.testTag("open-terminals")) {
                    SectionHeader("Open sessions", topGap = 6.dp)
                    GroupCard {
                        terminals.forEachIndexed { index, terminal ->
                            if (index > 0) GroupDivider()
                            ListRow(
                                terminal.title + if (terminal.closed) " (closed)" else "", chevron = true,
                                modifier = Modifier.testTag("open-terminal:${terminal.id}"), onClick = { resume(terminal.id) },
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
    if (link == LinkStatus.CONNECTED && pickerOpen) {
        SessionPickerSheet(
            caps, capsError, tmux, terminals, openShell = { pickerOpen = false; openShell() },
            openTmux = { pickerOpen = false; openTmux(it) }, openHerdr = { pickerOpen = false; openHerdr(it) },
            resume = { pickerOpen = false; resume(it) }, refresh = refresh, dismiss = { pickerOpen = false },
        )
    }
    (hostState as? HostState.AwaitingHostKeyDecision)?.let { prompt ->
        HostTrustDialog(prompt, busy, { approve(prompt) }, reject)
    }
}

@Composable
private fun StatusCard(host: Host, hostState: HostState?, link: LinkStatus) {
    val message = if (link == LinkStatus.ASLEEP) LinkStatus.ASLEEP.label else hostState?.let(::hostStateMessage) ?: LinkStatus.NOT_CONNECTED.label
    val color = when (link) {
        LinkStatus.FAILED -> Or2Colors.Danger
        LinkStatus.CONNECTED -> Or2Colors.Done
        LinkStatus.NEEDS_HOST_KEY -> Or2Colors.Attention
        LinkStatus.CONNECTING -> Or2Colors.Accent
        LinkStatus.NOT_CONNECTED, LinkStatus.ASLEEP -> Or2Colors.TextMuted
    }
    Or2Card {
        Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(Or2Dimens.IconTile), contentAlignment = Alignment.Center) {
                if (link == LinkStatus.CONNECTING) {
                    Spinner(size = Or2Dimens.Spinner)
                } else {
                    Icon(Or2Icons.Server, null, Modifier.size(Or2Dimens.Spinner), tint = Or2Colors.TextMuted)
                }
            }
            Spacer(Modifier.width(8.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(host.username + "@" + host.addressSummary, style = Or2Type.Mono, color = Or2Colors.TextMuted)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (link != LinkStatus.CONNECTING) {
                        StatusDot(color, Modifier.padding(end = 6.dp))
                    }
                    Text(message, style = Or2Type.Body, color = if (link == LinkStatus.FAILED) Or2Colors.Danger else Or2Colors.Text,
                        modifier = Modifier.testTag("host-state"))
                }
                (hostState as? HostState.Connected)?.let { connected ->
                    if (host.addresses.size > 1) {
                        Text(
                            "Using address ${connected.addressIndex + 1u}: " +
                                host.addresses.getOrNull(connected.addressIndex.toInt())?.let { "${it.hostname}:${it.port}" }.orEmpty(),
                            style = Or2Type.MonoSmall, color = Or2Colors.TextMuted,
                        )
                    }
                }
            }
        }
    }
}

enum class PickerTab(val label: String) { HERDR("herdr"), TMUX("tmux"), RECENT("Recent") }

/**
 * The session picker: a segmented control (herdr, tmux, Recent) with a "Skip" pill that opens a
 * plain shell, and one grouped list below. Hosts without tmux or herdr, failed listings and
 * errors are explained in muted text, never hidden.
 */
@Composable
fun SessionPickerSheet(
    caps: HostCapabilities?,
    capsError: String?,
    tmux: TmuxList,
    recent: List<HostTerminalItem>,
    openShell: () -> Unit,
    openTmux: (String) -> Unit,
    openHerdr: (session: String?) -> Unit,
    resume: (Long) -> Unit,
    refresh: () -> Unit,
    dismiss: () -> Unit,
    initialTab: PickerTab? = null,
) {
    var chosen by remember { mutableStateOf(initialTab) }
    val tab = chosen ?: if (caps != null && caps.herdr == null && caps.tmux != null) PickerTab.TMUX else PickerTab.HERDR
    // A fixed minimum height keeps the segmented control where the thumb left it when the tab changes.
    val minHeight = (LocalConfiguration.current.screenHeightDp * 0.4f).dp
    Or2Sheet(dismiss, title = null, done = null) {
        Column(Modifier.imePadding().heightIn(min = minHeight).testTag("session-picker")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                Segmented(
                    PickerTab.entries.map { it.label }, tab.ordinal, { chosen = PickerTab.entries[it] },
                    tagPrefix = "picker-tab",
                )
                Spacer(Modifier.weight(1f))
                PillButton("Skip", openShell, Modifier.testTag("host-shell"), icon = Or2Icons.Skip)
            }
            Column(
                Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(Or2Dimens.Gutter)
                    .testTag("picker-list"),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                when (tab) {
                    PickerTab.HERDR -> HerdrList(caps, capsError, openHerdr)
                    PickerTab.TMUX -> TmuxPane(caps, capsError, tmux, openTmux)
                    PickerTab.RECENT -> RecentList(recent, resume)
                }
                if (tab != PickerTab.RECENT) {
                    Row(
                        // The icon starts at the rows' text inset (their 12 dp padding inside the card, less the glyph's own margin).
                        Modifier.clickable(role = Role.Button, onClick = refresh)
                            .padding(start = Or2Dimens.Gutter - 2.dp, end = Or2Dimens.Gutter, top = 8.dp, bottom = 8.dp).testTag("host-refresh"),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Or2Icons.Refresh, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.TextMuted)
                        Spacer(Modifier.width(6.dp))
                        Text("Refresh", style = Or2Type.Body, color = Or2Colors.TextMuted)
                    }
                }
                Spacer(Modifier.height(12.dp))
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
private fun Marker(color: Color, text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        StatusDot(color)
        Spacer(Modifier.width(6.dp))
        Text(text, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
    }
}

@Composable
private fun HerdrList(caps: HostCapabilities?, capsError: String?, open: (String?) -> Unit) {
    when {
        capsError != null -> Muted("Could not query the host. Try Refresh.")
        caps == null -> Muted("Checking the host…")
        caps.herdr == null -> Muted("herdr is not installed on this host.", Modifier.testTag("herdr-missing"))
        caps.herdrSessions.isEmpty() -> Muted("No herdr sessions.")
        else -> GroupCard(color = Or2Colors.SurfaceRaisedRow) {
            caps.herdrSessions.forEachIndexed { index, session ->
                if (index > 0) GroupDivider()
                SheetRow(
                    "herdr:${session.name}", session.name + if (session.isDefault) " (default)" else "",
                    // The state is the marker at the right, never a second caption line as well.
                    null,
                    // The default session is opened without a name, never by its listed name.
                    { open(if (session.isDefault) null else session.name) }, "herdr-open:${session.name}", enabled = session.running,
                    marker = { if (session.running) Marker(Or2Colors.Done, "Running") else Marker(Or2Colors.Subtle, "Not running") },
                )
            }
        }
    }
}

@Composable
private fun TmuxPane(caps: HostCapabilities?, capsError: String?, tmux: TmuxList, open: (String) -> Unit) {
    when {
        capsError != null -> Muted("Could not query the host. Try Refresh.")
        caps == null -> Muted("Checking the host…")
        caps.tmux == null -> Muted("tmux is not installed on this host.", Modifier.testTag("tmux-missing"))
        else -> {
            when (tmux) {
                TmuxList.Loading -> Muted("Loading sessions…")
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
                                { open(session.name) }, "tmux-attach:${session.name}",
                                marker = if (session.attachedClients > 0u) ({ Marker(Or2Colors.Attention, "Attached") }) else null,
                            )
                        }
                    }
                }
            }
            NewTmuxSession(open)
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

@Composable
private fun RecentList(recent: List<HostTerminalItem>, resume: (Long) -> Unit) {
    if (recent.isEmpty()) {
        Muted("No recent sessions yet. Sessions you open on this host appear here until you close them.", Modifier.testTag("recent-empty"))
        return
    }
    GroupCard(color = Or2Colors.SurfaceRaisedRow) {
        recent.forEachIndexed { index, terminal ->
            if (index > 0) GroupDivider()
            SheetRow(
                "recent:${terminal.id}", terminal.title, if (terminal.closed) "closed" else null, { resume(terminal.id) }, "recent-open:${terminal.id}",
                marker = if (terminal.closed) null else ({ Marker(Or2Colors.Accent, "Open") }),
            )
        }
    }
}

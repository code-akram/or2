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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
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

/** Mirrors the Rust rules: nonempty, at most 128 bytes, no control characters, `\`, `:` or `.`. */
fun tmuxNameError(name: String): String? = when {
    name.isEmpty() -> "Enter a session name."
    name.toByteArray(Charsets.UTF_8).size > 128 -> "Use at most 128 bytes."
    name.any { it.isISOControl() } -> "Remove control characters."
    name.any { it == '\\' || it == ':' || it == '.' } -> "Remove backslash, colon and dot characters."
    else -> null
}

enum class PickerTab(val label: String) { HERDR("herdr"), TMUX("tmux") }

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
 * The session picker over Home (a host card's header opens it): a segmented control (herdr, tmux) with a "Shell" pill
 * (the `>_` glyph) that opens a plain shell, and one grouped list below. A session that already has an open terminal
 * is marked `● Open` ([open]): choosing it switches to that terminal. Hosts without tmux or herdr, failed listings and
 * errors are explained in muted text, never hidden; so is mosh's UDP being blocked ([udpBlocked]), under the tabs.
 * While [gate] is set (the host is not connected yet) the sheet shows it instead: the host's progress, or why it is
 * not connected with [gateAction]'s pill; the lists follow in the same sheet once the gate is null.
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
) {
    var chosen by remember { mutableStateOf(initialTab) }
    val tab = chosen ?: if (caps != null && caps.herdr == null && caps.tmux != null) PickerTab.TMUX else PickerTab.HERDR
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
                    PickerTab.HERDR -> HerdrList(caps, capsError, open, openHerdr)
                    PickerTab.TMUX -> TmuxPane(caps, capsError, tmux, open, openTmux)
                }
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
private fun Marker(color: Color, text: String, modifier: Modifier = Modifier) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        StatusDot(color)
        Spacer(Modifier.width(6.dp))
        Text(text, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
    }
}

/** `● Open`: this session already has a terminal in or2, and choosing it switches there. */
@Composable
private fun OpenMarker(tag: String) = Marker(Or2Colors.Accent, "Open", Modifier.testTag(tag))

@Composable
private fun HerdrList(caps: HostCapabilities?, capsError: String?, open: OpenSessions, choose: (String?) -> Unit) {
    when {
        capsError != null -> Muted("Could not query the host. Try Refresh.")
        caps == null -> Muted("Checking the host…")
        caps.herdr == null -> Muted("herdr is not installed on this host.", Modifier.testTag("herdr-missing"))
        caps.herdrSessions.isEmpty() -> Muted("No herdr sessions.")
        else -> GroupCard(color = Or2Colors.SurfaceRaisedRow) {
            caps.herdrSessions.forEachIndexed { index, session ->
                if (index > 0) GroupDivider()
                val isOpen = open.has(session)
                SheetRow(
                    "herdr:${session.name}", session.name + if (session.isDefault) " (default)" else "",
                    // The state is the marker at the right, never a second caption line as well.
                    null,
                    // The default session is opened without a name, never by its listed name.
                    { choose(if (session.isDefault) null else session.name) }, "herdr-open:${session.name}",
                    enabled = session.running || isOpen,
                    marker = {
                        when {
                            isOpen -> OpenMarker("open-mark:herdr:${session.name}")
                            session.running -> Marker(Or2Colors.Done, "Running")
                            else -> Marker(Or2Colors.Subtle, "Not running")
                        }
                    },
                )
            }
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

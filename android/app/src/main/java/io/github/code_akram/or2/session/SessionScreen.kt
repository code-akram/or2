package io.github.code_akram.or2.session

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.linkStaleLabel
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.terminal.TerminalScreen
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.terminal.display
import io.github.code_akram.or2.terminal.swipeNav
import io.github.code_akram.or2.ui.Badge
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.TopBar
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** The mono header title: `host: target`. */
fun terminalTitle(terminal: ActiveTerminal) = terminal.host.label + ": " + terminal.title

/**
 * One terminal session in a full-height card. Minimising ([minimise]: the round button, or a
 * drag down on the handle or header) returns to Home and never disconnects: the session keeps
 * running and stays listed as a thumbnail until the user closes it. The display lease keeps the
 * native handle alive while this screen shows it.
 */
@Composable
fun SessionScreen(
    holder: HostConnections,
    terminal: ActiveTerminal?,
    open: List<ActiveTerminal>,
    minimise: () -> Unit,
    select: (ActiveTerminal) -> Unit,
) {
    if (terminal == null) {
        Column(Modifier.fillMaxSize()) {
            TopBar(back = minimise)
            Text("This terminal is no longer open.", style = Or2Type.Body, color = Or2Colors.TextMuted,
                modifier = Modifier.padding(Or2Dimens.Gutter))
        }
        return
    }
    key(terminal) {
        DisposableEffect(terminal) {
            holder.attachDisplay(terminal)
            onDispose { holder.detachDisplay(terminal) }
        }
        val state by terminal.state.collectAsStateWithLifecycle()
        val handle by terminal.handle.collectAsStateWithLifecycle()
        val hasConnected by terminal.hasConnected.collectAsStateWithLifecycle()
        val transport by terminal.transport.collectAsStateWithLifecycle()
        val linkHealth by terminal.linkHealth.collectAsStateWithLifecycle()
        val closed = state is SessionState.Closed
        val endSession = { if (closed) { holder.dismissTerminal(terminal); minimise() } else holder.disconnectTerminal(terminal) }
        var switcher by remember { mutableStateOf(false) }
        val scope = rememberCoroutineScope()
        val haptics = LocalHapticFeedback.current
        // The card follows the terminal's own background, which the remote can change (OSC 11).
        var background by remember { mutableStateOf(Or2Colors.TerminalBackground) }
        if (hasConnected) {
            TerminalCard(terminalTitle(terminal), transport.display(), state, minimise, openSwitcher = { switcher = true }, endSession,
                background = background, linkHealth = linkHealth) {
                // Keep the borrowed handle composed through Closed so its final frame stays visible.
                handle?.let { TerminalScreen(it, terminal.state, terminal.frameReady, Modifier.weight(1f),
                    composerHint = "Message " + terminal.host.label + "…", openPanes = { switcher = true },
                    onBackground = { background = it }, onFrameDrawn = { holder.timing.terminalFrame(terminal.id) },
                    target = terminal.target, scrollTarget = { holder.scrollTarget(terminal, it) },
                    // Swipes move tmux or herdr; a shell has nothing to move and keeps every touch.
                    onSwipe = if (terminal.target is TerminalTarget.Shell) null else { swipe ->
                        haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
                        scope.launch { holder.navigate(terminal, swipeNav(swipe)) }
                    },
                    switchTo = { index -> open.getOrNull(index)?.let { if (it !== terminal) select(it) } },
                    closeTerminal = { holder.dismissTerminal(terminal); minimise() }) }
            }
        } else {
            PendingTerminal(terminal, state, closed, endSession, minimise, open, select)
        }
        if (switcher) {
            SessionSwitcher(terminal, open, closed, select = { switcher = false; select(it) }, endSession = { switcher = false; endSession() },
                dismiss = { switcher = false })
        }
    }
}

/**
 * The full-height card: drag handle, header row (mono [title], [transport] badge), and the terminal
 * below. A mosh session that has not heard from the server for more than five seconds
 * ([linkHealth]) greys its badge and says how long ago, in the header row itself: nothing is ever
 * drawn over the terminal's rows, and a flapping link does not resize the grid (the row keeps its
 * height; the title gives way).
 */
@Composable
fun TerminalCard(
    title: String, transport: Transport, state: SessionState, minimise: () -> Unit, openSwitcher: () -> Unit, endSession: () -> Unit,
    modifier: Modifier = Modifier, background: Color = Or2Colors.TerminalBackground, linkHealth: LinkHealth? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val stale = linkStaleLabel(linkHealth)
    var dragY by remember { mutableFloatStateOf(0f) }
    val threshold = with(LocalDensity.current) { 96.dp.toPx() }
    Box(
        modifier.fillMaxSize().padding(top = 2.dp).offset { IntOffset(0, dragY.roundToInt()) }
            .clip(Or2Shapes.TerminalCard).background(background).testTag("terminal-card"),
    ) {
        Column(Modifier.fillMaxSize()) {
            Column(Modifier.pointerInput(Unit) {
                detectVerticalDragGestures(
                    onDragEnd = { if (dragY > threshold) minimise() else dragY = 0f },
                    onDragCancel = { dragY = 0f },
                    onVerticalDrag = { change, amount -> change.consume(); dragY = (dragY + amount).coerceAtLeast(0f) },
                )
            }.testTag("terminal-header")) {
                // The grab handle overlaps the top of the 36 dp header row, so the header costs no extra height.
                Box(Modifier.fillMaxWidth()) {
                    Box(
                        Modifier.align(Alignment.TopCenter).padding(top = 4.dp)
                            .size(width = Or2Dimens.TerminalHandleWidth, height = Or2Dimens.SheetHandleHeight)
                            .clip(Or2Shapes.Pill).background(Or2Colors.Subtle),
                    )
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = Or2Dimens.HeaderRow).padding(horizontal = 2.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        RoundHeaderButton(Or2Icons.Minimize, "Minimise to home", Or2Colors.Attention, minimise, Modifier.testTag("terminal-back"))
                        RoundHeaderButton(Or2Icons.Sidebar, "Panes and sessions", Or2Colors.Done, openSwitcher, Modifier.testTag("terminal-panes"))
                        Text(
                            title, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1,
                            overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).padding(horizontal = 4.dp).testTag("terminal-title"),
                        )
                        if (stale != null) {
                            Text(stale, style = Or2Type.MonoSmall, color = Or2Colors.Attention, maxLines = 1,
                                modifier = Modifier.padding(end = 6.dp).testTag("terminal-link"))
                        }
                        TransportBadge(transport, Modifier.padding(end = 10.dp).testTag("terminal-transport"), small = true, stale = stale != null)
                    }
                }
            }
            if (state !is SessionState.Connected) {
                Row(Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                    Text(sessionMessage(state), style = Or2Type.MonoSmall,
                        color = if (state is SessionState.Closed) Or2Colors.Attention else Or2Colors.TextMuted,
                        modifier = Modifier.weight(1f).testTag("terminal-status"))
                    if (state is SessionState.Closed) TextAction("Close", endSession, modifier = Modifier.testTag("terminal-close"))
                }
            }
            content()
        }
    }
}

@Composable
private fun RoundHeaderButton(icon: ImageVector, description: String, color: Color, onClick: () -> Unit, modifier: Modifier = Modifier) {
    // A small round disc (18 dp) in a 48 x 36 dp box; the platform grows the height of the touch target to 48 dp,
    // and neighbouring boxes do not overlap, so a tap between two discs goes to the nearer one.
    Box(
        modifier.size(Or2Dimens.HeaderButtonTouchWidth, Or2Dimens.HeaderButtonTouch).clip(Or2Shapes.Pill).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(Or2Dimens.HeaderButtonDisc).clip(Or2Shapes.Circle).background(color), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(Or2Dimens.HeaderButtonGlyph), tint = Or2Colors.Background)
        }
    }
}

/**
 * `SSH` in a `surfaceTrack` pill with full `text` (it sits on the terminal), `Mosh` in a saturated teal one.
 * [stale] (no word from the server for a while) greys either into the `SSH` look with muted text.
 */
@Composable
fun TransportBadge(transport: Transport, modifier: Modifier = Modifier, small: Boolean = false, stale: Boolean = false) {
    val (container, content) = transportBadgeColors(transport, stale)
    // The grey is not the only signal: the badge says it for assistive services (and the UI tests) too.
    val described = if (stale) modifier.semantics { stateDescription = STALE_BADGE_DESCRIPTION } else modifier
    Badge(transport.label, described, container = container, content = content, small = small)
}

/** What a greyed badge reports as its state. */
const val STALE_BADGE_DESCRIPTION = "No word from the server"

/** The badge's fill and text: teal for a healthy `Mosh`, the `SSH` look for SSH and for any stale link. */
fun transportBadgeColors(transport: Transport, stale: Boolean): Pair<Color, Color> = when {
    stale -> Or2Colors.SurfaceTrack to Or2Colors.TextMuted
    transport == Transport.SSH -> Or2Colors.SurfaceTrack to Or2Colors.Text
    else -> Or2Colors.Teal to Or2Colors.Background
}

/** A terminal that has not connected yet, or closed before it did: no terminal, no keys. */
@Composable
private fun PendingTerminal(
    terminal: ActiveTerminal, state: SessionState, closed: Boolean, endSession: () -> Unit, minimise: () -> Unit,
    open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        TopBar(title = terminal.host.label, back = minimise, backIcon = Or2Icons.ChevronDown)
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(terminal.title + " · " + sessionMessage(state), style = Or2Type.Mono, color = Or2Colors.TextMuted,
                modifier = Modifier.testTag("terminal-status"))
            PillButton(if (closed) "Close session" else "Disconnect", endSession, Modifier.testTag("terminal-end"))
            if (open.size > 1) SessionList(terminal, open, select)
        }
    }
}

/** The open sessions as a grouped list; the current one is marked. */
@Composable
private fun SessionList(current: ActiveTerminal, open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit) {
    GroupCard(Modifier.testTag("terminal-switcher")) {
        open.forEachIndexed { index, other ->
            val state by other.state.collectAsStateWithLifecycle()
            if (index > 0) GroupDivider()
            ListRow(
                other.host.label, subtitle = other.title + if (state is SessionState.Closed) " (closed)" else "", subtitleMono = true,
                modifier = Modifier.testTag("terminal-tab:${other.id}"),
                onClick = { if (other !== current) select(other) },
                trailing = if (other === current) ({
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        StatusDot(Or2Colors.Accent)
                        Spacer(Modifier.width(6.dp))
                        Text("Current", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
                    }
                }) else null,
            )
        }
    }
}

/** The panes/sidebar sheet: switch between open sessions, or end this one. */
@Composable
private fun SessionSwitcher(
    current: ActiveTerminal, open: List<ActiveTerminal>, closed: Boolean, select: (ActiveTerminal) -> Unit,
    endSession: () -> Unit, dismiss: () -> Unit,
) {
    Or2Sheet(dismiss, title = "Sessions") {
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SessionList(current, open, select)
            GroupCard {
                ListRow(
                    if (closed) "Close session" else "Disconnect session", icon = if (closed) Or2Icons.Close else Or2Icons.Power,
                    titleColor = Or2Colors.Danger, modifier = Modifier.testTag("terminal-disconnect"), onClick = endSession,
                )
            }
        }
    }
}

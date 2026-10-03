package io.github.code_akram.or2.session

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
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
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.linkStaleLabel
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.paste.NO_UPLOAD
import io.github.code_akram.or2.paste.uploadNotice
import io.github.code_akram.or2.paste.uploading
import io.github.code_akram.or2.terminal.TerminalScreen
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.terminal.display
import io.github.code_akram.or2.terminal.swipeNav
import io.github.code_akram.or2.ui.Badge
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.NoticeStrip
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.StatusDot
import io.github.code_akram.or2.ui.TopBar
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

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
            onDispose {
                // Leaving the terminal (not the swap's new view, which keeps this effect): tmux or herdr back to live.
                holder.hideTerminal(terminal)
                holder.detachDisplay(terminal)
            }
        }
        val state by terminal.state.collectAsStateWithLifecycle()
        val handle by terminal.handle.collectAsStateWithLifecycle()
        val hasConnected by terminal.hasConnected.collectAsStateWithLifecycle()
        val transport by terminal.transport.collectAsStateWithLifecycle()
        val linkHealth by terminal.linkHealth.collectAsStateWithLifecycle()
        val paste = terminal.imagePaste
        val upload by (paste?.state ?: NO_UPLOAD).collectAsStateWithLifecycle()
        val closed = state is SessionState.Closed
        // "Close session" ends the terminal in one tap: an open one is disconnected (the usual cleanup: its mosh server
        // is told to stop, the ledger cleared on its Closed) and dismissed together, a closed one only dismissed; then
        // Home. A terminal that closed by itself (a lost connection, a remote exit) keeps its final frame until then.
        val endSession = {
            if (!closed) holder.disconnectTerminal(terminal)
            holder.dismissTerminal(terminal)
            minimise()
        }
        var switcher by remember { mutableStateOf(false) }
        val scope = rememberCoroutineScope()
        val haptics = LocalHapticFeedback.current
        // The card follows the terminal's own background, which the remote can change (OSC 11).
        var background by remember { mutableStateOf(Or2Colors.TerminalBackground) }
        if (hasConnected) {
            TerminalCard(terminal.host.label, terminal.title, transport.display(), state, minimise, openSwitcher = { switcher = true }, endSession,
                background = background, linkHealth = linkHealth, upload = uploadNotice(upload),
                uploadAction = { if (upload.uploading) paste?.cancel() else paste?.dismiss() }) {
                // Keep the borrowed handle composed through Closed so its final frame stays visible.
                handle?.let { TerminalScreen(it, terminal.state, terminal.frameReady, Modifier.weight(1f),
                    composerHint = "Message " + terminal.host.label + "…",
                    onBackground = { background = it }, onFrameDrawn = { holder.timing.terminalFrame(terminal.id) },
                    target = terminal.target, targetScroller = terminal.targetScroller, input = terminal.input,
                    // Swipes move tmux or herdr; a shell has nothing to move and keeps every touch.
                    onSwipe = if (terminal.target is TerminalTarget.Shell) null else { swipe ->
                        haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
                        scope.launch { holder.navigate(terminal, swipeNav(swipe)) }
                    },
                    switchTo = { index -> open.getOrNull(index)?.let { if (it !== terminal) select(it) } },
                    closeTerminal = { holder.dismissTerminal(terminal); minimise() }, imagePaste = paste) }
            }
        } else {
            PendingTerminal(terminal, state, endSession, minimise, open, select)
        }
        if (switcher) {
            SessionSwitcher(terminal, open, select = { switcher = false; select(it) }, endSession = { switcher = false; endSession() },
                dismiss = { switcher = false })
        }
    }
}

/**
 * The full-height card: the [TerminalHeader] (drag handle, the two discs, the centred `host · target`
 * title, the [transport] pill) on the header's tonal step, one [NoticeStrip] under it (a closed
 * terminal's reason with **Close**, else an image upload's [upload] with its action [uploadAction],
 * Cancel or Dismiss), a `crust` hairline, and the terminal below. A drag down anywhere on the header
 * minimises. A mosh session that has not heard from the server for more than five seconds
 * ([linkHealth]) says how long ago inside its pill (`Mosh · 12 s` in `attention`): nothing is ever drawn
 * over the terminal's rows, and a flapping link does not resize the grid (the title gives way).
 */
@Composable
fun TerminalCard(
    host: String, target: String, transport: Transport, state: SessionState, minimise: () -> Unit, openSwitcher: () -> Unit,
    endSession: () -> Unit, modifier: Modifier = Modifier, background: Color = Or2Colors.TerminalBackground, linkHealth: LinkHealth? = null,
    upload: TerminalNotice? = null, uploadAction: () -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val stale = linkStaleLabel(linkHealth)
    val closed = terminalNotice(state)
    var dragY by remember { mutableFloatStateOf(0f) }
    val threshold = with(LocalDensity.current) { 96.dp.toPx() }
    Box(
        modifier.fillMaxSize().padding(top = 2.dp).offset { IntOffset(0, dragY.roundToInt()) }
            .clip(Or2Shapes.TerminalCard).background(background).testTag("terminal-card"),
    ) {
        Column(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxWidth().background(Or2Colors.TerminalHeader)) {
                TerminalHeader(
                    host, target, transport, stale, minimise, openSwitcher,
                    Modifier.pointerInput(Unit) {
                        detectVerticalDragGestures(
                            onDragEnd = { if (dragY > threshold) minimise() else dragY = 0f },
                            onDragCancel = { dragY = 0f },
                            onVerticalDrag = { change, amount -> change.consume(); dragY = (dragY + amount).coerceAtLeast(0f) },
                        )
                    }.testTag("terminal-header"),
                )
                (closed ?: upload)?.let { notice ->
                    val tag = if (closed != null) "terminal" else "upload"
                    NoticeStrip(
                        notice.text, Modifier.testTag("$tag-notice"), tone = notice.tone, busy = notice.busy,
                        actionLabel = notice.action, onAction = if (closed != null) endSession else uploadAction,
                        actionModifier = Modifier.testTag(if (closed != null) "terminal-close" else "upload-action"),
                        textModifier = Modifier.testTag("$tag-status"),
                    )
                }
            }
            // A hairline in `crust` between the header's tone and the grid.
            Box(Modifier.fillMaxWidth().height(Dp.Hairline).background(Or2Colors.Crust).testTag("terminal-header-line"))
            content()
        }
    }
}

/** `SSH` in a `surfaceTrack` pill with full `text` (it sits on the terminal), `Mosh` in a saturated teal one. */
@Composable
fun TransportBadge(transport: Transport, modifier: Modifier = Modifier) {
    val (container, content) = transportBadgeColors(transport)
    Badge(transport.label, container, content, modifier)
}

/** What the header's pill reports as its state while the link is quiet (`Mosh · 12 s`). */
const val STALE_BADGE_DESCRIPTION = "No word from the server"

/** The badge's fill and text: teal with dark text for `Mosh`, the `SSH` track look for SSH. */
fun transportBadgeColors(transport: Transport): Pair<Color, Color> =
    if (transport == Transport.SSH) Or2Colors.SurfaceTrack to Or2Colors.Text else Or2Colors.Teal to Or2Colors.Background

/** A terminal that has not connected yet, or closed before it did: no terminal, no keys. */
@Composable
private fun PendingTerminal(
    terminal: ActiveTerminal, state: SessionState, endSession: () -> Unit, minimise: () -> Unit,
    open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        TopBar(title = terminal.host.label, back = minimise, backIcon = Or2Icons.ChevronDown)
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(terminal.title + " · " + sessionMessage(state), style = Or2Type.Mono, color = Or2Colors.TextMuted,
                modifier = Modifier.testTag("terminal-status"))
            PillButton("Close session", endSession, Modifier.testTag("terminal-end"))
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

/** The panes/sidebar sheet: switch between open sessions, or close this one ("Close session": one tap, then Home). */
@Composable
private fun SessionSwitcher(
    current: ActiveTerminal, open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit,
    endSession: () -> Unit, dismiss: () -> Unit,
) {
    Or2Sheet(dismiss, title = "Sessions") {
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SessionList(current, open, select)
            GroupCard {
                ListRow(
                    "Close session", icon = Or2Icons.Close, titleColor = Or2Colors.Danger,
                    modifier = Modifier.testTag("terminal-close-session"), onClick = endSession,
                )
            }
        }
    }
}

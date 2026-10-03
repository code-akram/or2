package io.github.code_akram.or2.session

import io.github.code_akram.or2.ui.copyText
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import io.github.code_akram.or2.app.CLOSE_SHELL_TEXT
import io.github.code_akram.or2.app.CLOSE_SHELL_TITLE
import io.github.code_akram.or2.app.closeAsks
import io.github.code_akram.or2.terminal.ShortcutsSheet
import io.github.code_akram.or2.terminal.TerminalChromeState
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.TextAction
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
import io.github.code_akram.or2.ui.NoticeTone
import io.github.code_akram.or2.inbox.herdrViews
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
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
        // Closing: the × of the Terminals sheet, Ctrl+Shift+W and the closed strip's Close, all through the one close
        // function (TerminalActivations.close: an open terminal is disconnected and dismissed together, a closed one only
        // dismissed). An open shell asks first. Closing the terminal on screen returns Home, the calm place to land.
        var closing by remember { mutableStateOf<ActiveTerminal?>(null) }
        fun close(target: ActiveTerminal) {
            holder.activations.close(target)
            if (target === terminal) minimise()
        }
        fun requestClose(target: ActiveTerminal) {
            if (closeAsks(target.target, target.state.value is SessionState.Closed)) closing = target else close(target)
        }
        val endSession = { requestClose(terminal) }
        // The Terminals sheet (the green disc), and the shortcuts sheet it opens.
        var switcher by remember { mutableStateOf(false) }
        var shortcuts by remember { mutableStateOf(false) }
        val chrome = remember { TerminalChromeState() }
        val context = LocalContext.current
        val scope = rememberCoroutineScope()
        val haptics = LocalHapticFeedback.current
        // The card follows the terminal's own background, which the remote can change (OSC 11).
        var background by remember { mutableStateOf(Or2Colors.TerminalBackground) }
        // The Spaces sheet (the blue disc, herdr terminals only), and why its last tap could not focus, if it could not.
        val herdr = terminal.target as? TerminalTarget.Herdr
        var spaces by remember { mutableStateOf(false) }
        var focusFailure by remember { mutableStateOf<String?>(null) }
        val uploadShown = uploadNotice(upload)
        if (hasConnected) {
            TerminalCard(terminal.host.label, terminal.title, transport.display(), state, minimise, openSwitcher = { switcher = true }, endSession,
                background = background, linkHealth = linkHealth,
                // An upload's notice first; a failed Spaces tap takes the same strip, with Dismiss.
                upload = uploadShown ?: focusFailure?.let { TerminalNotice(it, NoticeTone.Warning, busy = false, action = "Dismiss") },
                uploadAction = {
                    when {
                        uploadShown == null -> focusFailure = null
                        upload.uploading -> paste?.cancel()
                        else -> paste?.dismiss()
                    }
                },
                openSpaces = herdr?.let { { spaces = true } }) {
                // Keep the borrowed handle composed through Closed so its final frame stays visible.
                handle?.let { TerminalScreen(it, terminal.state, terminal.frameReady, Modifier.weight(1f),
                    composerHint = "Message " + terminal.host.label + "…",
                    onBackground = { background = it }, onFrameDrawn = { holder.timing.terminalFrame(terminal.id) },
                    target = terminal.target, targetScroller = terminal.targetScroller, input = terminal.input, chrome = chrome,
                    // Swipes move tmux or herdr; a shell has nothing to move and keeps every touch.
                    onSwipe = if (terminal.target is TerminalTarget.Shell) null else { swipe ->
                        haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
                        scope.launch { holder.navigate(terminal, swipeNav(swipe)) }
                    },
                    switchTo = { index -> open.getOrNull(index)?.let { if (it !== terminal) select(it) } },
                    closeTerminal = { requestClose(terminal) }, imagePaste = paste) }
            }
        } else {
            PendingTerminal(terminal, state, minimise, open, select, ::requestClose)
        }
        if (switcher) {
            TerminalsSheet(
                terminalItems(open), terminal.id,
                select = { id -> switcher = false; open.find { it.id == id }?.takeIf { it !== terminal }?.let(select) },
                close = { id -> open.find { it.id == id }?.let(::requestClose) },
                copyScreen = {
                    switcher = false
                    // Android shows its own "copied" confirmation (API 33 and later; or2 needs 34).
                    chrome.screenText().takeIf { it.isNotEmpty() }?.let { text -> copyText(context, "Terminal screen", text) }
                },
                shortcuts = { switcher = false; shortcuts = true },
                dismiss = { switcher = false },
            )
        }
        if (spaces && herdr != null) {
            // The session's live view only while the sheet is up: nothing recomposes the terminal for it otherwise.
            val views = remember(holder) { holder.herdrViews() }
            val all by views.collectAsStateWithLifecycle(emptyMap())
            SpacesSheet(
                herdr.session, all[terminal.host.id to herdr.session],
                focus = { target ->
                    spaces = false
                    focusFailure = null
                    scope.launch { focusFailure = holder.activations.focusInTerminal(terminal, target) }
                },
                dismiss = { spaces = false },
            )
        }
        if (shortcuts) ShortcutsSheet(dismiss = { shortcuts = false })
        closing?.let { target ->
            CloseShellDialog(close = { closing = null; close(target) }, dismiss = { closing = null })
        }
    }
}

/** The confirmation of closing an open shell ([closeAsks]): its programs end with it. */
@Composable
fun CloseShellDialog(close: () -> Unit, dismiss: () -> Unit) {
    Or2Dialog(
        onDismiss = dismiss, title = CLOSE_SHELL_TITLE,
        confirm = { TextAction("Close", close, color = Or2Colors.Danger, modifier = Modifier.testTag("close-shell-confirm")) },
        dismiss = { TextAction("Cancel", dismiss, color = Or2Colors.Text) },
    ) { Text(CLOSE_SHELL_TEXT) }
}

/**
 * The full-height card: the [TerminalHeader] (drag handle, the discs (a third, Spaces, with [openSpaces]), the centred `host · target`
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
    upload: TerminalNotice? = null, uploadAction: () -> Unit = {}, openSpaces: (() -> Unit)? = null,
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
                    openSpaces = openSpaces,
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

/** A terminal that has not connected yet, or closed before it did: no terminal, no keys; the open terminals to close or switch to. */
@Composable
private fun PendingTerminal(
    terminal: ActiveTerminal, state: SessionState, minimise: () -> Unit, open: List<ActiveTerminal>,
    select: (ActiveTerminal) -> Unit, close: (ActiveTerminal) -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        TopBar(title = terminal.host.label, back = minimise, backIcon = Or2Icons.ChevronDown)
        Column(Modifier.padding(horizontal = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(terminal.title + " · " + sessionMessage(state), style = Or2Type.Mono, color = Or2Colors.TextMuted,
                modifier = Modifier.testTag("terminal-status"))
            TerminalGroups(
                terminalItems(open), terminal.id,
                select = { id -> open.find { it.id == id }?.takeIf { it !== terminal }?.let(select) },
                close = { id -> open.find { it.id == id }?.let(close) },
            )
        }
    }
}

/** One open terminal as the Terminals sheet lists it, under its host. */
data class TerminalItem(val id: Long, val hostId: Long, val hostLabel: String, val title: String, val closed: Boolean)

/** The [TerminalItem]s of [open], in order, each with its state as it is now. */
@Composable
private fun terminalItems(open: List<ActiveTerminal>): List<TerminalItem> = open.map { terminal ->
    key(terminal.id) {
        val state by terminal.state.collectAsStateWithLifecycle()
        TerminalItem(terminal.id, terminal.host.id, terminal.host.label, terminal.title, state is SessionState.Closed)
    }
}

/**
 * The Terminals sheet (the terminal header's green disc): every open terminal grouped by host, the one on screen
 * ([currentId]) marked `● Current`, a tap switching to another ([select]) and each row's `×` closing it ([close]: the
 * same rules as Home's). Then **Copy screen** (the visible screen's text to the clipboard) and **Gestures &
 * shortcuts** (the shortcuts sheet).
 */
@Composable
fun TerminalsSheet(
    items: List<TerminalItem>, currentId: Long, select: (Long) -> Unit, close: (Long) -> Unit, copyScreen: () -> Unit,
    shortcuts: () -> Unit, dismiss: () -> Unit,
) {
    Or2Sheet(dismiss, title = "Terminals", modifier = Modifier.testTag("terminals-sheet")) {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            TerminalGroups(items, currentId, select, close)
            GroupCard {
                ListRow("Copy screen", icon = Or2Icons.Copy, modifier = Modifier.testTag("terminals-copy-screen"), onClick = copyScreen)
                GroupDivider(inset = 44.dp)
                ListRow("Gestures & shortcuts", icon = Or2Icons.Keyboard, modifier = Modifier.testTag("terminals-shortcuts"), onClick = shortcuts)
            }
        }
    }
}

/** The open terminals under one header per host (in the order their first terminal opened), on cards of [color]. */
@Composable
private fun TerminalGroups(items: List<TerminalItem>, currentId: Long, select: (Long) -> Unit, close: (Long) -> Unit) {
    Column(Modifier.testTag("terminal-switcher")) {
        items.groupBy { it.hostId }.values.forEachIndexed { index, group ->
            SectionHeader(group.first().hostLabel, topGap = if (index == 0) 0.dp else 6.dp)
            GroupCard {
                group.forEachIndexed { row, item ->
                    if (row > 0) GroupDivider()
                    TerminalRow(item, item.id == currentId, { select(item.id) }, { close(item.id) })
                }
            }
        }
    }
}

@Composable
private fun TerminalRow(item: TerminalItem, current: Boolean, select: () -> Unit, close: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = Or2Dimens.RowMin).clickable(role = Role.Button, onClick = select)
            .testTag("terminal-tab:${item.id}").padding(start = Or2Dimens.Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(item.title, style = Or2Type.RowLabel, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (item.closed) Text("Closed", style = Or2Type.Secondary, color = Or2Colors.TextMuted, maxLines = 1)
        }
        if (current) {
            StatusDot(Or2Colors.Accent)
            Spacer(Modifier.width(6.dp))
            Text("Current", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
        }
        IconAction(Or2Icons.Close, "Close ${item.title}", close, Modifier.testTag("terminal-row-close:${item.id}"), tint = Or2Colors.TextMuted)
    }
}


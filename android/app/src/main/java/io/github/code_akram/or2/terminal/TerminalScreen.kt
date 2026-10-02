package io.github.code_akram.or2.terminal

import android.content.ClipboardManager
import android.graphics.RectF
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.TextAction
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/**
 * The terminal and its input chrome: the Canvas terminal edge to edge, the arrow pad floating
 * over it, and below it the floating key toolbar, with the composer above the toolbar while it is
 * open (Esc, Ctrl and Tab stay reachable). IME insets are owned here, so both ride on top of the
 * keyboard. [chrome] is the hoisted open/closed state; [onBackground] reports the terminal's own
 * background colour (the remote can change it) so the card around it can follow. A tmux or herdr
 * [target] scrolls its own history through the terminal's [targetScroller] (`scroll_target`) when
 * the program does not track the mouse; a small round button returns to the bottom while anything is
 * scrolled up. Every input of the view (keys, IME text, paste, the composer, the toolbar and pad,
 * wheel scrolls) goes to the terminal's [input] route, resolved at call time; [session] is only where
 * this view's frames come from.
 */
@Composable
fun TerminalScreen(
    session: SessionInterface,
    state: StateFlow<SessionState>,
    frameReady: Flow<Unit>,
    modifier: Modifier = Modifier,
    composerHint: String = "Message…",
    openPanes: () -> Unit = {},
    onBackground: (Color) -> Unit = {},
    chrome: TerminalChromeState = remember { TerminalChromeState() },
    /** A frame was drawn (reported to the timing markers, which ignore it unless a path is waiting for one). */
    onFrameDrawn: () -> Unit = {},
    target: TerminalTarget = TerminalTarget.Shell,
    /** The terminal's own target scroller (its state outlives this view); null scrolls the local viewport. */
    targetScroller: TargetScroller? = null,
    /** The terminal's input route (its current session at call time); null sends input to [session]. */
    input: SessionRoute? = null,
    /** A navigation swipe on the terminal ([SwipeClassifier]); null (a shell) recognises none. */
    onSwipe: ((Swipe) -> Unit)? = null,
    /** Ctrl+Shift+1..9: the open terminal at this index (0-based, Home's order). */
    switchTo: (Int) -> Unit = {},
    /** Ctrl+Shift+W. */
    closeTerminal: () -> Unit = {},
) {
    key(session) {
        val context = LocalContext.current
        val haptics = LocalHapticFeedback.current
        val view = remember(session, context) {
            TerminalView(context).apply {
                bind(session, input)
                if (targetScroller != null && target !is TerminalTarget.Shell) useTargetScroll(target, targetScroller)
            }
        }
        // A terminal scrolled away before this view existed (shown again, or swapped to mosh) shows the button at once.
        var scrolledAway by remember { mutableStateOf(view.scrolledAway) }
        var ctrl by remember { mutableStateOf(false) }
        var alt by remember { mutableStateOf(false) }
        var selecting by remember { mutableStateOf(false) }
        var pendingPaste by remember { mutableStateOf<String?>(null) }
        var pendingSend by remember { mutableStateOf<String?>(null) }
        var shortcutsOpen by remember { mutableStateOf(false) }
        val sessionState by state.collectAsState()
        val background by rememberUpdatedState(onBackground)
        val frameDrawn by rememberUpdatedState(onFrameDrawn)
        DisposableEffect(view) {
            view.onFrameDrawn = { frameDrawn() }
            view.onInputChanged = { ctrl = view.input.ctrl; alt = view.input.alt }
            view.onSelectionChanged = { selecting = view.selection != null }
            view.onBackgroundChanged = { background(Color(it.toInt() or (0xff shl 24))) }
            view.onScrolledAwayChanged = { scrolledAway = it }
            scrolledAway = view.scrolledAway
            onDispose {
                view.onFrameDrawn = {}
                view.onInputChanged = {}
                view.onSelectionChanged = {}
                view.onBackgroundChanged = {}
                view.onScrolledAwayChanged = {}
            }
        }
        LaunchedEffect(view, state, frameReady) {
            launch { state.collect { view.sessionState(it) } }
            launch { frameReady.collect { view.frameReady() } }
            // The terminal's scroller outlives this view: a Bottom that returns (or fails) after a swap or a return reaches it.
            if (view.targetScroller != null) launch { view.targetScroller?.awayState?.collect { view.targetScrollChanged() } }
        }
        fun clipboardText() = context.getSystemService(ClipboardManager::class.java).primaryClip
            ?.getItemAt(0)?.text?.toString().orEmpty()
        val toolbar = ToolbarActions(
            toggleCtrl = { view.input.toggleCtrl() },
            toggleAlt = { view.input.toggleAlt() },
            escape = { view.input.key(TerminalKey.Escape) },
            tab = { view.input.key(TerminalKey.Tab) },
            togglePad = { chrome.padOpen = !chrome.padOpen },
            panes = openPanes,
            paste = {
                val text = clipboardText()
                if (pasteNeedsConfirmation(text)) pendingPaste = text else view.paste(text)
            },
            history = { view.pageUp() },
            jumpToBottom = { view.jumpToBottom() },
            toggleComposer = {
                chrome.composerOpen = !chrome.composerOpen
                if (chrome.composerOpen) chrome.padOpen = false
            },
            toggleKeyboard = {
                val shown = ViewCompat.getRootWindowInsets(view)?.isVisible(WindowInsetsCompat.Type.ime()) == true
                if (shown) view.hideKeyboard() else view.showKeyboard()
            },
            copy = { view.copySelection() },
            clearSelection = { view.clearSelection() },
        )
        val shortcut by rememberUpdatedState<(TerminalShortcut) -> Unit> { pressed ->
            when (pressed) {
                is TerminalShortcut.SwitchTo -> switchTo(pressed.index)
                TerminalShortcut.Close -> closeTerminal()
                TerminalShortcut.Paste -> toolbar.paste()
                TerminalShortcut.Copy -> if (view.selection != null) view.copySelection()
                TerminalShortcut.Composer -> {
                    toolbar.toggleComposer()
                    // Closed from inside the composer: the keys go back to the terminal.
                    if (!chrome.composerOpen) view.requestFocus()
                }
                TerminalShortcut.Help -> shortcutsOpen = true
            }
        }
        val swiped by rememberUpdatedState(onSwipe)
        val swipes = onSwipe != null
        DisposableEffect(view, swipes) {
            view.onShortcut = { shortcut(it) }
            view.onSwipe = if (swipes) ({ swipe -> swiped?.invoke(swipe) }) else null
            onDispose {
                view.onShortcut = {}
                view.onSwipe = null
            }
        }
        val pad = PadActions(
            backspace = { view.input.key(TerminalKey.Backspace) },
            up = { view.input.key(TerminalKey.ArrowUp) },
            // Ctrl-U, the shell's "clear the line before the cursor".
            clearLine = { view.input.key(TerminalKey.Character("u"), KeyModifiers(false, true, false, false)) },
            left = { view.input.key(TerminalKey.ArrowLeft) },
            enter = { view.input.key(TerminalKey.Enter) },
            right = { view.input.key(TerminalKey.ArrowRight) },
            down = { view.input.key(TerminalKey.ArrowDown) },
            extra = { label ->
                view.input.key(when (label) {
                    "Home" -> TerminalKey.Home
                    "End" -> TerminalKey.End
                    "PgUp" -> TerminalKey.PageUp
                    "PgDn" -> TerminalKey.PageDown
                    else -> TerminalKey.Character(label)
                })
            },
        )
        Column(modifier.fillMaxSize().windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))) {
            Box(Modifier.weight(1f)) {
                AndroidView(factory = { view }, modifier = Modifier.fillMaxSize().clipToBounds())
                if (scrolledAway) {
                    ScrollToBottomButton({ view.jumpToBottom() }, Modifier.align(Alignment.BottomEnd).padding(end = 4.dp, bottom = 4.dp))
                }
                if (chrome.padOpen) ArrowPad(pad, alt, { view.input.toggleAlt() }, collapse = { chrome.padOpen = false }, modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp))
            }
            if (chrome.composerOpen) {
                Composer(
                    placeholder = composerHint,
                    text = chrome.composerText,
                    onTextChange = { chrome.composerText = it },
                    canSend = sessionState == SessionState.Connected,
                    // Several lines would run as typed, so they are confirmed like a multi-line paste.
                    send = { text ->
                        if (pasteNeedsConfirmation(text)) {
                            pendingSend = text
                            false
                        } else {
                            view.sendLine(text)
                        }
                    },
                    close = { chrome.composerOpen = false },
                    // A hardware keyboard's shortcuts work while the composer has the keys, except
                    // paste and copy, which are the text field's own there.
                    modifier = Modifier.onPreviewKeyEvent { event ->
                        val native = event.nativeKeyEvent
                        val pressed = terminalShortcut(native.keyCode, native.isCtrlPressed, native.isShiftPressed, native.isAltPressed, native.isMetaPressed)
                        if (native.flags and KeyEvent.FLAG_SOFT_KEYBOARD != 0 || pressed == null ||
                            pressed == TerminalShortcut.Paste || pressed == TerminalShortcut.Copy
                        ) return@onPreviewKeyEvent false
                        if (native.action == KeyEvent.ACTION_DOWN && native.repeatCount == 0) shortcut(pressed)
                        true
                    },
                )
            }
            KeyToolbar(
                ToolbarState(ctrl, alt, selecting, chrome.padOpen, chrome.composerOpen), toolbar,
                Modifier.onGloballyPositioned { view.toolbarBounds = it.unclippedBoundsInRoot() },
                onKeyPositioned = { label, coordinates -> view.toolbarKeyBounds[label] = coordinates.unclippedBoundsInRoot() },
            )
        }
        if (shortcutsOpen) ShortcutsSheet(dismiss = { shortcutsOpen = false })
        pendingSend?.let { text ->
            Or2Dialog(
                onDismiss = { pendingSend = null }, title = "Send ${pasteLineCount(text)} lines?",
                confirm = {
                    TextAction("Send", {
                        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                        pendingSend = null
                        // Cleared only when it went out; after a drop the message stays in the composer.
                        if (view.sendLine(text)) chrome.composerText = ""
                    }, modifier = Modifier.testTag("composer-send-confirm"))
                },
                dismiss = { TextAction("Cancel", { pendingSend = null }, color = Or2Colors.Text) },
            ) { Text("They will run as typed, one line at a time.") }
        }
        pendingPaste?.let { text ->
            Or2Dialog(
                onDismiss = { pendingPaste = null }, title = "Paste ${pasteLineCount(text)} lines?",
                confirm = {
                    TextAction("Paste", {
                        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                        pendingPaste = null
                        view.paste(text)
                    })
                },
                dismiss = { TextAction("Cancel", { pendingPaste = null }, color = Or2Colors.Text) },
            ) { Text("They will run as typed.") }
        }
    }
}

/**
 * The scroll-to-bottom button: a small round disc ([Or2Dimens.Chip]) with a down chevron, in a
 * [Or2Dimens.KeyTouch] box over the terminal's bottom-right corner (the platform grows the target).
 */
@Composable
private fun ScrollToBottomButton(onClick: () -> Unit, modifier: Modifier = Modifier) {
    Box(
        modifier.size(Or2Dimens.KeyTouch).clip(Or2Shapes.Circle)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = "Scroll to bottom" }.testTag("scroll-to-bottom"),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            Modifier.size(Or2Dimens.Chip).clip(Or2Shapes.Circle).background(Or2Colors.ToolbarPill),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Or2Icons.ArrowDown, null, Modifier.size(Or2Dimens.HeaderButtonGlyph + 4.dp), tint = Or2Colors.Accent)
        }
    }
}

// Position + measured size, not boundsInRoot(), so clipping cannot hide a partial key.
private fun LayoutCoordinates.unclippedBoundsInRoot(): RectF {
    val position = positionInRoot()
    return RectF(position.x, position.y, position.x + size.width, position.y + size.height)
}

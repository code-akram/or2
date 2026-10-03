package io.github.code_akram.or2.terminal

import android.graphics.RectF
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
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
import io.github.code_akram.or2.paste.ImagePaste
import io.github.code_akram.or2.paste.InsertTarget
import io.github.code_akram.or2.paste.MAX_IMAGES
import io.github.code_akram.or2.paste.composerWithPaths
import io.github.code_akram.or2.paste.imageFromUri
import io.github.code_akram.or2.paste.insertablePath
import io.github.code_akram.or2.paste.insertTarget
import io.github.code_akram.or2.paste.pathsInsertion
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.clipboardText
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/** The composer's attach button's Photo Picker: several images at once, at most [MAX_IMAGES], no permission. */
val IMAGE_PICKER = ActivityResultContracts.PickMultipleVisualMedia(MAX_IMAGES)

/** What [IMAGE_PICKER] asks for: images only. */
fun imagePickRequest(): PickVisualMediaRequest = PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)

/** Shift alone: the toolbar's `⇧Tab`. */
private val Shift = KeyModifiers(true, false, false, false)

/**
 * Several lines about to run as typed, asked first ([pasteNeedsConfirmation]): the composer's message ([send]) or a
 * paste. One dialog serves both.
 */
private class PendingLines(val text: String, val send: Boolean)

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
 * this view's frames come from. The pad and the composer are never open together; closing the
 * composer gives the keys back to the terminal.
 */
@Composable
fun TerminalScreen(
    session: SessionInterface,
    state: StateFlow<SessionState>,
    frameReady: Flow<Unit>,
    modifier: Modifier = Modifier,
    composerHint: String = "Message…",
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
    /**
     * The terminal's image paste: the composer's attach button (the Photo Picker) and a keyboard's
     * images upload through its queue, and each run's uploaded paths are inserted here together
     * (contracts.md, "Image paste"). Null takes no images.
     */
    imagePaste: ImagePaste? = null,
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
        var shift by remember { mutableStateOf(false) }
        var selecting by remember { mutableStateOf(false) }
        var pending by remember { mutableStateOf<PendingLines?>(null) }
        var shortcutsOpen by remember { mutableStateOf(false) }
        val sessionState by state.collectAsState()
        // Images: from the Photo Picker (images only, at most MAX_IMAGES, no storage permission) or a keyboard, each
        // joining the terminal's upload queue, in the order picked.
        val picker = rememberLauncherForActivityResult(IMAGE_PICKER) { uris ->
            for (uri in uris) imagePaste?.start(imageFromUri(context.applicationContext, uri))
        }
        val attach = imagePaste?.let { { picker.launch(imagePickRequest()) } }
        val receiveImage = imagePaste?.let { paste -> { uri: Uri -> paste.start(imageFromUri(context.applicationContext, uri)) } }
        DisposableEffect(view, imagePaste) {
            view.onImage = imagePaste?.let { paste -> { uri, release -> paste.start(imageFromUri(context.applicationContext, uri, release)) } }
            onDispose { view.onImage = null }
        }
        // A queue run's uploaded paths, together: each a space and the quoted path, no Enter; into the message being
        // written when the composer is open, else one paste into the terminal (bracketed when the program asked for that).
        LaunchedEffect(view, imagePaste) {
            imagePaste?.paths?.collect { paths ->
                // ImagePaste delivers only insertable paths; never type anything else.
                val usable = paths.filter(::insertablePath)
                if (usable.isEmpty()) return@collect
                when (insertTarget(chrome.composerOpen)) {
                    InsertTarget.COMPOSER -> chrome.composerText = composerWithPaths(chrome.composerText, usable)
                    InsertTarget.TERMINAL -> view.pasteText(pathsInsertion(usable))
                }
            }
        }
        val background by rememberUpdatedState(onBackground)
        val frameDrawn by rememberUpdatedState(onFrameDrawn)
        DisposableEffect(view, chrome) {
            view.onFrameDrawn = { frameDrawn() }
            view.onInputChanged = { ctrl = view.input.ctrl; alt = view.input.alt; shift = view.input.shift }
            view.onSelectionChanged = { selecting = view.selection != null }
            view.onBackgroundChanged = { background(Color(it.toInt() or (0xff shl 24))) }
            view.onScrolledAwayChanged = { scrolledAway = it }
            scrolledAway = view.scrolledAway
            chrome.screenText = { view.screenText() }
            onDispose {
                view.onFrameDrawn = {}
                view.onInputChanged = {}
                view.onSelectionChanged = {}
                view.onBackgroundChanged = {}
                view.onScrolledAwayChanged = {}
                chrome.screenText = { "" }
            }
        }
        LaunchedEffect(view, state, frameReady) {
            launch { state.collect { view.sessionState(it) } }
            launch { frameReady.collect { view.frameReady() } }
            // The terminal's scroller outlives this view: a Bottom that returns (or fails) after a swap or a return reaches it.
            if (view.targetScroller != null) launch { view.targetScroller?.awayState?.collect { view.targetScrollChanged() } }
        }
        // One paste path, the session's `paste_text`: one bracketed paste when the program has that mode on (nothing to
        // confirm then), typed as it is otherwise, after the "Paste N lines?" confirmation for several lines.
        fun paste(text: String) {
            if (text.isEmpty()) return
            if (pasteNeedsConfirmation(text, view.grid.modes.bracketedPaste)) pending = PendingLines(text, send = false) else view.pasteText(text)
        }
        // Closing the composer (its ×, the toolbar toggle, Ctrl+Shift+Enter, opening the pad) gives the keys back to the terminal.
        fun closeComposer() {
            chrome.composerOpen = false
            view.requestFocus()
        }
        fun press(key: ToolbarKey) {
            when (key) {
                ToolbarKey.COPY -> view.copySelection()
                ToolbarKey.CLEAR -> view.clearSelection()
                ToolbarKey.CTRL -> view.input.toggleCtrl()
                ToolbarKey.SHIFT -> view.input.toggleShift()
                ToolbarKey.ESC -> view.input.key(TerminalKey.Escape)
                ToolbarKey.TAB -> view.input.key(TerminalKey.Tab)
                ToolbarKey.ARROWS -> {
                    chrome.padOpen = !chrome.padOpen
                    if (chrome.padOpen && chrome.composerOpen) closeComposer()
                }
                ToolbarKey.PASTE -> paste(clipboardText(context))
                // Shift+Tab whatever is latched: the latches stay for the next key.
                ToolbarKey.SHIFT_TAB -> view.input.exactKey(TerminalKey.Tab, Shift)
                ToolbarKey.SLASH, ToolbarKey.AT -> {
                    val text = key.label!!
                    if (chrome.composerOpen) chrome.typeInComposer(text) else view.input.key(TerminalKey.Character(text))
                }
                ToolbarKey.COMPOSER -> if (chrome.composerOpen) {
                    closeComposer()
                } else {
                    chrome.composerOpen = true
                    chrome.padOpen = false
                }
                ToolbarKey.KEYBOARD -> {
                    val shown = ViewCompat.getRootWindowInsets(view)?.isVisible(WindowInsetsCompat.Type.ime()) == true
                    if (shown) view.hideKeyboard() else view.showKeyboard()
                }
            }
        }
        val shortcut by rememberUpdatedState<(TerminalShortcut) -> Unit> { pressed ->
            when (pressed) {
                is TerminalShortcut.SwitchTo -> switchTo(pressed.index)
                TerminalShortcut.Close -> closeTerminal()
                TerminalShortcut.Paste -> press(ToolbarKey.PASTE)
                TerminalShortcut.Copy -> if (view.selection != null) view.copySelection()
                TerminalShortcut.Composer -> press(ToolbarKey.COMPOSER)
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
        Column(modifier.fillMaxSize().windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))) {
            Box(Modifier.weight(1f)) {
                AndroidView(factory = { view }, modifier = Modifier.fillMaxSize().clipToBounds())
                if (scrolledAway) {
                    ScrollToBottomButton({ view.jumpToBottom() }, Modifier.align(Alignment.BottomEnd).padding(end = 4.dp, bottom = 4.dp))
                }
                if (chrome.padOpen) {
                    ArrowPad(
                        { key, modifiers -> view.input.key(key, modifiers) }, alt, { view.input.toggleAlt() },
                        modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 4.dp),
                    )
                }
            }
            if (chrome.composerOpen) {
                Composer(
                    placeholder = composerHint,
                    state = chrome.composer,
                    canSend = sessionState == SessionState.Connected,
                    attach = attach,
                    receiveImage = receiveImage,
                    // Several lines would run as typed, so they are confirmed like a multi-line paste; with bracketed
                    // paste on they arrive as one paste and one Enter, and nothing is asked.
                    send = { text ->
                        if (pasteNeedsConfirmation(text, view.grid.modes.bracketedPaste)) {
                            pending = PendingLines(text, send = true)
                            false
                        } else {
                            view.sendLine(text)
                        }
                    },
                    close = ::closeComposer,
                    // A hardware keyboard's shortcuts work while the composer has the keys, except
                    // paste and copy, which are the text field's own there.
                    modifier = Modifier.onPreviewKeyEvent { event ->
                        consumeShortcut(event.nativeKeyEvent, { shortcut(it) }, passes = setOf(TerminalShortcut.Paste, TerminalShortcut.Copy))
                    },
                )
            }
            KeyToolbar(
                ToolbarState(ctrl, selecting, chrome.padOpen, chrome.composerOpen, shift), ::press,
                Modifier.onGloballyPositioned { view.toolbarBounds = it.unclippedBoundsInRoot() },
                onKeyPositioned = { label, coordinates -> view.toolbarKeyBounds[label] = coordinates.unclippedBoundsInRoot() },
            )
        }
        if (shortcutsOpen) ShortcutsSheet(dismiss = { shortcutsOpen = false })
        pending?.let { lines ->
            val verb = if (lines.send) "Send" else "Paste"
            Or2Dialog(
                onDismiss = { pending = null }, title = "$verb ${pasteLineCount(lines.text)} lines?",
                confirm = {
                    TextAction(verb, {
                        // The one haptic of a confirmed send or paste (the composer's button did not buzz).
                        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                        pending = null
                        if (!lines.send) {
                            view.pasteText(lines.text)
                        } else if (view.sendLine(lines.text)) {
                            // Cleared only when it went out; after a drop the message stays in the composer. Only what
                            // was sent is cleared: an image's path that arrived under the dialog stays.
                            chrome.composerSent(lines.text)
                        }
                    }, modifier = Modifier.testTag(if (lines.send) "composer-send-confirm" else "paste-confirm"))
                },
                dismiss = { TextAction("Cancel", { pending = null }, color = Or2Colors.Text) },
            ) { Text("They will run as typed, one line at a time.") }
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
            Icon(Or2Icons.ArrowDown, null, Modifier.size(Or2Dimens.ScrollButtonGlyph), tint = Or2Colors.Accent)
        }
    }
}

// Position + measured size, not boundsInRoot(), so clipping cannot hide a partial key.
private fun LayoutCoordinates.unclippedBoundsInRoot(): RectF {
    val position = positionInRoot()
    return RectF(position.x, position.y, position.x + size.width, position.y + size.height)
}

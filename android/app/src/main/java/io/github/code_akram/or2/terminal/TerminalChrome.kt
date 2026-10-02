package io.github.code_akram.or2.terminal

import android.net.Uri
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.content.MediaType
import androidx.compose.foundation.content.consume
import androidx.compose.foundation.content.contentReceiver
import androidx.compose.foundation.content.hasMediaType
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.TextFieldState
import androidx.compose.foundation.text.input.clearText
import androidx.compose.foundation.text.input.setTextAndPlaceCursorAtEnd
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedback
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Type
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The chrome's own state, hoisted so a caller (the debug gallery) can start it open: the arrow pad,
 * the composer and the text typed into it. Production passes none and starts closed.
 */
class TerminalChromeState(padOpen: Boolean = false, composerOpen: Boolean = false, composerText: String = "") {
    var padOpen by mutableStateOf(padOpen)
    var composerOpen by mutableStateOf(composerOpen)

    /** The composer's field (a state-based text field: it takes a keyboard's images, [Composer]). */
    val composer = TextFieldState(composerText)

    /** The composer's text; setting it puts the caret at its end. */
    var composerText: String
        get() = composer.text.toString()
        set(value) = composer.setTextAndPlaceCursorAtEnd(value)

    /**
     * [sent] went out from the composer (a confirmed multi-line send): only that text is cleared. What
     * arrived since the send was asked (an uploaded image's path, while the confirmation was open) stays.
     */
    fun composerSent(sent: String) {
        composerText = composerAfterSend(composerText, sent)
    }
}

/**
 * What the composer holds once [sent] went out, when it holds [current] now: what was added after the sent
 * text (an image's path, inserted while a confirmation was open), without the space before it; all of
 * [current] when it no longer starts with what was sent (nothing is lost).
 */
fun composerAfterSend(current: String, sent: String): String =
    if (current.startsWith(sent)) current.substring(sent.length).trimStart() else current

/**
 * One key of the floating toolbar: a 30 dp tall rounded pill in `surface` (in a 40 dp touch box)
 * holding mono text or an outline icon in [tint]. [framed] false draws the bare icon (the composer and keyboard toggles). A
 * latched modifier ([latched]) draws in `accent`.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ToolKey(
    description: String, onClick: () -> Unit, modifier: Modifier = Modifier, label: String? = null,
    icon: ImageVector? = null, latched: Boolean? = null, framed: Boolean = true, active: Boolean = false,
    onLongClick: (() -> Unit)? = null, compact: Boolean = false, tint: Color = Or2Colors.Text,
) {
    val on = latched == true || active
    // The touch target is larger than the key drawn in it: 5 dp more above and below, 2 dp at the sides.
    Box(
        modifier.heightIn(min = if (compact) Or2Dimens.PadExtrasHeight else Or2Dimens.KeyTouch).widthIn(min = Or2Dimens.KeyWidth + 4.dp)
            .combinedClickable(role = Role.Button, onClick = onClick, onLongClick = onLongClick)
            .semantics {
                contentDescription = description
                if (latched != null) stateDescription = if (latched) "Armed for next key" else "Off"
            }
            .padding(2.dp),
        contentAlignment = Alignment.Center,
    ) {
        val color = if (on) Or2Colors.Accent else tint
        Box(
            Modifier.heightIn(min = if (compact) Or2Dimens.PadExtrasHeight - 4.dp else Or2Dimens.Key).widthIn(min = Or2Dimens.KeyWidth).clip(Or2Shapes.Key)
                .background(
                    when {
                        latched == true -> Or2Colors.AccentMuted
                        framed -> Or2Colors.Surface
                        else -> Color.Transparent
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (label != null) {
                Text(label, style = Or2Type.Key, color = color, maxLines = 1, softWrap = false, modifier = Modifier.padding(horizontal = 6.dp))
            } else if (icon != null) {
                Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = color)
            }
        }
    }
}

/** What the toolbar needs from the terminal; the screen owns the state, the toolbar only draws it. */
class ToolbarState(
    val ctrl: Boolean, val alt: Boolean, val selecting: Boolean, val padOpen: Boolean, val composerOpen: Boolean,
)

class ToolbarActions(
    val toggleCtrl: () -> Unit, val toggleAlt: () -> Unit, val escape: () -> Unit, val tab: () -> Unit,
    val togglePad: () -> Unit, val panes: () -> Unit, val paste: () -> Unit, val history: () -> Unit,
    val jumpToBottom: () -> Unit, val toggleComposer: () -> Unit, val toggleKeyboard: () -> Unit,
    val copy: () -> Unit, val clearSelection: () -> Unit,
)

/**
 * The floating key pill: `Ctrl`, `Alt`, `Esc`, `Tab` as mono text, then icon keys (arrow pad,
 * panes, paste, history), then, apart, the composer and keyboard toggles without key
 * backgrounds. It scrolls horizontally when it overflows.
 */
@Composable
fun KeyToolbar(
    state: ToolbarState, actions: ToolbarActions, modifier: Modifier = Modifier,
    onKeyPositioned: (String, LayoutCoordinates) -> Unit = { _, _ -> },
) {
    val haptics = LocalHapticFeedback.current
    fun tracked(label: String) = Modifier.testTag("key:$label").onGloballyPositioned { onKeyPositioned(label, it) }
    Row(
        modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp).clip(Or2Shapes.Pill)
            .background(Or2Colors.ToolbarPill).padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier.weight(1f).horizontalScroll(rememberScrollState()).testTag("toolbar-keys"),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (state.selecting) {
                ToolKey("Copy selection", actions.copy, tracked("Copy"), label = "Copy")
                ToolKey("Clear selection", actions.clearSelection, tracked("Clear"), label = "Clear")
            }
            ToolKey("Ctrl", { haptics.tick(!state.ctrl); actions.toggleCtrl() }, tracked("Ctrl"), label = "Ctrl", latched = state.ctrl)
            ToolKey("Esc", actions.escape, tracked("Esc"), label = "Esc")
            ToolKey("Tab", actions.tab, tracked("Tab"), label = "Tab")
            ToolKey("Arrow pad", actions.togglePad, tracked("Arrows"), icon = Or2Icons.Dpad, active = state.padOpen)
            ToolKey("Panes and sessions", actions.panes, tracked("Panes"), icon = Or2Icons.Sidebar)
            ToolKey("Paste", actions.paste, tracked("Paste"), icon = Or2Icons.Paste)
            ToolKey("History: page up, hold for the bottom", actions.history, tracked("History"), icon = Or2Icons.History,
                onLongClick = actions.jumpToBottom)
        }
        Spacer(Modifier.width(4.dp))
        ToolKey("Composer", actions.toggleComposer, tracked("Composer"), icon = Or2Icons.Chat, framed = false, active = state.composerOpen)
        ToolKey("Keyboard", actions.toggleKeyboard, tracked("Keyboard"), icon = Or2Icons.Keyboard, framed = false)
    }
}

private fun HapticFeedback.tick(on: Boolean) =
    performHapticFeedback(if (on) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff)

// --- arrow pad -------------------------------------------------------------------------------

/**
 * A key that sends on press and repeats while held (after 400 ms, every 60 ms). The tap
 * semantics send once, so accessibility services and tests can press it.
 */
@Composable
private fun RepeatKey(
    description: String, onKey: () -> Unit, modifier: Modifier = Modifier, icon: ImageVector? = null, label: String? = null,
    container: Color = Or2Colors.PadKey, tint: Color = Or2Colors.Accent, width: Dp = Or2Dimens.PadKey, height: Dp = Or2Dimens.PadKey,
) {
    val scope = rememberCoroutineScope()
    val current by rememberUpdatedState(onKey)
    // A filled key floats on the terminal by itself, so it carries its own blue hairline edge.
    val edge = if (container.alpha > 0f) Modifier.border(Dp.Hairline, Or2Colors.PadKeyEdge, Or2Shapes.Key) else Modifier
    Box(
        modifier.size(width, height).clip(Or2Shapes.Key).background(container).then(edge)
            .pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    current()
                    val repeat: Job = scope.launch {
                        delay(400)
                        while (true) {
                            current()
                            delay(60)
                        }
                    }
                    try {
                        waitForUpOrCancellation(PointerEventPass.Main)
                    } finally {
                        repeat.cancel()
                    }
                }
            }
            .semantics {
                contentDescription = description
                role = Role.Button
                onClick(label = description) { current(); true }
            },
        contentAlignment = Alignment.Center,
    ) {
        if (icon != null) Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = tint)
        else if (label != null) Text(label, style = Or2Type.Key, color = tint, maxLines = 1)
    }
}

/** The arrow keys' send helpers, one place for what each pad key means. */
class PadActions(
    val backspace: () -> Unit, val up: () -> Unit, val clearLine: () -> Unit, val left: () -> Unit,
    val enter: () -> Unit, val right: () -> Unit, val down: () -> Unit, val extra: (String) -> Unit,
)

/** Extra keys that no longer have a place in the toolbar: navigation and shell symbols. */
val NavigationKeys = listOf("Home", "End", "PgUp", "PgDn")
val SymbolKeys = listOf("/", "-", "|", "~", "_", "$", "&", "*", "{", "}", "(", ")", "[", "]", "=", ";", "'", "\"")
val ExtraKeys = NavigationKeys + SymbolKeys

/**
 * The floating 3x3 cluster above the toolbar: Backspace, Up, Clear-line / Left, Enter, Right /
 * Down; 40 dp squares with 12 dp radius that auto-repeat on hold, in the accent blue family so they never
 * blend into the terminal: a `padKey` fill (accent over the terminal background, opaque), an `accent`
 * glyph and a `padKeyEdge` hairline; Enter, the primary key, is filled `accent` with a `background`
 * glyph (the composer's send button). Nothing is drawn behind the cluster: the keys float over the
 * terminal, each opaque on its own. The toolbar's arrow-pad key opens and closes it. A scrolling row
 * below keeps the navigation and symbol keys one tap away: `accent` labels in a `background` pill with
 * the same blue hairline.
 */
@Composable
fun ArrowPad(actions: PadActions, alt: Boolean, toggleAlt: () -> Unit, modifier: Modifier = Modifier) {
    val haptics = LocalHapticFeedback.current
    Column(modifier.testTag("arrow-pad"), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap)) {
        Column(
            Modifier.testTag("pad-cluster"),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap)) {
                RepeatKey("Backspace", actions.backspace, Modifier.testTag("pad:Backspace"), icon = Or2Icons.Backspace)
                RepeatKey("Up", actions.up, Modifier.testTag("pad:Up"), icon = Or2Icons.ArrowUp)
                RepeatKey("Clear line", actions.clearLine, Modifier.testTag("pad:Clear"), icon = Or2Icons.Eraser)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap)) {
                RepeatKey("Left", actions.left, Modifier.testTag("pad:Left"), icon = Or2Icons.ArrowLeft)
                RepeatKey("Enter", actions.enter, Modifier.testTag("pad:Enter"), icon = Or2Icons.Enter,
                    container = Or2Colors.Accent, tint = Or2Colors.Background)
                RepeatKey("Right", actions.right, Modifier.testTag("pad:Right"), icon = Or2Icons.ArrowRight)
            }
            RepeatKey("Down", actions.down, Modifier.testTag("pad:Down"), icon = Or2Icons.ArrowDown)
        }
        val scroll = rememberScrollState()
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).height(Or2Dimens.PadExtrasHeight).clip(Or2Shapes.Pill)
                .background(Or2Colors.Background).border(Dp.Hairline, Or2Colors.PadKeyEdge, Or2Shapes.Pill)
                // An edge fade on each side that has more keys behind it: the row scrolls.
                .drawWithContent {
                    drawContent()
                    val fade = 20.dp.toPx()
                    if (scroll.canScrollForward) {
                        drawRect(
                            Brush.horizontalGradient(listOf(Color.Transparent, Or2Colors.Background), startX = size.width - fade, endX = size.width),
                            topLeft = Offset(size.width - fade, 0f), size = Size(fade, size.height),
                        )
                    }
                    if (scroll.canScrollBackward) {
                        drawRect(
                            Brush.horizontalGradient(listOf(Or2Colors.Background, Color.Transparent), startX = 0f, endX = fade),
                            size = Size(fade, size.height),
                        )
                    }
                }
                .horizontalScroll(scroll).padding(horizontal = 6.dp).testTag("pad-extras"),
            horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            ToolKey("Alt", { haptics.tick(!alt); toggleAlt() }, Modifier.testTag("key:Alt"), label = "Alt", latched = alt, framed = false, compact = true,
                tint = Or2Colors.Accent)
            NavigationKeys.forEach { key ->
                RepeatKey(key, { actions.extra(key) }, Modifier.testTag("extra:$key"), label = key,
                    width = Or2Dimens.PadExtraNavKeyWidth, height = Or2Dimens.PadExtrasHeight, container = Color.Transparent)
            }
            SymbolKeys.forEach { symbol ->
                RepeatKey(symbol, { actions.extra(symbol) }, Modifier.testTag("extra:$symbol"), label = symbol,
                    width = Or2Dimens.PadExtraKeyWidth, height = Or2Dimens.PadExtrasHeight, container = Color.Transparent)
            }
        }
    }
}

// --- composer --------------------------------------------------------------------------------

/**
 * The chat input: a rounded 20 dp card in `crust` (darker than the toolbar around it) docked above
 * the IME, in one row: an attach action when the terminal takes images ([attach]: the Photo Picker),
 * the mono text (a placeholder while empty), a close action and a circular send button,
 * `surfaceTrack` until there is text, then `accent`. Paste and the panes sheet are one tap away in
 * the toolbar beneath, so the card does not repeat them (it was two stacked rows of the same glyphs);
 * the keyboard pastes into the text itself. Sending writes the text plus Enter to the session; this
 * is the quick-reply path for a blocked agent. The text is the caller's ([state]), so a message that
 * could not be sent stays where it was typed: [send] returns whether it went out, and only then is
 * the text cleared. [canSend] is false while the session is not connected. An image a keyboard
 * commits into the text (a clipboard screenshot, a GIF keyboard) goes to [receiveImage] with its
 * content Uri, which returns whether it took it; without one the field takes no images.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Composer(
    placeholder: String, state: TextFieldState, canSend: Boolean, send: (String) -> Boolean,
    close: () -> Unit, modifier: Modifier = Modifier, attach: (() -> Unit)? = null, receiveImage: ((Uri) -> Boolean)? = null,
) {
    val focusRequester = remember { FocusRequester() }
    val haptics = LocalHapticFeedback.current
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    val receiver = receiveImage?.let { receive ->
        Modifier.contentReceiver { content ->
            if (!content.hasMediaType(MediaType.Image)) content
            else content.consume { item -> item.uri?.let(receive) == true }
        }
    } ?: Modifier
    val text = state.text
    // The actions stay at the bottom of a message that grows to several lines.
    Row(
        modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 6.dp).clip(Or2Shapes.Composer)
            .background(Or2Colors.Crust).padding(start = if (attach != null) 0.dp else 14.dp, end = 4.dp).testTag("composer"),
        verticalAlignment = Alignment.Bottom,
    ) {
        if (attach != null) ComposerAction(Or2Icons.Image, "Attach image", "composer-attach", attach, tint = Or2Colors.TextMuted)
        BasicTextField(
            state,
            Modifier.weight(1f).padding(vertical = (Or2Dimens.ComposerTouch - 18.dp) / 2).focusRequester(focusRequester)
                .then(receiver).testTag("composer-input"),
            textStyle = Or2Type.Composer.copy(color = Or2Colors.Text), cursorBrush = SolidColor(Or2Colors.Accent),
            lineLimits = TextFieldLineLimits.MultiLine(minHeightInLines = 1, maxHeightInLines = 5),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Default),
            decorator = { inner ->
                Box {
                    if (text.isEmpty()) Text(placeholder, style = Or2Type.Composer, color = Or2Colors.Placeholder, maxLines = 1)
                    inner()
                }
            },
        )
        ComposerAction(Or2Icons.Close, "Close composer", "composer-close", close)
        val ready = text.isNotBlank() && canSend
        Box(
            Modifier.size(Or2Dimens.ComposerTouch).padding((Or2Dimens.ComposerTouch - Or2Dimens.ComposerAction) / 2).clip(Or2Shapes.Circle)
                .background(if (ready) Or2Colors.Accent else Or2Colors.SurfaceTrack)
                .clickable(enabled = ready, role = Role.Button) {
                    haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                    // Only a message that went out is cleared: after a drop the text is still there to resend.
                    if (send(state.text.toString())) state.clearText()
                }
                .semantics { contentDescription = "Send" }.testTag("composer-send"),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Or2Icons.Send, null, Modifier.size(Or2Dimens.Icon), tint = if (ready) Or2Colors.Background else Or2Colors.TextMuted)
        }
    }
}

@Composable
private fun ComposerAction(icon: ImageVector, description: String, tag: String, onClick: () -> Unit, tint: Color = Or2Colors.Text) {
    Box(
        Modifier.size(Or2Dimens.ComposerTouch).clip(Or2Shapes.Circle).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description }.testTag(tag),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = tint)
    }
}

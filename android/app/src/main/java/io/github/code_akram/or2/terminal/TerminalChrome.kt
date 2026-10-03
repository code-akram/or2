package io.github.code_akram.or2.terminal

import android.net.Uri
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.TerminalKey
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
     * The visible screen's text ([TerminalView.screenText]) while a [TerminalScreen] shows this state, else
     * empty: a caller that hoists the state (the Terminals sheet's Copy screen) reads it.
     */
    var screenText: () -> String = { "" }
        internal set

    /**
     * [sent] went out from the composer (a confirmed multi-line send): only that text is cleared. What
     * arrived since the send was asked (an uploaded image's path, while the confirmation was open) stays.
     */
    fun composerSent(sent: String) {
        composerText = composerAfterSend(composerText, sent)
    }

    /** [text] typed into the composer at its cursor (over its selection, if any), the cursor after it: a toolbar key. */
    fun typeInComposer(text: String) {
        composer.edit {
            val start = selection.min
            replace(start, selection.max, text)
            selection = TextRange(start + text.length)
        }
    }
}

/**
 * What the composer holds once [sent] went out, when it holds [current] now: what was added after the sent
 * text (an image's path, inserted while a confirmation was open), without the space before it; all of
 * [current] when it no longer starts with what was sent (nothing is lost).
 */
fun composerAfterSend(current: String, sent: String): String =
    if (current.startsWith(sent)) current.substring(sent.length).trimStart() else current

/** The 2 dp on each side (5 dp above and below) by which a key's touch box exceeds the key drawn in it. */
private val KeyTouchInset = (Or2Dimens.KeyTouchWidth - Or2Dimens.KeyWidth) / 2

/**
 * One key of the floating toolbar: a [Or2Dimens.Key] tall rounded pill in `surface`, in a [Or2Dimens.KeyTouchWidth] x
 * [Or2Dimens.KeyTouch] touch box, holding mono text (padded [Or2Dimens.KeyLabelPadding] each side) or an outline icon
 * in [tint]. [framed] false draws the bare icon (the composer and keyboard toggles). A latched modifier ([latched])
 * draws in `accent` on `accentMuted`, an open pad or composer ([active]) in `accent`. [compact] is the pad extras' height.
 */
@Composable
fun ToolKey(
    description: String, onClick: () -> Unit, modifier: Modifier = Modifier, label: String? = null,
    icon: ImageVector? = null, latched: Boolean? = null, framed: Boolean = true, active: Boolean = false,
    compact: Boolean = false, tint: Color = Or2Colors.Text,
) {
    val on = latched == true || active
    Box(
        modifier.heightIn(min = if (compact) Or2Dimens.PadExtrasHeight else Or2Dimens.KeyTouch).widthIn(min = Or2Dimens.KeyTouchWidth)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics {
                contentDescription = description
                if (latched != null) stateDescription = if (latched) "Armed for next key" else "Off"
            }
            .padding(KeyTouchInset),
        contentAlignment = Alignment.Center,
    ) {
        val color = if (on) Or2Colors.Accent else tint
        Box(
            Modifier.heightIn(min = if (compact) Or2Dimens.PadExtrasHeight - KeyTouchInset * 2 else Or2Dimens.Key)
                .widthIn(min = Or2Dimens.KeyWidth).clip(Or2Shapes.Key)
                .background(
                    when {
                        latched == true -> Or2Colors.AccentMuted
                        framed -> Or2Colors.Surface
                        else -> Color.Transparent
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (label != null && icon != null) {
                // An icon before the label (`⇧Tab`): the glyph small, so the key stays a text key's width.
                Row(Modifier.padding(horizontal = Or2Dimens.KeyLabelPadding), verticalAlignment = Alignment.CenterVertically) {
                    Icon(icon, null, Modifier.size(KeyGlyph), tint = color)
                    Text(label, style = Or2Type.Key, color = color, maxLines = 1, softWrap = false)
                }
            } else if (label != null) {
                Text(
                    label, style = Or2Type.Key, color = color, maxLines = 1, softWrap = false,
                    modifier = Modifier.padding(horizontal = Or2Dimens.KeyLabelPadding),
                )
            } else if (icon != null) {
                Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = color)
            }
        }
    }
}

/** The icon drawn before a key's label (`⇧Tab`'s Shift arrow). */
val KeyGlyph = 12.dp

/** What the toolbar needs from the terminal; the screen owns the state, the toolbar only draws it. */
class ToolbarState(val ctrl: Boolean, val selecting: Boolean, val padOpen: Boolean, val composerOpen: Boolean, val shift: Boolean = false)

/**
 * The toolbar's keys: the test tag (`key:<tag>`), what an assistive service reads, and the mono label or the icon.
 * `⇧Tab` is Shift+Tab whatever is latched (Claude Code's mode cycle, which Gboard cannot send); `⇧` latches Shift for the
 * next key, as `Ctrl` latches Ctrl (Shift+Enter, Shift+arrows, a capital); `/` and `@` type into the composer at its
 * cursor while it is open, else into the terminal.
 */
enum class ToolbarKey(val tag: String, val description: String, val label: String? = null, val icon: ImageVector? = null) {
    COPY("Copy", "Copy selection", label = "Copy"),
    CLEAR("Clear", "Clear selection", label = "Clear"),
    CTRL("Ctrl", "Ctrl", label = "Ctrl"),
    ESC("Esc", "Esc", label = "Esc"),
    TAB("Tab", "Tab", label = "Tab"),
    ARROWS("Arrows", "Arrow pad", icon = Or2Icons.Dpad),
    PASTE("Paste", "Paste", icon = Or2Icons.Paste),
    SHIFT_TAB("ShiftTab", "Shift+Tab", label = "Tab", icon = Or2Icons.Shift),
    SHIFT("Shift", "Shift", icon = Or2Icons.Shift),
    SLASH("Slash", "Slash", label = "/"),
    AT("At", "At sign", label = "@"),
    COMPOSER("Composer", "Composer", icon = Or2Icons.Chat),
    KEYBOARD("Keyboard", "Keyboard", icon = Or2Icons.Keyboard),
}

/**
 * The keys before the toggles, in the owner's order (2026-10-03): `Ctrl`, `Esc`, `Tab`, `⇧Tab`, `⇧`, the arrow pad, Paste,
 * `/`, `@`. While text is selected Copy and Clear lead the row and the typing keys
 * (`⇧Tab`, `/`, `@`, which would clear the selection anyway) give way to them, so the row fits a phone either way.
 */
fun toolbarKeys(selecting: Boolean): List<ToolbarKey> = if (selecting) {
    listOf(ToolbarKey.COPY, ToolbarKey.CLEAR, ToolbarKey.CTRL, ToolbarKey.ESC, ToolbarKey.TAB, ToolbarKey.ARROWS, ToolbarKey.PASTE)
} else {
    listOf(
        ToolbarKey.CTRL, ToolbarKey.ESC, ToolbarKey.TAB, ToolbarKey.SHIFT_TAB, ToolbarKey.SHIFT, ToolbarKey.ARROWS,
        ToolbarKey.PASTE, ToolbarKey.SLASH, ToolbarKey.AT,
    )
}

/** The composer and keyboard toggles, apart at the end of the row, without key backgrounds. */
val ToolbarToggles = listOf(ToolbarKey.COMPOSER, ToolbarKey.KEYBOARD)

/**
 * The floating key pill: [toolbarKeys] (`Ctrl`, `Esc`, `Tab`, `⇧Tab`, `⇧`, the arrow pad, Paste, `/`, `@`), then
 * [ToolbarToggles], the spare width spread evenly between them all. It fits a 411 dp wide phone without scrolling (`KeyToolbarTest`); the scroll is only a fallback for
 * a large system font. [press] runs a key; a latched `Ctrl` or `⇧` draws in `accent` until it has been used for one key.
 */
@Composable
fun KeyToolbar(
    state: ToolbarState, press: (ToolbarKey) -> Unit, modifier: Modifier = Modifier,
    onKeyPositioned: (String, LayoutCoordinates) -> Unit = { _, _ -> },
) {
    val haptics = LocalHapticFeedback.current
    val key: @Composable (ToolbarKey) -> Unit = { key ->
        ToolKey(
            key.description,
            {
                if (key == ToolbarKey.CTRL) haptics.tick(!state.ctrl)
                if (key == ToolbarKey.SHIFT) haptics.tick(!state.shift)
                press(key)
            },
            Modifier.testTag("key:${key.tag}").onGloballyPositioned { onKeyPositioned(key.tag, it) },
            label = key.label, icon = key.icon, framed = key !in ToolbarToggles,
            latched = when (key) {
                ToolbarKey.CTRL -> state.ctrl
                ToolbarKey.SHIFT -> state.shift
                else -> null
            },
            active = (key == ToolbarKey.ARROWS && state.padOpen) || (key == ToolbarKey.COMPOSER && state.composerOpen),
        )
    }
    BoxWithConstraints(
        modifier.fillMaxWidth().padding(horizontal = Or2Dimens.ToolbarMargin, vertical = 6.dp).clip(Or2Shapes.Pill)
            .background(Or2Colors.ToolbarPill).padding(horizontal = Or2Dimens.ToolbarPadding),
    ) {
        val width = maxWidth
        // The spare width is spread evenly between every key and the toggles (owner, 2026-10-03: no gap before the
        // composer toggle); the scroll is only a fallback for a large system font.
        Row(Modifier.horizontalScroll(rememberScrollState()).testTag("toolbar-keys"), verticalAlignment = Alignment.CenterVertically) {
            Row(Modifier.widthIn(min = width), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                (toolbarKeys(state.selecting) + ToolbarToggles).forEach { key(it) }
            }
        }
    }
}

private fun HapticFeedback.tick(on: Boolean) =
    performHapticFeedback(if (on) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff)

// --- arrow pad -------------------------------------------------------------------------------

private val NoModifiers = KeyModifiers(false, false, false, false)

/** One key of the arrow pad's cluster: its test tag (`pad:<tag>`), its description, its glyph and what it sends. */
class PadKey(val tag: String, val description: String, val icon: ImageVector, val key: TerminalKey, val modifiers: KeyModifiers = NoModifiers)

/**
 * The cluster's rows: Backspace, Up, Clear-line (Ctrl-U, the shell's "clear the line before the cursor") / Left, Enter,
 * Right / Down.
 */
val PadRows = listOf(
    listOf(
        PadKey("Backspace", "Backspace", Or2Icons.Backspace, TerminalKey.Backspace),
        PadKey("Up", "Up", Or2Icons.ArrowUp, TerminalKey.ArrowUp),
        PadKey("Clear", "Clear line", Or2Icons.Eraser, TerminalKey.Character("u"), KeyModifiers(false, true, false, false)),
    ),
    listOf(
        PadKey("Left", "Left", Or2Icons.ArrowLeft, TerminalKey.ArrowLeft),
        PadKey("Enter", "Enter", Or2Icons.Enter, TerminalKey.Enter),
        PadKey("Right", "Right", Or2Icons.ArrowRight, TerminalKey.ArrowRight),
    ),
    listOf(PadKey("Down", "Down", Or2Icons.ArrowDown, TerminalKey.ArrowDown)),
)

/**
 * The extras row below the cluster, each label with what it sends: the navigation keys, then the shell symbols
 * (`/` is on the toolbar itself).
 */
val PadExtras: List<Pair<String, TerminalKey>> =
    listOf("Home" to TerminalKey.Home, "End" to TerminalKey.End, "PgUp" to TerminalKey.PageUp, "PgDn" to TerminalKey.PageDown) +
        listOf("-", "|", "~", "_", "$", "&", "*", "{", "}", "(", ")", "[", "]", "=", ";", "'", "\"").map { it to TerminalKey.Character(it) }

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

/**
 * The floating 3x3 cluster above the toolbar ([PadRows]); 40 dp squares with 12 dp radius that auto-repeat on hold, in
 * the accent blue family so they never blend into the terminal: a `padKey` fill (accent over the terminal background,
 * opaque), an `accent` glyph and a `padKeyEdge` hairline; Enter, the primary key, is filled `accent` with a
 * `background` glyph (the composer's send button). Nothing is drawn behind the cluster: the keys float over the
 * terminal, each opaque on its own. The toolbar's arrow-pad key opens and closes it. A scrolling row below keeps `Alt`
 * and [PadExtras] one tap away: `accent` labels in a `background` pill with the same blue hairline. Every key goes to
 * [send].
 */
@Composable
fun ArrowPad(send: (TerminalKey, KeyModifiers) -> Unit, alt: Boolean, toggleAlt: () -> Unit, modifier: Modifier = Modifier) {
    val haptics = LocalHapticFeedback.current
    Column(modifier.testTag("arrow-pad"), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap)) {
        Column(
            Modifier.testTag("pad-cluster"),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap),
        ) {
            PadRows.forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(Or2Dimens.PadGap)) {
                    row.forEach { pad ->
                        val enter = pad.key == TerminalKey.Enter
                        RepeatKey(
                            pad.description, { send(pad.key, pad.modifiers) }, Modifier.testTag("pad:${pad.tag}"), icon = pad.icon,
                            container = if (enter) Or2Colors.Accent else Or2Colors.PadKey, tint = if (enter) Or2Colors.Background else Or2Colors.Accent,
                        )
                    }
                }
            }
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
            PadExtras.forEach { (label, key) ->
                RepeatKey(label, { send(key, NoModifiers) }, Modifier.testTag("extra:$label"), label = label,
                    width = if (label.length > 1) Or2Dimens.PadExtraNavKeyWidth else Or2Dimens.PadExtraKeyWidth,
                    height = Or2Dimens.PadExtrasHeight, container = Color.Transparent)
            }
        }
    }
}

// --- composer --------------------------------------------------------------------------------

/**
 * The chat input: a rounded 20 dp card in `crust` (darker than the toolbar around it) docked above
 * the IME, in one row: an attach action when the terminal takes images ([attach]: the Photo Picker),
 * the mono text (a placeholder while empty), a close action and a circular send button,
 * `surfaceTrack` until there is text, then `accent`. Paste is one tap away in the toolbar beneath,
 * so the card does not repeat it; the keyboard pastes into the text itself. Sending writes the text
 * plus Enter to the session; this is the quick-reply path for a blocked agent. The text is the
 * caller's ([state]), so a message that could not be sent stays where it was typed: [send] returns
 * whether it went out (false too when it waits for a confirmation), and only then is the text cleared
 * and the send felt (one haptic per message: a confirmed one buzzes from its dialog). [canSend] is
 * false while the session is not connected. An image a keyboard commits into the text (a clipboard
 * screenshot, a GIF keyboard) goes to [receiveImage] with its content Uri, which returns whether it
 * took it; without one the field takes no images.
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
                    // Only a message that went out is cleared: after a drop the text is still there to resend.
                    if (send(state.text.toString())) {
                        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                        state.clearText()
                    }
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

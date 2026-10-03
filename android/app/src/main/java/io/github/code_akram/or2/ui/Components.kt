package io.github.code_akram.or2.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.remember
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.exclude
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.windowInsetsBottomHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

// --- top bar -------------------------------------------------------------------------------

/** A 44 dp round icon button (the platform grows its touch target to 48 dp); every icon in the app is one of these or sits in a row. */
@Composable
fun IconAction(
    icon: ImageVector, description: String, onClick: () -> Unit, modifier: Modifier = Modifier,
    tint: Color = Or2Colors.Text, enabled: Boolean = true,
) {
    Box(
        modifier.size(Or2Dimens.IconButton).clip(CircleShape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, description, Modifier.size(Or2Dimens.Icon), tint = if (enabled) tint else Or2Colors.TextMuted)
    }
}

/**
 * The navigation-bar inset a scrolling screen reserves at the end of its content, so lists run
 * edge to edge and scroll under the gesture bar instead of being cut above it. The keyboard
 * already covers the bar when it is up (the root applies the IME padding), so it is excluded.
 */
val Or2BottomInsets: WindowInsets @Composable get() = WindowInsets.navigationBars.exclude(WindowInsets.ime)

/** The end-of-content spacer that carries [Or2BottomInsets]. */
@Composable
fun BottomInsetSpacer(modifier: Modifier = Modifier) {
    Spacer(modifier.windowInsetsBottomHeight(Or2BottomInsets))
}

/**
 * Every screen's top bar, the one pattern (docs/ui.md, "Top bar"): exactly [Or2Dimens.TopBar] tall,
 * no fill, an optional back icon, a light 16 sp title and trailing icon buttons. Top-level screens
 * (Home, Inbox) pass no title and only [actions]. The icon buttons sit on the screen edges, so their
 * 20 dp glyphs land on the 12 dp gutter like the content below. It is placed above the screen's
 * scrolling content, never inside it, so it stays put; [scrolled] (see [scrolledUnder]) fades in a
 * hairline at its bottom edge once that content has scrolled under it.
 */
@Composable
fun TopBar(
    modifier: Modifier = Modifier, title: String? = null, back: (() -> Unit)? = null,
    backIcon: ImageVector = Or2Icons.Back, backDescription: String = "Back", scrolled: Boolean = false,
    actions: @Composable RowScope.() -> Unit = {},
) {
    val edge by animateFloatAsState(if (scrolled) 1f else 0f, tween(150), label = "top-bar-edge")
    Box(modifier.fillMaxWidth().height(Or2Dimens.TopBar).testTag("top-bar")) {
        Row(Modifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically) {
            if (back != null) IconAction(backIcon, backDescription, back, Modifier.testTag("top-back"))
            if (title != null) {
                Text(
                    title, style = Or2Type.TopBarTitle, color = Or2Colors.Text, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f).padding(start = if (back != null) 12.dp else Or2Dimens.Gutter, end = 8.dp)
                        .semantics { heading() },
                )
            } else {
                Spacer(Modifier.weight(1f))
            }
            actions()
        }
        // The scroll edge: a full-width hairline, only while content sits under the bar.
        if (edge > 0f) {
            Box(
                Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(1.dp).alpha(edge).background(Or2Colors.Divider)
                    .then(if (scrolled) Modifier.testTag("top-bar-edge") else Modifier),
            )
        }
    }
}

/** Whether a scrolling column's content sits under the top bar: anything but its very top ([ScrollState.value] > 0). */
fun isScrolledUnder(scrollOffset: Int): Boolean = scrollOffset > 0

/** Whether a lazy list's content sits under the top bar: its first item is gone or partly scrolled away. */
fun isScrolledUnder(firstVisibleItemIndex: Int, firstVisibleItemScrollOffset: Int): Boolean =
    firstVisibleItemIndex > 0 || firstVisibleItemScrollOffset > 0

/** [TopBar]'s `scrolled` for a screen whose content is a `verticalScroll` column on this state. */
@Composable
fun ScrollState.scrolledUnder(): Boolean {
    val state = this
    return remember(state) { derivedStateOf { isScrolledUnder(state.value) } }.value
}

/** [TopBar]'s `scrolled` for a screen whose content is a lazy list on this state. */
@Composable
fun LazyListState.scrolledUnder(): Boolean {
    val state = this
    return remember(state) { derivedStateOf { isScrolledUnder(state.firstVisibleItemIndex, state.firstVisibleItemScrollOffset) } }.value
}

// --- sections and cards --------------------------------------------------------------------

/** Muted UPPERCASE section header, optionally with a right-aligned hint. */
@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier, hint: String? = null, topGap: Dp = Or2Dimens.SectionGap) {
    Row(
        modifier.fillMaxWidth().padding(top = topGap, bottom = Or2Dimens.SectionHeaderGap),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text.uppercase(), style = Or2Type.SectionHeader, color = Or2Colors.TextMuted,
            modifier = Modifier.weight(1f).semantics { heading() },
        )
        if (hint != null) Text(hint, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
    }
}

/**
 * The fill of cards and grouped lists ([Or2Card], [GroupCard]) where they are: `surface` on a screen, `surfaceRaisedRow`
 * inside a sheet (an [Or2Sheet] provides it, so a card there is a step above the sheet's `surfaceRaised`, never below it).
 */
val LocalCardColor = staticCompositionLocalOf { Or2Colors.Surface }

/** A card with the 16 dp radius in [LocalCardColor] (unless [color] says otherwise); click and long-click are optional. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Or2Card(
    modifier: Modifier = Modifier, onClick: (() -> Unit)? = null, onLongClick: (() -> Unit)? = null,
    color: Color = LocalCardColor.current, border: BorderStroke? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val interactive = if (onClick != null || onLongClick != null) {
        Modifier.combinedClickable(role = Role.Button, onClick = onClick ?: {}, onLongClick = onLongClick)
    } else {
        Modifier
    }
    Column(
        modifier.fillMaxWidth().clip(Or2Shapes.Card).background(color)
            .then(if (border != null) Modifier.border(border, Or2Shapes.Card) else Modifier).then(interactive),
        content = content,
    )
}

/** Rows of one group: one card in [LocalCardColor] (unless [color] says otherwise); [GroupDivider] draws the hairlines between rows. */
@Composable
fun GroupCard(modifier: Modifier = Modifier, color: Color = LocalCardColor.current, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth().clip(Or2Shapes.Card).background(color), content = content)
}

/** The 1 px hairline between rows, inset to the text column. */
@Composable
fun GroupDivider(inset: Dp = Or2Dimens.Gutter) {
    Box(Modifier.fillMaxWidth().padding(start = inset).height(1.dp).background(Or2Colors.Divider))
}

/**
 * A grouped-list row: optional 20 dp muted outline icon, label, muted subtitle (mono for
 * machine text), trailing value and chevron. At least 44 dp tall, 56 dp with a subtitle.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ListRow(
    title: String, modifier: Modifier = Modifier, subtitle: String? = null, subtitleMono: Boolean = false,
    icon: ImageVector? = null, value: String? = null, valueMono: Boolean = false, chevron: Boolean = false,
    titleColor: Color = Or2Colors.Text, onClick: (() -> Unit)? = null, onLongClick: (() -> Unit)? = null,
    enabled: Boolean = true, trailing: @Composable (() -> Unit)? = null,
) {
    val interactive = if (onClick != null || onLongClick != null) {
        Modifier.combinedClickable(enabled = enabled, role = Role.Button, onClick = onClick ?: {}, onLongClick = onLongClick)
    } else {
        Modifier
    }
    Row(
        modifier.fillMaxWidth().heightIn(min = if (subtitle != null) Or2Dimens.RowMinSubtitle else Or2Dimens.RowMin)
            .then(interactive).padding(horizontal = Or2Dimens.Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Subtle)
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = Or2Type.RowLabel, color = if (enabled) titleColor else Or2Colors.TextMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) {
                Text(
                    subtitle, style = if (subtitleMono) Or2Type.MonoSmall else Or2Type.Secondary,
                    color = Or2Colors.TextMuted, maxLines = if (subtitleMono) 1 else 2, overflow = TextOverflow.Ellipsis,
                )
            }
        }
        if (value != null) {
            Spacer(Modifier.width(8.dp))
            Text(value, style = if (valueMono) Or2Type.Mono else Or2Type.Body, color = Or2Colors.TextMuted, maxLines = 1)
        }
        if (trailing != null) {
            Spacer(Modifier.width(8.dp))
            trailing()
        }
        if (chevron) {
            Spacer(Modifier.width(6.dp))
            Icon(Or2Icons.ChevronRight, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Subtle)
        }
    }
}

// --- status ----------------------------------------------------------------------------------

/** A coloured dot; [pulsing] dots (working agents) breathe between full and 70 % alpha, 1.6 s. */
@Composable
fun StatusDot(color: Color, modifier: Modifier = Modifier, pulsing: Boolean = false) {
    val alpha = if (pulsing) {
        val transition = rememberInfiniteTransition(label = "pulse")
        transition.animateFloat(
            initialValue = 1f, targetValue = 0.7f,
            animationSpec = infiniteRepeatable(tween(800, easing = FastOutSlowInEasing), RepeatMode.Reverse),
            label = "pulse-alpha",
        ).value
    } else {
        1f
    }
    Box(modifier.size(Or2Dimens.StatusDot).alpha(alpha).clip(CircleShape).background(color))
}

/**
 * The accent spinner shown in the icon slot while a host connects: pass the slot's size (the
 * server icon's) so it sits exactly where the icon does, running on a faint ring so it never reads
 * as a stray arc.
 */
@Composable
fun Spinner(modifier: Modifier = Modifier, size: Dp = 16.dp) {
    CircularProgressIndicator(
        modifier.size(size), color = Or2Colors.Accent, trackColor = Or2Colors.SurfaceTrack,
        strokeWidth = if (size >= 24.dp) 2.5.dp else 2.dp,
    )
}

/** A 28 dp pill in `surface` with an 8 dp coloured dot and a muted label, e.g. `● Needs attention: 1`. */
@Composable
fun StatusChip(label: String, dot: Color, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    Row(
        modifier.then(if (onClick != null) Modifier.minimumInteractiveComponentSize() else Modifier)
            .clip(Or2Shapes.Pill).background(Or2Colors.Surface)
            .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
            .heightIn(min = Or2Dimens.Chip).padding(horizontal = 12.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        StatusDot(dot)
        Spacer(Modifier.width(8.dp))
        Text(label, style = Or2Type.Chip, color = Or2Colors.TextMuted, maxLines = 1)
    }
}

/** A small mono pill, [content] text on [container]: the transport badge (`SSH`, `Mosh`) and its quiet-link form. */
@Composable
fun Badge(text: String, container: Color, content: Color, modifier: Modifier = Modifier) {
    Text(
        text, style = Or2Type.Pill, color = content, maxLines = 1,
        modifier = modifier.clip(Or2Shapes.Pill).background(container).padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

// --- inputs ----------------------------------------------------------------------------------

/**
 * A filled `surface` field with its label above in `text`; placeholder and, by default, the typed
 * text in mono. [errorText] is shown below in the danger colour. [tag] is the test tag of the
 * editable text itself (the modifier applies to the whole labelled field).
 */
@Composable
fun Or2Field(
    value: String, onValueChange: (String) -> Unit, modifier: Modifier = Modifier, label: String? = null,
    placeholder: String = "", mono: Boolean = true, enabled: Boolean = true, errorText: String? = null,
    singleLine: Boolean = true, keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
    visualTransformation: VisualTransformation = VisualTransformation.None, trailing: @Composable (() -> Unit)? = null,
    tag: String? = null,
) {
    Column(modifier) {
        val style: TextStyle = (if (mono) Or2Type.Mono else Or2Type.Body)
        BasicTextField(
            value, onValueChange, Modifier.fillMaxWidth().then(if (tag != null) Modifier.testTag(tag) else Modifier),
            enabled = enabled, singleLine = singleLine,
            textStyle = style.copy(color = if (enabled) Or2Colors.Text else Or2Colors.TextMuted),
            cursorBrush = SolidColor(Or2Colors.Accent), keyboardOptions = keyboardOptions, keyboardActions = keyboardActions,
            visualTransformation = visualTransformation,
            // The label is drawn inside the decoration, so the field's semantics merge it: an
            // assistive service reads "Name, edit box" instead of an unlabelled edit box.
            decorationBox = { inner ->
                Column {
                    if (label != null) {
                        Text(label, style = Or2Type.Body, color = Or2Colors.Text, modifier = Modifier.padding(bottom = 6.dp))
                    }
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = Or2Dimens.Field).clip(Or2Shapes.Field).background(Or2Colors.Surface)
                            .then(if (errorText != null) Modifier.border(1.dp, Or2Colors.Danger, Or2Shapes.Field) else Modifier)
                            .padding(horizontal = 12.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(Modifier.weight(1f)) {
                            if (value.isEmpty()) Text(placeholder, style = style, color = Or2Colors.Placeholder, maxLines = 1)
                            inner()
                        }
                        trailing?.let {
                            Spacer(Modifier.width(8.dp))
                            it()
                        }
                    }
                }
            },
        )
        if (errorText != null) {
            Text(errorText, style = Or2Type.Secondary, color = Or2Colors.Danger, modifier = Modifier.padding(start = 4.dp, top = 4.dp))
        }
    }
}

/** A `surfaceTrack` pill with the selected segment raised in `surface`; the others are muted. */
@Composable
fun Segmented(
    options: List<String>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier, tagPrefix: String = "segment",
) {
    Row(
        modifier.clip(Or2Shapes.Pill).background(Or2Colors.SurfaceTrack).selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        options.forEachIndexed { index, option ->
            val on = index == selected
            val fill by animateColorAsState(if (on) Or2Colors.Surface else Color.Transparent, label = "segment-fill")
            // The clickable spans the whole 32 dp track; the raised segment is drawn 3 dp inside it.
            Row(
                Modifier.heightIn(min = Or2Dimens.Segmented).clip(Or2Shapes.Pill)
                    .selectable(selected = on, role = Role.Tab, onClick = { onSelect(index) })
                    .semantics { contentDescription = option }.padding(3.dp)
                    .clip(Or2Shapes.Pill).background(fill)
                    .padding(horizontal = 12.dp).testTag("$tagPrefix:$index"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center,
            ) {
                Text(option, style = Or2Type.Body, color = if (on) Or2Colors.Text else Or2Colors.TextMuted, maxLines = 1)
            }
        }
    }
}

/** `accent` track with a `text` knob when on (a dark knob read as a hole in the track); `surfaceTrack` with a muted knob when off. */
@Composable
fun Or2Toggle(checked: Boolean, onCheckedChange: (Boolean) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val track by animateColorAsState(if (checked) Or2Colors.Accent else Or2Colors.SurfaceTrack, label = "toggle-track")
    val knob by animateColorAsState(if (checked) Or2Colors.Text else Or2Colors.TextMuted, label = "toggle-knob")
    val offset by animateDpAsState(if (checked) 18.dp else 0.dp, label = "toggle-offset")
    Box(
        modifier.size(width = 42.dp, height = 26.dp).clip(Or2Shapes.Pill).background(track)
            .toggleable(checked, enabled = enabled, role = Role.Switch, onValueChange = onCheckedChange)
            .padding(3.dp),
    ) {
        Box(Modifier.offset(x = offset).size(20.dp).clip(CircleShape).background(knob))
    }
}

// --- buttons ---------------------------------------------------------------------------------

/** The full-width pill at the end of a form: `accent`, 44 dp, text in `background`; disabled it is `accentMuted` with a light label. */
@Composable
fun PrimaryButton(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Box(
        modifier.fillMaxWidth().height(Or2Dimens.PrimaryButton).clip(Or2Shapes.Pill)
            .background(if (enabled) Or2Colors.Accent else Or2Colors.AccentMuted)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = Or2Type.Button, color = if (enabled) Or2Colors.Background else Or2Colors.OnAccentDisabled, maxLines = 1)
    }
}

/**
 * A secondary pill: `surfaceTrack` with text and an optional icon, trailing by default or leading with
 * [iconFirst] (the picker's prompt glyph before "Shell"). [compact] is the chip scale (28 dp, 12 sp) for
 * an action that sits inside a list row (Retry, Unlock).
 */
@Composable
fun PillButton(
    text: String, onClick: () -> Unit, modifier: Modifier = Modifier, icon: ImageVector? = null,
    enabled: Boolean = true, compact: Boolean = false, iconFirst: Boolean = false,
) {
    val tint = if (enabled) Or2Colors.Text else Or2Colors.TextMuted
    Row(
        modifier.heightIn(min = if (compact) Or2Dimens.Chip else Or2Dimens.Segmented + 4.dp).clip(Or2Shapes.Pill).background(Or2Colors.SurfaceTrack)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(start = if (icon != null && iconFirst) 12.dp else if (compact) 12.dp else 18.dp, end = if (compact) 12.dp else 18.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center,
    ) {
        if (icon != null && iconFirst) {
            Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = tint)
            Spacer(Modifier.width(4.dp))
        }
        Text(text, style = if (compact) Or2Type.Chip else Or2Type.Body, color = tint, maxLines = 1)
        if (icon != null && !iconFirst) {
            Spacer(Modifier.width(6.dp))
            Icon(icon, null, Modifier.size(Or2Dimens.Icon), tint = tint)
        }
    }
}

/** A text-only action for dialogs and sheet headers. */
@Composable
fun TextAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, color: Color = Or2Colors.Accent, enabled: Boolean = true) {
    Text(
        text, style = Or2Type.Body, color = if (enabled) color else Or2Colors.TextMuted, maxLines = 1,
        modifier = modifier.heightIn(min = 40.dp).clip(Or2Shapes.Pill)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 10.dp),
    )
}

// --- empty states, notices ---------------------------------------------------------------------

/** Centred 72 dp `surface` circle with a 32 dp outline icon, a 20 sp title and a muted explanation. */
@Composable
fun EmptyState(
    icon: ImageVector, title: String, body: String, modifier: Modifier = Modifier, action: @Composable (() -> Unit)? = null,
) {
    Column(modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(Or2Dimens.EmptyCircle).clip(CircleShape).background(Or2Colors.Surface), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(Or2Dimens.EmptyIcon), tint = Or2Colors.Subtle)
        }
        Spacer(Modifier.height(16.dp))
        Text(title, style = Or2Type.CardTitle, color = Or2Colors.Text, textAlign = TextAlign.Center)
        Spacer(Modifier.height(6.dp))
        Text(body, style = Or2Type.Body, color = Or2Colors.TextMuted, textAlign = TextAlign.Center)
        if (action != null) {
            Spacer(Modifier.height(16.dp))
            action()
        }
    }
}

/** A warning/call-to-action card: `attentionSurface` with a 1 px border, a leading warning glyph and a muted subtitle. */
@Composable
fun AttentionCard(title: String, subtitle: String?, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    Or2Card(
        modifier, onClick = onClick, color = Or2Colors.AttentionSurface,
        border = BorderStroke(1.dp, Or2Colors.AttentionBorder),
    ) {
        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Or2Icons.Warning, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Attention)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(title, style = Or2Type.RowLabel, color = Or2Colors.Text)
                if (subtitle != null) Text(subtitle, style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            }
            if (onClick != null) Icon(Or2Icons.ChevronRight, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Subtle)
        }
    }
}

// --- sheets and dialogs --------------------------------------------------------------------------

@Composable
fun SheetHandle(modifier: Modifier = Modifier) {
    Box(
        modifier.padding(vertical = 10.dp).size(Or2Dimens.SheetHandleWidth, Or2Dimens.SheetHandleHeight)
            .clip(Or2Shapes.Pill).background(Or2Colors.Subtle),
    )
}

/**
 * How far a sheet may rise: never over the status bar or a top cutout, and [Or2Dimens.SheetTopGap]
 * below them, so a full-height sheet still reads as a sheet (its rounded top edge shows on the scrim).
 */
val Or2SheetTopInsets: WindowInsets @Composable get() = WindowInsets.safeDrawing.only(WindowInsetsSides.Top)

/**
 * The insets a sheet's content keeps inside the sheet: the gesture bar (and the keyboard) at the
 * bottom and the side cutouts. The top is left out: [Or2SheetTopInsets] keeps the whole sheet below it.
 */
val Or2SheetContentInsets: WindowInsets
    @Composable get() = WindowInsets.safeDrawing.only(WindowInsetsSides.Bottom + WindowInsetsSides.Horizontal)

/**
 * A modal bottom sheet in `surfaceRaised`: 24 dp top radius, drag handle, optional title on the
 * left and a "Done" action on the right. Always opens fully.
 *
 * The sheet rule (docs/ui.md): Material's sheet window is edge to edge and lets a tall sheet's
 * surface rise to the very top of the screen, under the status bar, padding only its content. Here
 * the whole sheet stops [Or2Dimens.SheetTopGap] below the status bar (the padding sits outside the
 * surface), and the content keeps only the bottom and side insets. [scrollable] content scrolls
 * inside the sheet when it is taller than that room (the title row stays put), inside the sheet's own
 * body padding (the 12 dp gutter at the sides and below), so callers lay out only their rows; a sheet
 * that lays out its own scrolling list (the session picker) passes false and pads itself. Cards and
 * grouped lists in a sheet take `surfaceRaisedRow` ([LocalCardColor]). [modifier] applies to the sheet's body.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Or2Sheet(
    onDismiss: () -> Unit, modifier: Modifier = Modifier, title: String? = null, done: String? = "Done",
    scrollable: Boolean = true, content: @Composable ColumnScope.() -> Unit,
) {
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        // Outermost on the sheet's surface: the anchors are measured inside it, so "expanded" is its top.
        modifier = Modifier.windowInsetsPadding(Or2SheetTopInsets).padding(top = Or2Dimens.SheetTopGap),
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = Or2Shapes.Sheet, containerColor = Or2Colors.SurfaceRaised, contentColor = Or2Colors.Text,
        scrimColor = Or2Colors.Scrim, dragHandle = { SheetHandle(Modifier.testTag("sheet-handle")) },
        contentWindowInsets = { Or2SheetContentInsets },
    ) {
        // The caller's modifier (its test tag) goes on the body, where the sheet is drawn: on the surface it would sit
        // outside the sheet's drag offset and report the bounds of where the sheet would be fully open.
        CompositionLocalProvider(LocalCardColor provides Or2Colors.SurfaceRaisedRow) {
            Column(modifier) {
                if (title != null || done != null) {
                    Row(
                        Modifier.fillMaxWidth().padding(start = Or2Dimens.Gutter + 4.dp, end = Or2Dimens.Gutter - 4.dp, bottom = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            title.orEmpty(), style = Or2Type.ScreenTitle, color = Or2Colors.Text, maxLines = 1,
                            overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).semantics { heading() },
                        )
                        if (done != null) TextAction(done, onDismiss, color = Or2Colors.Text, modifier = Modifier.testTag("sheet-done"))
                    }
                }
                if (scrollable) {
                    Column(
                        Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())
                            .padding(start = Or2Dimens.Gutter, end = Or2Dimens.Gutter, bottom = Or2Dimens.Gutter).testTag("sheet-content"),
                        content = content,
                    )
                } else {
                    content()
                }
            }
        }
    }
}

/**
 * An alert dialog in the app's own style: a `surfaceRaised` card with the 12 dp gutter on the screen
 * sides and 16 dp inside (Material's AlertDialog keeps 24 dp everywhere and a 280 dp minimum width,
 * which looked like another design system), the title, the content and the buttons right-aligned
 * under it (wrapping onto a second line when they do not fit). The caller supplies the buttons with
 * [TextAction].
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun Or2Dialog(
    onDismiss: () -> Unit, title: String, confirm: @Composable () -> Unit, modifier: Modifier = Modifier,
    dismiss: @Composable (() -> Unit)? = null, titleColor: Color = Or2Colors.Text,
    titleStyle: TextStyle = Or2Type.CardTitle, content: @Composable () -> Unit,
) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Column(
            // A tall dialog (a changed host key with many old fingerprints) keeps clear of the status bar and
            // the cutouts whatever the dialog window does with the insets, and a section gap from the edges.
            modifier.windowInsetsPadding(WindowInsets.safeDrawing).padding(horizontal = Or2Dimens.Gutter, vertical = Or2Dimens.SectionGap)
                .widthIn(max = 480.dp).fillMaxWidth()
                .clip(Or2Shapes.Card).background(Or2Colors.SurfaceRaised).padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 8.dp),
        ) {
            Text(title, style = titleStyle, color = titleColor, modifier = Modifier.semantics { heading() })
            Spacer(Modifier.height(12.dp))
            // Dialog content is body text in `text` by default, as the Material dialog's text slot was.
            CompositionLocalProvider(LocalContentColor provides Or2Colors.Text, LocalTextStyle provides Or2Type.Body) {
                Box(Modifier.weight(1f, fill = false)) { content() }
            }
            Spacer(Modifier.height(4.dp))
            FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.End)) {
                dismiss?.invoke()
                confirm()
            }
        }
    }
}

/** A line of mono machine text in a `surfaceRaisedRow` block (fingerprints, public keys). */
@Composable
fun MonoBlock(text: String, modifier: Modifier = Modifier, color: Color = Or2Colors.Text) {
    Text(
        text, style = Or2Type.Mono, color = color,
        modifier = modifier.fillMaxWidth().clip(Or2Shapes.Field).background(Or2Colors.SurfaceRaisedRow).padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

/**
 * A call-to-action card with an icon tile: a mono accent kicker, a 15 sp title, a muted
 * explanation and a mono meta line (`~3 min · needs hostname + key`), then a chevron.
 */
@Composable
fun ActionCard(
    kicker: String, title: String, body: String, modifier: Modifier = Modifier, meta: String? = null,
    icon: ImageVector = Or2Icons.Server, onClick: (() -> Unit)? = null,
) {
    Or2Card(modifier, onClick = onClick) {
        Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(Or2Dimens.IconTile).clip(Or2Shapes.Tile).background(Or2Colors.AccentMuted),
                contentAlignment = Alignment.Center,
            ) {
                Icon(icon, null, Modifier.size(24.dp), tint = Or2Colors.Accent)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(kicker.uppercase(), style = Or2Type.Kicker, color = Or2Colors.Accent)
                Text(title, style = Or2Type.CardTitle, color = Or2Colors.Text)
                Text(body, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 2.dp))
                if (meta != null) Text(meta, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 6.dp))
            }
            Spacer(Modifier.width(8.dp))
            Icon(Or2Icons.ChevronRight, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Accent)
        }
    }
}

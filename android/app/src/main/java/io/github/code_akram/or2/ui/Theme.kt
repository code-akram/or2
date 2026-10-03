package io.github.code_akram.or2.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/**
 * The one place that defines how or2 looks (docs/ui.md): Catppuccin Mocha tokens, the type
 * scale, shapes and spacing. Screens use these names, never ad-hoc colours or sizes. Dark only.
 */
object Or2Colors {
    val Background = Color(0xFF181825)
    val BackgroundGlow = Color(0xFF1F2232)
    val Surface = Color(0xFF262636)
    val SurfaceRaised = Color(0xFF29293A)
    val SurfaceRaisedRow = Color(0xFF2E2E3F)
    val SurfaceTrack = Color(0xFF323345)
    val Divider = Color(0xFF333342)
    val Scrim = Color(0xFF101019).copy(alpha = 0.7f)
    val Text = Color(0xFFCDD6F4)

    /** Secondary text: Catppuccin overlay2, at least 4.5:1 on every surface it sits on (ThemeTest). */
    val TextMuted = Color(0xFF9399B2)

    /** Icons, chevrons, handles and idle dots: Catppuccin overlay0. Too dim for text. */
    val Subtle = Color(0xFF6C7086)

    /** Field placeholders: Catppuccin subtext0, one step above `textMuted` so a hint reads at the light weight (6.7:1 on `surface`). */
    val Placeholder = Color(0xFFA6ADC8)

    /** The label of a disabled primary button on `accentMuted`: `text` at 80 % composited (5.5:1; `textMuted` there was 3.9:1). */
    val OnAccentDisabled = Color(0xFFAEB7D4)
    val Accent = Color(0xFF89B4FA)
    val AccentMuted = Color(0xFF343B53)
    val Attention = Color(0xFFFAB387)
    val AttentionSurface = Color(0xFF30272B)
    val AttentionBorder = Color(0xFF6F4E3C)
    val Working = Accent
    val Done = Color(0xFFA6E3A1)
    val Idle = Subtle
    val Danger = Color(0xFFF38BA8)

    /** The Mosh transport badge: a saturated teal fill with `background` text (Moshi's own pill). */
    val Teal = Color(0xFF0FA89A)

    /** The composer card: Catppuccin crust, darker than the terminal and the key pills around it. */
    val Crust = Color(0xFF11111B)

    /** The terminal card follows the terminal's own default background (core/terminal.rs). */
    val TerminalBackground = Color(0xFF1E1E2E)

    /**
     * The terminal card's header (and any notice strip under it): halfway between the terminal's
     * background and `surface`, one slight tonal step above the grid, with a `crust` hairline below.
     */
    val TerminalHeader = Color(0xFF222232)

    /** The terminal header's drag handle: Catppuccin surface2, quieter than `subtle` so it never reads as a control bar. */
    val Handle = Color(0xFF585B70)

    /** The floating toolbar pill: `background` at ~85 %. */
    val ToolbarPill = Background.copy(alpha = 0.85f)

    /**
     * An arrow-pad key's fill: `accent` at 24 % composited over the terminal's background, opaque, so the
     * key reads blue against the terminal and no terminal text shows through it. Its glyph is `accent`
     * (4.7:1 here, ThemeTest).
     */
    val PadKey = Color(0xFF38425F)

    /** The hairline of the arrow-pad keys and of the extras pill: `accent` at 55 % (3:1 on the terminal, ThemeTest). */
    val PadKeyEdge = Accent.copy(alpha = 0.55f)
}

/**
 * The compact scale (docs/ui.md): the owner prefers a dense phone UI, in the same range as the
 * small 12 dp terminal font. Visual sizes are small; the platform still grows every clickable to a
 * 48 dp touch target (hit testing), and the primary controls are at least 40 dp tall as drawn.
 */
object Or2Dimens {
    val Gutter = 12.dp
    val SectionGap = 24.dp
    val SectionHeaderGap = 6.dp
    val IconButton = 44.dp
    val RowMin = 44.dp
    val RowMinSubtitle = 56.dp
    val Icon = 20.dp
    val IconTile = 44.dp
    val StatusDot = 8.dp
    val Fab = 48.dp
    val PrimaryButton = 44.dp
    val EmptyCircle = 72.dp
    val EmptyIcon = 32.dp
    val Field = 44.dp
    val Segmented = 32.dp
    val Chip = 28.dp

    /**
     * A toolbar key as drawn: a 30 dp pill, at least 30 dp wide (a text key is its label plus [KeyLabelPadding] on each
     * side), in a touch box [KeyTouchWidth] wide (4 dp wider than the key) and [KeyTouch] tall.
     */
    val Key = 30.dp
    val KeyWidth = 30.dp
    val KeyTouch = 40.dp
    val KeyTouchWidth = 34.dp
    val KeyLabelPadding = 6.dp

    /** The toolbar pill: [ToolbarMargin] from the screen's sides, its keys [ToolbarPadding] inside it, the toggles [ToolbarTogglesGap] apart. */
    val ToolbarMargin = 8.dp
    val ToolbarPadding = 6.dp
    val ToolbarTogglesGap = 4.dp
    val PadKey = 40.dp
    val PadGap = 6.dp
    val SheetHandleWidth = 32.dp
    val SheetHandleHeight = 4.dp

    /** Every screen's top bar is exactly this tall, title or not (docs/ui.md, "Top bar"). */
    val TopBar = 48.dp

    /** The least room between the status bar (or a cutout) and a full-height sheet's top edge. */
    val SheetTopGap = 12.dp

    /** The terminal header's drag handle: thin (28 x 3 dp), 4 dp from the card's top edge, inside the 36 dp header. */
    val TerminalHandleWidth = 28.dp
    val TerminalHandleHeight = 3.dp
    val TerminalHandleTop = 4.dp

    /** The terminal header row and its buttons: a small visible disc in a 36 dp tall box (the platform grows it to 48). */
    val HeaderRow = 36.dp
    val HeaderButtonDisc = 16.dp
    val HeaderButtonGlyph = 10.dp
    val HeaderButtonTouch = 36.dp

    /**
     * The two discs are a pair: 10 dp apart edge to edge. Each sits centred in a box that reaches halfway to its
     * neighbour (16 + 10 = 26 dp wide), so the boxes meet at the midpoint and a tap between the discs goes to the
     * nearer one; the platform grows each to a 48 dp target. The first disc's edge is the 12 dp gutter.
     */
    val HeaderDiscGap = 10.dp
    val HeaderButtonBox = HeaderButtonDisc + HeaderDiscGap

    /** The least room between the centred title and the side elements. */
    val HeaderTitleGap = 8.dp
    val TerminalInset = 4.dp

    /** A notice strip under a header (connecting, closed): one compact line that takes layout space. */
    val NoticeStrip = 28.dp
    val NoticeIcon = 14.dp

    /** The arrow pad's extras pill: 36 dp tall, symbol keys 28 dp wide, navigation keys as wide as their label. */
    val PadExtrasHeight = 36.dp
    val PadExtraKeyWidth = 28.dp
    val PadExtraNavKeyWidth = 38.dp

    /** The scroll-to-bottom chevron in its 28 dp disc. */
    val ScrollButtonGlyph = 16.dp

    /** The composer's actions are 36 dp circles (the send button's fill); icon-only actions have a 40 dp touch box. */
    val ComposerAction = 36.dp
    val ComposerTouch = 40.dp
    /** The connecting spinner sits in the server icon's own slot, so it is exactly the icon's size. */
    val Spinner = Icon
}

object Or2Shapes {
    val Card = RoundedCornerShape(16.dp)
    val Field = RoundedCornerShape(12.dp)
    val Tile = RoundedCornerShape(12.dp)
    val Thumbnail = RoundedCornerShape(12.dp)
    val Key = RoundedCornerShape(12.dp)
    val Composer = RoundedCornerShape(20.dp)
    val Pill = RoundedCornerShape(percent = 50)
    val Circle = CircleShape
    val Sheet = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp)
    val TerminalCard = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp)
}

/** Light and regular weights only (the terminal header's host is the one medium): emphasis comes from colour and size. */
object Or2Type {
    private val sans = FontFamily.Default
    private val mono get() = Or2Mono

    /** Sheet titles and the debug gallery menu. */
    val ScreenTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 20.sp, lineHeight = 26.sp)

    /** The title of a pushed screen (form, keys, host). */
    val TopBarTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 16.sp, lineHeight = 22.sp)
    val CardTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 15.sp, lineHeight = 20.sp)
    val RowLabel = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 14.sp, lineHeight = 19.sp)
    val Body = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 13.sp, lineHeight = 18.sp)
    val Secondary = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 12.sp, lineHeight = 16.sp)
    val Button = TextStyle(fontFamily = sans, fontWeight = FontWeight.Normal, fontSize = 14.sp, lineHeight = 19.sp)
    val SectionHeader = TextStyle(
        fontFamily = sans, fontWeight = FontWeight.Normal, fontSize = 10.5.sp, lineHeight = 14.sp, letterSpacing = 0.08.em,
    )
    val Kicker = TextStyle(
        fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 14.sp, letterSpacing = 0.15.em,
    )
    val Mono = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 12.sp, lineHeight = 16.sp)
    val MonoSmall = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 10.5.sp, lineHeight = 16.sp)

    /** The Easy pair code `7KQ4-M2XD-9PTM`: the one large element of the app, because it is read off the phone and typed on a host. */
    val PairCode = TextStyle(
        fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 24.sp, lineHeight = 30.sp, letterSpacing = 0.04.em,
    )

    /**
     * The terminal header's centred title: the host in the sans at 12 sp, the one medium weight in the app (a short
     * label that must win a glance over the terminal), then the target in mono 11 sp ([HeaderTarget]), muted.
     */
    val HeaderTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Medium, fontSize = 12.sp, lineHeight = 16.sp)
    val HeaderTarget = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 16.sp)

    /** Overlay pills on thumbnails and the terminal header's transport badge: small, they sit on the terminal. */
    val Pill = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 14.sp)
    val Chip = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 12.sp, lineHeight = 16.sp)

    /** Toolbar and pad keys (`Ctrl`, `Esc`, `Tab`). */
    val Key = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 12.sp, lineHeight = 16.sp)

    /** The composer's input text. */
    val Composer = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 18.sp)
}

private val Or2ColorScheme = darkColorScheme(
    primary = Or2Colors.Accent,
    onPrimary = Or2Colors.Background,
    primaryContainer = Or2Colors.AccentMuted,
    onPrimaryContainer = Or2Colors.Accent,
    secondary = Or2Colors.Accent,
    onSecondary = Or2Colors.Background,
    background = Or2Colors.Background,
    onBackground = Or2Colors.Text,
    surface = Or2Colors.Surface,
    onSurface = Or2Colors.Text,
    surfaceVariant = Or2Colors.SurfaceTrack,
    onSurfaceVariant = Or2Colors.TextMuted,
    surfaceContainer = Or2Colors.SurfaceRaised,
    surfaceContainerHigh = Or2Colors.SurfaceRaised,
    surfaceContainerHighest = Or2Colors.SurfaceTrack,
    outline = Or2Colors.Divider,
    outlineVariant = Or2Colors.Divider,
    error = Or2Colors.Danger,
    onError = Or2Colors.Background,
    scrim = Or2Colors.Scrim,
)

private val Or2Typography = Typography(
    displayLarge = Or2Type.ScreenTitle, displayMedium = Or2Type.ScreenTitle, displaySmall = Or2Type.ScreenTitle,
    headlineLarge = Or2Type.ScreenTitle, headlineMedium = Or2Type.ScreenTitle, headlineSmall = Or2Type.CardTitle,
    titleLarge = Or2Type.ScreenTitle, titleMedium = Or2Type.CardTitle, titleSmall = Or2Type.RowLabel,
    bodyLarge = Or2Type.Body, bodyMedium = Or2Type.Body, bodySmall = Or2Type.Secondary,
    labelLarge = Or2Type.Button, labelMedium = Or2Type.Secondary, labelSmall = Or2Type.SectionHeader,
)

/** The only theme: dark Catppuccin Mocha with Material components mapped onto the tokens. */
@Composable
fun Or2Theme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = Or2ColorScheme, typography = Or2Typography, content = content)
}

/**
 * The screen background with its soft glows: one behind the top bar and one in the bottom-right
 * corner, always fading to the plain background, never ending in an edge.
 */
fun Modifier.or2Background(): Modifier = background(Or2Colors.Background).drawBehind {
    // A wide, flat ellipse: the glow is gone by the time lists start below the top bar, so opaque
    // sticky headers never meet a visible edge.
    val squash = 0.3f
    withTransform({ scale(1f, squash, Offset(size.width / 2, 0f)) }) {
        drawRect(
            Brush.radialGradient(
                listOf(Or2Colors.BackgroundGlow, Color.Transparent),
                center = Offset(size.width * 0.5f, 0f), radius = size.width * 0.95f,
            ),
            size = Size(size.width, size.height / squash),
        )
    }
    drawCornerGlow()
}

/** The bottom-right glow on its own, for surfaces that cover the top one (opaque list areas). */
fun DrawScope.drawCornerGlow() {
    drawRect(Brush.radialGradient(
        listOf(Or2Colors.BackgroundGlow, Color.Transparent),
        center = Offset(size.width, size.height), radius = size.width * 0.65f,
    ))
}

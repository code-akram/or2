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

    /** The floating toolbar pill: `background` at ~85 %. */
    val ToolbarPill = Background.copy(alpha = 0.85f)
}

object Or2Dimens {
    val Gutter = 16.dp
    val SectionGap = 32.dp
    val SectionHeaderGap = 8.dp
    val IconButton = 48.dp
    val RowMin = 56.dp
    val RowMinSubtitle = 72.dp
    val Icon = 24.dp
    val IconTile = 56.dp
    val StatusDot = 10.dp
    val Fab = 60.dp
    val PrimaryButton = 56.dp
    val EmptyCircle = 96.dp
    val EmptyIcon = 40.dp
    val Field = 48.dp
    val Key = 44.dp
    val KeyWidth = 36.dp
    val KeyTouch = 52.dp
    val PadKey = 56.dp
    val SheetHandleWidth = 36.dp
    val SheetHandleHeight = 4.dp
    val TerminalHandleWidth = 44.dp

    /** Terminal header buttons: a small visible disc inside a 48 dp touch target. */
    val HeaderButtonDisc = 18.dp
    val HeaderButtonGlyph = 12.dp
    val HeaderButtonTouch = 48.dp
    val TerminalInset = 4.dp

    /** The arrow pad's extras pill: 44 dp tall, symbol keys 32 dp wide, navigation keys as wide as their label. */
    val PadExtrasHeight = 44.dp
    val PadExtraKeyWidth = 32.dp
    val PadExtraNavKeyWidth = 44.dp
    val PadHandleTouch = 32.dp
    val ComposerAction = 48.dp
    val Spinner = 28.dp
}

object Or2Shapes {
    val Card = RoundedCornerShape(20.dp)
    val Field = RoundedCornerShape(16.dp)
    val Tile = RoundedCornerShape(16.dp)
    val Thumbnail = RoundedCornerShape(16.dp)
    val Key = RoundedCornerShape(16.dp)
    val Composer = RoundedCornerShape(24.dp)
    val Pill = RoundedCornerShape(percent = 50)
    val Circle = CircleShape
    val Sheet = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)
    val TerminalCard = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)
}

/** Light and regular weights only: emphasis comes from colour and size. */
object Or2Type {
    private val sans = FontFamily.Default
    private val mono get() = Or2Mono

    /** Sheet titles and the debug gallery menu. */
    val ScreenTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 28.sp, lineHeight = 34.sp)

    /** The title of a pushed screen (form, keys, host): Moshi's is about 20 sp light. */
    val TopBarTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 20.sp, lineHeight = 26.sp)
    val CardTitle = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 20.sp, lineHeight = 26.sp)
    val RowLabel = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 18.sp, lineHeight = 24.sp)
    val Body = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 16.sp, lineHeight = 22.sp)
    val Secondary = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 15.sp, lineHeight = 20.sp)
    val Button = TextStyle(fontFamily = sans, fontWeight = FontWeight.Normal, fontSize = 18.sp, lineHeight = 24.sp)
    val SectionHeader = TextStyle(
        fontFamily = sans, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 18.sp, letterSpacing = 0.08.em,
    )
    val Kicker = TextStyle(
        fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 12.sp, lineHeight = 16.sp, letterSpacing = 0.15.em,
    )
    val Mono = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 15.sp, lineHeight = 20.sp)
    val MonoSmall = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 18.sp)
    val MonoLarge = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 17.sp, lineHeight = 24.sp)
    val Badge = TextStyle(fontFamily = sans, fontWeight = FontWeight.Normal, fontSize = 13.sp, lineHeight = 16.sp)

    /** Overlay pills on thumbnails: small, because the whole terminal is shown scaled down. */
    val Pill = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 14.sp)
    val Chip = TextStyle(fontFamily = sans, fontWeight = FontWeight.Light, fontSize = 14.sp, lineHeight = 18.sp)

    /** Toolbar and pad keys (`Ctrl`, `Esc`, `Tab`). */
    val Key = TextStyle(fontFamily = mono, fontWeight = FontWeight.Normal, fontSize = 14.sp, lineHeight = 18.sp)
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

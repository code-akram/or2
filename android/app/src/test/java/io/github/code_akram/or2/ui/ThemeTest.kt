package io.github.code_akram.or2.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The tokens are the ones docs/ui.md names; a change here is a change to the design system. */
class ThemeTest {
    private fun channel(value: Float) = Math.round(value * 255)

    private fun hex(color: Color) = "#%06X".format((channel(color.red) shl 16) or (channel(color.green) shl 8) or channel(color.blue))

    @Test
    fun theCatppuccinMochaTokensMatchTheUiDocument() {
        assertEquals("#181825", hex(Or2Colors.Background))
        assertEquals("#1F2232", hex(Or2Colors.BackgroundGlow))
        assertEquals("#262636", hex(Or2Colors.Surface))
        assertEquals("#29293A", hex(Or2Colors.SurfaceRaised))
        assertEquals("#2E2E3F", hex(Or2Colors.SurfaceRaisedRow))
        assertEquals("#323345", hex(Or2Colors.SurfaceTrack))
        assertEquals("#333342", hex(Or2Colors.Divider))
        assertEquals("#CDD6F4", hex(Or2Colors.Text))
        assertEquals("#9399B2", hex(Or2Colors.TextMuted))
        assertEquals("#6C7086", hex(Or2Colors.Subtle))
        assertEquals("#A6ADC8", hex(Or2Colors.Placeholder))
        assertEquals("#AEB7D4", hex(Or2Colors.OnAccentDisabled))
        assertEquals("#0FA89A", hex(Or2Colors.Teal))
        assertEquals("#11111B", hex(Or2Colors.Crust))
        assertEquals("#89B4FA", hex(Or2Colors.Accent))
        assertEquals("#343B53", hex(Or2Colors.AccentMuted))
        assertEquals("#FAB387", hex(Or2Colors.Attention))
        assertEquals("#30272B", hex(Or2Colors.AttentionSurface))
        assertEquals("#6F4E3C", hex(Or2Colors.AttentionBorder))
        assertEquals("#A6E3A1", hex(Or2Colors.Done))
        assertEquals("#F38BA8", hex(Or2Colors.Danger))
        assertEquals(Or2Colors.Accent, Or2Colors.Working)
        assertEquals(Or2Colors.Subtle, Or2Colors.Idle)
        // The terminal card follows the terminal's default background (core/terminal.rs).
        assertEquals("#1E1E2E", hex(Or2Colors.TerminalBackground))
        assertTrue(Or2Colors.Scrim.alpha in 0.65f..0.75f)
    }

    @Test
    fun theTypeScaleUsesLightWeightsAndTheDocumentedSizes() {
        // The compact scale (docs/ui.md).
        assertEquals(20f, Or2Type.ScreenTitle.fontSize.value, 0f)
        assertEquals(16f, Or2Type.TopBarTitle.fontSize.value, 0f)
        assertEquals(15f, Or2Type.CardTitle.fontSize.value, 0f)
        assertEquals(14f, Or2Type.RowLabel.fontSize.value, 0f)
        assertEquals(13f, Or2Type.Body.fontSize.value, 0f)
        assertEquals(12f, Or2Type.Secondary.fontSize.value, 0f)
        assertEquals(10.5f, Or2Type.SectionHeader.fontSize.value, 0f)
        assertEquals(11f, Or2Type.Kicker.fontSize.value, 0f)
        assertEquals(12f, Or2Type.Mono.fontSize.value, 0f)
        assertEquals(10.5f, Or2Type.MonoSmall.fontSize.value, 0f)
        assertEquals(11f, Or2Type.Pill.fontSize.value, 0f)
        // The Easy pair code is the one large element; nothing else is above the 20 sp screen title.
        assertEquals(24f, Or2Type.PairCode.fontSize.value, 0f)
        // Mono lines sit on a 16 sp grid, so stacked notes (title, status, herdr note) keep an even rhythm.
        assertEquals(16f, Or2Type.MonoSmall.lineHeight.value, 0f)
        assertEquals(13f, Or2Type.Composer.fontSize.value, 0f)
        assertEquals(0.08f, Or2Type.SectionHeader.letterSpacing.value, 0f)
        assertEquals(0.15f, Or2Type.Kicker.letterSpacing.value, 0f)
        // Large titles are light, never bold.
        listOf(Or2Type.ScreenTitle, Or2Type.TopBarTitle, Or2Type.CardTitle, Or2Type.RowLabel, Or2Type.Body).forEach {
            assertEquals(300, it.fontWeight!!.weight)
        }
    }

    private fun luminance(color: Color): Double {
        fun lin(c: Float) = if (c <= 0.03928f) c / 12.92 else Math.pow(((c + 0.055) / 1.055), 2.4)
        return 0.2126 * lin(color.red) + 0.7152 * lin(color.green) + 0.0722 * lin(color.blue)
    }

    private fun contrast(foreground: Color, background: Color): Double {
        val a = luminance(foreground)
        val b = luminance(background)
        return (maxOf(a, b) + 0.05) / (minOf(a, b) + 0.05)
    }

    @Test
    fun mutedTextMeetsWcagAaOnEverySurfaceItSitsOn() {
        listOf(
            Or2Colors.Background, Or2Colors.Surface, Or2Colors.SurfaceRaised, Or2Colors.SurfaceRaisedRow,
            Or2Colors.TerminalBackground, Or2Colors.Crust, Or2Colors.AttentionSurface,
        ).forEach {
            assertTrue("muted text on ${hex(it)} is ${contrast(Or2Colors.TextMuted, it)}:1", contrast(Or2Colors.TextMuted, it) >= 4.5)
        }
        // Primary text, and the 70 % text of fingerprints and the SSH pill, stay comfortably legible too.
        listOf(Or2Colors.Surface, Or2Colors.SurfaceRaisedRow, Or2Colors.SurfaceTrack).forEach {
            assertTrue(contrast(Or2Colors.Text, it) >= 7.0)
            assertTrue("70 % text on ${hex(it)}", contrast(Or2Colors.Text.copy(alpha = 0.7f).compositeOver(it), it) >= 4.5)
        }
        // The icon grey is for icons and handles only: it is allowed to be dimmer than text.
        assertTrue(contrast(Or2Colors.Subtle, Or2Colors.Surface) < 4.5)
    }

    @Test
    fun theWindowBackgroundResourceMirrorsTheThemeBackground() {
        // The window shows this colour before Compose draws; a copy that drifted would flash.
        val xml = java.io.File("src/main/res/values/themes.xml").readText()
        val value = Regex("""name="or2_window_background">#([0-9A-Fa-f]{8})<""").find(xml)!!.groupValues[1]
        assertEquals("FF" + hex(Or2Colors.Background).removePrefix("#"), value.uppercase())
    }

    @Test
    fun placeholdersAndDisabledPrimaryLabelsAreLegible() {
        // The placeholder is one step brighter than muted text on every field and sheet fill ...
        listOf(Or2Colors.Surface, Or2Colors.SurfaceRaisedRow, Or2Colors.SurfaceTrack).forEach {
            assertTrue("placeholder on ${hex(it)}", contrast(Or2Colors.Placeholder, it) >= 4.5)
            assertTrue(contrast(Or2Colors.Placeholder, it) > contrast(Or2Colors.TextMuted, it))
        }
        // ... but stays dimmer than typed text, so a hint never reads as a value.
        assertTrue(contrast(Or2Colors.Placeholder, Or2Colors.Surface) < contrast(Or2Colors.Text, Or2Colors.Surface))
        // A disabled primary button's label on its accentMuted fill (muted text there was 3.9:1) is the text colour at 80 %.
        assertTrue(contrast(Or2Colors.OnAccentDisabled, Or2Colors.AccentMuted) >= 4.5)
        assertEquals(hex(Or2Colors.Text.copy(alpha = 0.8f).compositeOver(Or2Colors.AccentMuted)), hex(Or2Colors.OnAccentDisabled))
        // Kicker lines (key algorithms in the host-key dialogs) are full accent, not a dimmed one.
        assertTrue(contrast(Or2Colors.Accent, Or2Colors.SurfaceRaised) >= 4.5)
    }

    @Test
    fun arrowPadKeysAreBlueAndStandOutFromTheTerminal() {
        // Owner feedback on v0.1.1: `surface` keys on the terminal were hard to tell apart from it.
        assertEquals("#38425F", hex(Or2Colors.PadKey))
        assertEquals(1f, Or2Colors.PadKey.alpha, 0f) // Opaque: terminal text never shows through a key.
        assertEquals(hex(Or2Colors.Accent.copy(alpha = 0.24f).compositeOver(Or2Colors.TerminalBackground)), hex(Or2Colors.PadKey))
        assertEquals(Or2Colors.Accent.copy(alpha = 0.55f), Or2Colors.PadKeyEdge)
        // Blue, not grey: blue clearly dominates, as it does in `accent`.
        assertTrue(Or2Colors.PadKey.blue - Or2Colors.PadKey.red > 0.1f && Or2Colors.PadKey.blue - Or2Colors.PadKey.green > 0.08f)
        // The fill stands out from the terminal (and from the `surface` it replaced), and its hairline more so.
        assertTrue(contrast(Or2Colors.PadKey, Or2Colors.TerminalBackground) >= 1.5)
        assertTrue(contrast(Or2Colors.PadKey, Or2Colors.TerminalBackground) > 1.4 * contrast(Or2Colors.Surface, Or2Colors.TerminalBackground))
        val edge = Or2Colors.PadKeyEdge.compositeOver(Or2Colors.TerminalBackground)
        assertTrue(contrast(edge, Or2Colors.TerminalBackground) >= 3.0)
        // Glyphs: `accent` on a key is at least 4.5:1; Enter (the primary key) is `background` on `accent`.
        assertTrue("accent on the pad key is ${contrast(Or2Colors.Accent, Or2Colors.PadKey)}:1", contrast(Or2Colors.Accent, Or2Colors.PadKey) >= 4.5)
        assertTrue(contrast(Or2Colors.Background, Or2Colors.Accent) >= 7.0)
        assertTrue(luminance(Or2Colors.Accent) > luminance(Or2Colors.PadKey))
        // The extras pill: `accent` labels on `background`, its hairline as the keys'.
        assertTrue(contrast(Or2Colors.Accent, Or2Colors.Background) >= 7.0)
        assertTrue(contrast(Or2Colors.PadKeyEdge.compositeOver(Or2Colors.Background), Or2Colors.Background) >= 3.0)
        // A latched Alt (`accentMuted` fill) keeps its `accent` label legible.
        assertTrue(contrast(Or2Colors.Accent, Or2Colors.AccentMuted) >= 4.5)
    }

    @Test
    fun theHeaderDiscsAreAPairWhoseBoxesMeetHalfway() {
        // Visible discs 16 dp with a 10 dp glyph, 10 dp apart; each box reaches halfway to the other disc.
        assertEquals(16.dp, Or2Dimens.HeaderButtonDisc)
        assertEquals(10.dp, Or2Dimens.HeaderButtonGlyph)
        assertEquals(10.dp, Or2Dimens.HeaderDiscGap)
        assertEquals(Or2Dimens.HeaderButtonDisc + Or2Dimens.HeaderDiscGap, Or2Dimens.HeaderButtonBox)
        // The handle is thin and fits above the title inside the 36 dp header.
        assertEquals(28.dp, Or2Dimens.TerminalHandleWidth)
        assertEquals(3.dp, Or2Dimens.TerminalHandleHeight)
        assertEquals(4.dp, Or2Dimens.TerminalHandleTop)
        assertEquals(28.dp, Or2Dimens.NoticeStrip)
        // The connecting spinner takes the server icon's slot exactly.
        assertEquals(Or2Dimens.Icon, Or2Dimens.Spinner)
    }

    @Test
    fun theTerminalHeaderIsASlightTonalStepThatKeepsItsTextLegible() {
        assertEquals("#222232", hex(Or2Colors.TerminalHeader))
        assertEquals("#585B70", hex(Or2Colors.Handle))
        // One step above the grid, below `surface`.
        assertTrue(luminance(Or2Colors.TerminalHeader) > luminance(Or2Colors.TerminalBackground))
        assertTrue(luminance(Or2Colors.TerminalHeader) < luminance(Or2Colors.Surface))
        // The muted target, the notice strip's text and the stale label read on it.
        assertTrue(contrast(Or2Colors.TextMuted, Or2Colors.TerminalHeader) >= 4.5)
        assertTrue(contrast(Or2Colors.Attention, Or2Colors.TerminalHeader) >= 4.5)
        assertTrue(contrast(Or2Colors.Accent, Or2Colors.TerminalHeader) >= 4.5)
        // The handle is quieter than the icon grey: a hint, not a control bar.
        assertTrue(contrast(Or2Colors.Handle, Or2Colors.TerminalHeader) < contrast(Or2Colors.Subtle, Or2Colors.TerminalHeader))
        // The host is the one medium weight in the type scale.
        assertEquals(500, Or2Type.HeaderTitle.fontWeight!!.weight)
        assertEquals(12f, Or2Type.HeaderTitle.fontSize.value, 0f)
        assertEquals(11f, Or2Type.HeaderTarget.fontSize.value, 0f)
    }

    @Test
    fun layoutConstantsFollowTheDocument() {
        // The compact scale (docs/ui.md): dense by default, small drawn sizes, 40 dp or more for primary controls.
        assertEquals(12.dp, Or2Dimens.Gutter)
        assertEquals(24.dp, Or2Dimens.SectionGap)
        assertEquals(6.dp, Or2Dimens.SectionHeaderGap)
        assertEquals(44.dp, Or2Dimens.RowMin)
        assertEquals(56.dp, Or2Dimens.RowMinSubtitle)
        assertEquals(48.dp, Or2Dimens.Fab)
        assertEquals(44.dp, Or2Dimens.PrimaryButton)
        assertEquals(44.dp, Or2Dimens.Field)
        assertEquals(32.dp, Or2Dimens.Segmented)
        assertEquals(28.dp, Or2Dimens.Chip)
        assertEquals(20.dp, Or2Dimens.Icon)
        assertEquals(36.dp, Or2Dimens.HeaderRow)
        assertEquals(30.dp, Or2Dimens.Key)
        assertEquals(40.dp, Or2Dimens.KeyTouch)
        // A toolbar key's touch box is 34 x 40 dp around the 30 dp key drawn in it.
        assertEquals(34.dp, Or2Dimens.KeyTouchWidth)
        assertEquals(6.dp, Or2Dimens.KeyLabelPadding)
        assertEquals(8.dp, Or2Dimens.ToolbarMargin)
        assertEquals(6.dp, Or2Dimens.ToolbarPadding)
        assertEquals(40.dp, Or2Dimens.PadKey)
        assertEquals(6.dp, Or2Dimens.PadGap)
        assertEquals(36.dp, Or2Dimens.PadExtrasHeight)
        assertEquals(36.dp, Or2Dimens.ComposerAction)
        assertEquals(72.dp, Or2Dimens.EmptyCircle)
        assertEquals(32.dp, Or2Dimens.EmptyIcon)
        assertEquals(8.dp, Or2Dimens.StatusDot)
    }
}

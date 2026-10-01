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
        assertEquals(28f, Or2Type.ScreenTitle.fontSize.value, 0f)
        assertEquals(20f, Or2Type.TopBarTitle.fontSize.value, 0f)
        assertEquals(20f, Or2Type.CardTitle.fontSize.value, 0f)
        assertEquals(18f, Or2Type.RowLabel.fontSize.value, 0f)
        assertEquals(16f, Or2Type.Body.fontSize.value, 0f)
        assertEquals(15f, Or2Type.Secondary.fontSize.value, 0f)
        assertEquals(13f, Or2Type.SectionHeader.fontSize.value, 0f)
        assertEquals(12f, Or2Type.Kicker.fontSize.value, 0f)
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
    fun layoutConstantsFollowTheDocument() {
        assertEquals(16.dp, Or2Dimens.Gutter)
        assertEquals(32.dp, Or2Dimens.SectionGap)
        assertEquals(56.dp, Or2Dimens.RowMin)
        assertEquals(72.dp, Or2Dimens.RowMinSubtitle)
        assertEquals(60.dp, Or2Dimens.Fab)
        assertEquals(56.dp, Or2Dimens.PrimaryButton)
        assertEquals(48.dp, Or2Dimens.HeaderButtonTouch)
        assertEquals(18.dp, Or2Dimens.HeaderButtonDisc)
        assertEquals(44.dp, Or2Dimens.PadExtrasHeight)
        assertEquals(96.dp, Or2Dimens.EmptyCircle)
        assertEquals(40.dp, Or2Dimens.EmptyIcon)
        assertEquals(10.dp, Or2Dimens.StatusDot)
        assertEquals(56.dp, Or2Dimens.PadKey)
    }
}

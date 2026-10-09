package io.github.code_akram.or2.terminal

import android.content.Context
import android.content.res.AssetManager
import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.fonts.Font
import android.graphics.fonts.FontFamily
import android.graphics.fonts.FontStyle
import android.icu.lang.UCharacter
import android.icu.lang.UProperty
import android.icu.text.BreakIterator
import java.io.File
import java.util.Locale
import kotlin.math.abs
import kotlin.math.ceil

/**
 * The terminal's typefaces, indexed by `Typeface` style (NORMAL, BOLD, ITALIC, BOLD_ITALIC): the bundled fonts
 * (`assets/fonts/`, see THIRD_PARTY_NOTICES.md), loaded once per process. JetBrains Mono in its four real styles
 * comes first; DejaVu Sans Mono then covers the symbols it lacks (Claude Code's `✻✢✽✳` spinner, `✔✘`, `☐☑`) at
 * the same advance; the system's own fallbacks (emoji, CJK, braille) come last. Box drawing, blocks and the
 * other sprites are drawn, never typeset ([isSprite]).
 */
internal fun terminalTypefaces(context: Context): Array<Typeface> = TerminalFonts.load(context.applicationContext.assets)

private object TerminalFonts {
    @Volatile private var loaded: Array<Typeface>? = null

    fun load(assets: AssetManager): Array<Typeface> = loaded ?: synchronized(this) {
        loaded ?: (runCatching { bundled(assets) }.getOrNull() ?: system()).also { loaded = it }
    }

    private fun bundled(assets: AssetManager): Array<Typeface> {
        fun font(file: String, bold: Boolean, italic: Boolean) = Font.Builder(assets, "fonts/$file")
            .setWeight(if (bold) FontStyle.FONT_WEIGHT_BOLD else FontStyle.FONT_WEIGHT_NORMAL)
            .setSlant(if (italic) FontStyle.FONT_SLANT_ITALIC else FontStyle.FONT_SLANT_UPRIGHT)
            .build()
        val primary = FontFamily.Builder(font("JetBrainsMono-Regular.ttf", bold = false, italic = false))
            .addFont(font("JetBrainsMono-Bold.ttf", bold = true, italic = false))
            .addFont(font("JetBrainsMono-Italic.ttf", bold = false, italic = true))
            .addFont(font("JetBrainsMono-BoldItalic.ttf", bold = true, italic = true))
            .build()
        val symbols = FontFamily.Builder(font("DejaVuSansMono.ttf", bold = false, italic = false)).build()
        return Array(4) { style ->
            Typeface.CustomFallbackBuilder(primary)
                .addCustomFallback(symbols)
                .setSystemFallback("monospace")
                .setStyle(FontStyle(
                    if (style and Typeface.BOLD != 0) FontStyle.FONT_WEIGHT_BOLD else FontStyle.FONT_WEIGHT_NORMAL,
                    if (style and Typeface.ITALIC != 0) FontStyle.FONT_SLANT_ITALIC else FontStyle.FONT_SLANT_UPRIGHT,
                ))
                .build()
        }
    }

    /** Without the bundled fonts: the genuine system monospace file, bypassing OEM font customization. */
    private fun system(): Array<Typeface> {
        val paint = Paint()
        val file = File("/system/fonts/DroidSansMono.ttf")
        val candidate = if (file.isFile) runCatching { Typeface.Builder(file).build() }.getOrNull() else null
        // Some OEMs override the monospace alias, in which case centring each glyph still keeps it aligned within
        // the fixed grid. The single file has no bold face: only italic is requested from it, bold is synthesized.
        val base = candidate?.takeIf { paint.typeface = it; paint.hasMonospacedAdvances() } ?: Typeface.MONOSPACE
        return Array(4) { style ->
            val italic = style and Typeface.ITALIC != 0
            val bold = style and Typeface.BOLD != 0 && base == Typeface.MONOSPACE
            when {
                bold && italic -> Typeface.create(base, Typeface.BOLD_ITALIC)
                bold -> Typeface.create(base, Typeface.BOLD)
                italic -> Typeface.create(base, Typeface.ITALIC)
                else -> base
            }
        }
    }
}

/**
 * A cell's height: the font's line spacing (ascent to descent, as Ghostty and other terminals use), not the bounds of
 * its tallest glyph, which for JetBrains Mono would make rows about 14% taller. Glyphs reaching past it are clipped
 * to their cell, as before.
 */
internal fun Paint.cellHeight(): Float = ceil(fontMetrics.descent - fontMetrics.ascent)

/** Where text sits in a cell of [cellHeight]: the font's ascent below its top. */
internal fun Paint.cellBaseline(): Float = -fontMetrics.ascent

internal fun Paint.hasMonospacedAdvances(): Boolean {
    val width = measureText("M")
    return width > 0 && listOf("i", "W").all { abs(measureText(it) - width) <= width * .001f }
}

/** Only local IME text needs width estimation; remote cells already carry Rust's widths. */
internal fun compositionCells(text: String): Int {
    val iterator = BreakIterator.getCharacterInstance(Locale.ROOT)
    iterator.setText(text)
    var cells = 0
    var start = iterator.first()
    var end = iterator.next()
    while (end != BreakIterator.DONE) {
        val cluster = text.substring(start, end)
        val textPresentation = '\uFE0E' in cluster
        var wide = '\uFE0F' in cluster || '\u20E3' in cluster
        var offset = 0
        while (offset < cluster.length) {
            val codePoint = cluster.codePointAt(offset)
            val asianWidth = UCharacter.getIntPropertyValue(codePoint, UProperty.EAST_ASIAN_WIDTH)
            wide = wide || asianWidth == UCharacter.EastAsianWidth.WIDE ||
                asianWidth == UCharacter.EastAsianWidth.FULLWIDTH ||
                (!textPresentation && UCharacter.hasBinaryProperty(codePoint, UProperty.EMOJI_PRESENTATION))
            offset += Character.charCount(codePoint)
        }
        cells += if (wide) 2 else 1
        start = end
        end = iterator.next()
    }
    return cells
}

package io.github.code_akram.or2.terminal

import android.graphics.Paint
import android.graphics.Typeface
import android.icu.lang.UCharacter
import android.icu.lang.UProperty
import android.icu.text.BreakIterator
import java.io.File
import java.util.Locale
import kotlin.math.abs

/** Bypass OEM font customization when a genuine system monospace file is available. */
internal fun terminalTypeface(paint: Paint): Typeface {
    val file = File("/system/fonts/DroidSansMono.ttf")
    val candidate = if (file.isFile) runCatching { Typeface.Builder(file).build() }.getOrNull() else null
    if (candidate != null) {
        paint.typeface = candidate
        if (paint.hasMonospacedAdvances()) return candidate
    }
    paint.typeface = Typeface.MONOSPACE
    // TerminalView validates the final choice too. Some OEMs override this alias, in which
    // case centring each glyph still keeps it aligned within the fixed grid.
    return Typeface.MONOSPACE
}

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

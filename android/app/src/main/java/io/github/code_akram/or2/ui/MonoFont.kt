package io.github.code_akram.or2.ui

import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.ui.text.font.FontFamily
import java.io.File
import kotlin.math.abs

/**
 * The monospace face for machine text. Some OEMs remap the generic `monospace` alias to a
 * proportional face, so, as the terminal does, prefer the genuine system monospace file when it
 * validates as having fixed advances, and fall back to the generic family otherwise.
 */
val Or2Mono: FontFamily by lazy {
    val file = File("/system/fonts/DroidSansMono.ttf")
    val candidate = if (file.isFile) runCatching { Typeface.Builder(file).build() }.getOrNull() else null
    val monospaced = candidate != null && Paint().apply { typeface = candidate; textSize = 100f }.let { paint ->
        val width = paint.measureText("M")
        width > 0 && listOf("i", "W").all { abs(paint.measureText(it) - width) <= width * .001f }
    }
    if (monospaced) FontFamily(candidate!!) else FontFamily.Monospace
}

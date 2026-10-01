package io.github.code_akram.or2.terminal

import android.content.Context

/**
 * The terminal's font size, remembered per device. The default is small and dense (12 dp: about
 * 55 columns on a 1440 px-wide phone in portrait, whatever the system font size); pinch zooms
 * between [MinFontSp] and [MaxFontSp]. Rust has no storage, so this lives in the app's private
 * preferences. The names say "Sp" for history; the unit is density-independent pixels.
 */
object TerminalPrefs {
    const val DefaultFontSp = 12f
    const val MinFontSp = 6f
    const val MaxFontSp = 28f
    private const val DEFAULT_FILE = "or2-terminal"

    /** The preferences file; device tests point it elsewhere so they never touch the owner's size. */
    internal var file = DEFAULT_FILE
    private const val FONT_SP = "font_dp"

    fun load(context: Context): Float = try {
        context.getSharedPreferences(file, Context.MODE_PRIVATE).getFloat(FONT_SP, DefaultFontSp)
            .takeIf { it in MinFontSp..MaxFontSp } ?: DefaultFontSp
    } catch (_: RuntimeException) {
        DefaultFontSp
    }

    fun save(context: Context, sp: Float) {
        try {
            context.getSharedPreferences(file, Context.MODE_PRIVATE).edit().putFloat(FONT_SP, sp).apply()
        } catch (_: RuntimeException) {
            // A size that cannot be remembered is still applied for this session.
        }
    }
}

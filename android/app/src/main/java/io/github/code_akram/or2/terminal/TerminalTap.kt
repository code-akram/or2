package io.github.code_akram.or2.terminal

/** What a single tap on the terminal does (docs/ui.md, "Taps on the terminal"). */
enum class TapAction {
    /**
     * A selection is shown: the tap clears it, never follows a link or clicks under it (without mouse
     * tracking the keyboard also opens, as before).
     */
    CLEAR_SELECTION,

    /** A link under the tap opens. */
    OPEN_LINK,

    /** The program tracks the mouse: a left click at the tapped cell (`Session.mouse_click`); no keyboard. */
    CLICK,

    /** Anything else: the keyboard shows (today's behaviour). */
    SHOW_KEYBOARD,
}

/**
 * The tap's action, in precedence order: a selection is cleared first; a link opens; with mouse tracking
 * on the tap is a click; otherwise the keyboard shows. [link] is whether a link lies under the tapped
 * cell, [mouseTracking] the terminal's `TerminalModes.mouse_tracking`.
 */
fun tapAction(selecting: Boolean, link: Boolean, mouseTracking: Boolean): TapAction = when {
    selecting -> TapAction.CLEAR_SELECTION
    link -> TapAction.OPEN_LINK
    mouseTracking -> TapAction.CLICK
    else -> TapAction.SHOW_KEYBOARD
}

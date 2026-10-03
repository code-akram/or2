package io.github.code_akram.or2.terminal

import android.view.KeyEvent

/** An app shortcut on an attached hardware keyboard (never the IME's keys). */
sealed interface TerminalShortcut {
    /** Ctrl+Shift+1..9: the [index]-th open terminal (0-based), in Home's order. */
    data class SwitchTo(val index: Int) : TerminalShortcut

    /** Ctrl+Shift+W: close this terminal. */
    data object Close : TerminalShortcut

    /** Ctrl+Shift+V: paste the clipboard into the terminal. */
    data object Paste : TerminalShortcut

    /** Ctrl+Shift+C: copy the selection. */
    data object Copy : TerminalShortcut

    /** Ctrl+Shift+Enter: open the composer (closes it when it is open). */
    data object Composer : TerminalShortcut

    /** Ctrl+Shift+/: the shortcuts sheet. */
    data object Help : TerminalShortcut
}

/**
 * The shortcut for a key press, matched by key code rather than by the character it types (layouts
 * differ: Shift+/ is `?` on one, something else on another). Exactly Ctrl and Shift must be held;
 * with Alt or Meta too it is an ordinary key for the terminal, as is every key without a shortcut.
 */
fun terminalShortcut(keyCode: Int, ctrl: Boolean, shift: Boolean, alt: Boolean, meta: Boolean): TerminalShortcut? {
    if (!ctrl || !shift || alt || meta) return null
    return when (keyCode) {
        in KeyEvent.KEYCODE_1..KeyEvent.KEYCODE_9 -> TerminalShortcut.SwitchTo(keyCode - KeyEvent.KEYCODE_1)
        KeyEvent.KEYCODE_W -> TerminalShortcut.Close
        KeyEvent.KEYCODE_V -> TerminalShortcut.Paste
        KeyEvent.KEYCODE_C -> TerminalShortcut.Copy
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> TerminalShortcut.Composer
        KeyEvent.KEYCODE_SLASH -> TerminalShortcut.Help
        else -> null
    }
}

/**
 * Runs [fire] for an attached keyboard's app shortcut and says whether [event] is part of one: its press, repeats and
 * release are all consumed, so none of it reaches the terminal (or the composer's text), and only the first press fires.
 * Never an IME's key. A shortcut in [passes] is left to the focused editor (the composer's own paste and copy). The
 * terminal view and the composer both handle their keys through it.
 */
fun consumeShortcut(event: KeyEvent, fire: (TerminalShortcut) -> Unit, passes: Set<TerminalShortcut> = emptySet()): Boolean {
    if (event.flags and KeyEvent.FLAG_SOFT_KEYBOARD != 0) return false
    val shortcut = terminalShortcut(event.keyCode, event.isCtrlPressed, event.isShiftPressed, event.isAltPressed, event.isMetaPressed)
    if (shortcut == null || shortcut in passes) return false
    if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) fire(shortcut)
    return true
}

/**
 * The touch gestures, first in the gestures-and-shortcuts sheet, as the view handles them ([tapAction], the
 * [SwipeClassifier], pinch zoom): the gesture, then what it does.
 */
val TouchRows = listOf(
    "Tap" to "Keyboard, or a click where the program uses the mouse",
    "Tap a link" to "Open it",
    "Long press" to "Select a word; drag to extend, then Copy",
    "Drag ↑ / ↓" to "Scroll back through the output",
    "Swipe ← / →" to "Next / previous window or tab",
    "Two fingers ← / →" to "Pane to the left / right",
    "Two fingers ↑ / ↓" to "Next / previous session or workspace",
    "Pinch" to "Text size",
)

/** Under the touch rows: which terminals the swipes move ([swipeNav]). */
const val SWIPES_NOTE = "Swipes move tmux and herdr; a shell ignores them."

/** The hardware-keyboard shortcuts, after the touch rows: keys, then what they do. */
val KeyboardShortcutRows = listOf(
    "Ctrl+Shift+1…9" to "Switch to terminal 1 to 9",
    "Ctrl+Shift+W" to "Close the terminal",
    "Ctrl+Shift+V" to "Paste",
    "Ctrl+Shift+C" to "Copy the selection",
    "Ctrl+Shift+Enter" to "Composer",
    "Ctrl+Shift+/" to "Gestures & shortcuts",
)

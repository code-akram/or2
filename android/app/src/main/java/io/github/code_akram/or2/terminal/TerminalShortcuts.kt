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

/** What the shortcuts sheet lists: keys, then what they do. */
val KeyboardShortcutRows = listOf(
    "Ctrl+Shift+1…9" to "Switch to terminal 1 to 9",
    "Ctrl+Shift+W" to "Close the terminal",
    "Ctrl+Shift+V" to "Paste",
    "Ctrl+Shift+C" to "Copy the selection",
    "Ctrl+Shift+Enter" to "Composer",
    "Ctrl+Shift+/" to "These shortcuts",
)

/** The gestures, for the same sheet (tmux and herdr terminals; a shell ignores the swipes). */
val GestureRows = listOf(
    "Swipe ← / →" to "Next / previous window or tab",
    "Two fingers ← / →" to "Pane to the left / right",
    "Two fingers ↑ / ↓" to "Next / previous session or workspace",
    "Pinch" to "Text size",
)

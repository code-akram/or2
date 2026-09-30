package io.github.code_akram.or2.terminal

import android.view.KeyEvent
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.TerminalKey

/** Active IME composition stays local; explicit commit or finish sends it exactly once. */
class TerminalInput(
    private val sendText: (String) -> Unit,
    private val sendKey: (KeyInput) -> Unit,
    private val changed: () -> Unit = {},
) {
    var composing = ""
        private set
    var ctrl = false
        private set
    var alt = false
        private set

    fun toggleCtrl() { ctrl = !ctrl; changed() }
    fun toggleAlt() { alt = !alt; changed() }
    fun compose(text: String) { composing = text; changed() }
    fun discardComposition() { composing = ""; changed() }
    fun finishComposition() { commit(composing) }

    /** Clipboard paste is literal text, not a typed character with sticky modifiers. */
    fun paste(text: String) {
        if (text.isEmpty()) return
        discardComposition()
        sendText(text)
    }

    fun commit(text: String) {
        discardComposition()
        if (text.isEmpty()) return
        if (!ctrl && !alt) {
            sendText(text)
        } else {
            // Sticky modifiers affect exactly the next character, not an entire pasted string.
            val end = Character.charCount(text.codePointAt(0))
            val first = text.substring(0, end)
            key(when (first) {
                "\n", "\r" -> TerminalKey.Enter
                "\t" -> TerminalKey.Tab
                else -> TerminalKey.Character(first)
            })
            if (end < text.length) sendText(text.substring(end))
        }
    }

    fun key(key: TerminalKey, modifiers: KeyModifiers = KeyModifiers(false, false, false, false)) {
        val input = KeyInput(key, modifiers.copy(ctrl = modifiers.ctrl || ctrl, alt = modifiers.alt || alt))
        ctrl = false
        alt = false
        changed()
        sendKey(input)
    }

    fun deleteSurrounding(before: Int, after: Int): Boolean {
        if (before < 0 || after < 0) return false
        repeat(before) { key(TerminalKey.Backspace) }
        repeat(after) { key(TerminalKey.Delete) }
        return true
    }
}

/** Logical lines, including a trailing empty line; CRLF is a single line break. */
fun pasteLineCount(text: String): Int {
    if (text.isEmpty()) return 0
    var lines = 1
    text.forEachIndexed { index, char ->
        if (char == '\r' || (char == '\n' && (index == 0 || text[index - 1] != '\r'))) lines++
    }
    return lines
}

fun pasteNeedsConfirmation(text: String): Boolean = pasteLineCount(text) > 1

/** Android constants are inlined so the mapping is also exercised in ordinary JVM tests. */
fun terminalKey(keyCode: Int, unicode: Int): TerminalKey? = when (keyCode) {
    KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> TerminalKey.Enter
    KeyEvent.KEYCODE_TAB -> TerminalKey.Tab
    KeyEvent.KEYCODE_DEL -> TerminalKey.Backspace
    KeyEvent.KEYCODE_FORWARD_DEL -> TerminalKey.Delete
    KeyEvent.KEYCODE_ESCAPE -> TerminalKey.Escape
    KeyEvent.KEYCODE_INSERT -> TerminalKey.Insert
    KeyEvent.KEYCODE_MOVE_HOME -> TerminalKey.Home
    KeyEvent.KEYCODE_MOVE_END -> TerminalKey.End
    KeyEvent.KEYCODE_PAGE_UP -> TerminalKey.PageUp
    KeyEvent.KEYCODE_PAGE_DOWN -> TerminalKey.PageDown
    KeyEvent.KEYCODE_DPAD_UP -> TerminalKey.ArrowUp
    KeyEvent.KEYCODE_DPAD_DOWN -> TerminalKey.ArrowDown
    KeyEvent.KEYCODE_DPAD_LEFT -> TerminalKey.ArrowLeft
    KeyEvent.KEYCODE_DPAD_RIGHT -> TerminalKey.ArrowRight
    in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12 -> TerminalKey.Function((keyCode - KeyEvent.KEYCODE_F1 + 1).toUByte())
    else -> if (unicode > 0 && Character.isValidCodePoint(unicode) && !Character.isISOControl(unicode)) {
        TerminalKey.Character(String(Character.toChars(unicode)))
    } else null
}

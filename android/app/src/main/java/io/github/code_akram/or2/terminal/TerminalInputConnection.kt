package io.github.code_akram.or2.terminal

import android.text.Editable
import android.text.SpannableStringBuilder
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection

internal class TerminalInputConnection(private val terminal: TerminalView) : BaseInputConnection(terminal, false) {
    private val composition = SpannableStringBuilder()

    override fun getEditable(): Editable = composition

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        composition.replace(0, composition.length, text ?: "")
        terminal.input.compose(composition.toString())
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        if (start < 0 || end < start || end > composition.length) return false
        terminal.input.compose(composition.substring(start, end))
        return true
    }

    override fun finishComposingText(): Boolean {
        composition.clear()
        terminal.input.finishComposition()
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        composition.clear()
        terminal.input.commit(text?.toString() ?: "")
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean =
        terminal.input.deleteSurrounding(beforeLength, afterLength)

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
        terminal.input.deleteSurrounding(beforeLength, afterLength)

    override fun sendKeyEvent(event: KeyEvent): Boolean = terminal.handleKey(event)

    override fun closeConnection() {
        // BaseInputConnection.closeConnection calls finishComposingText. Empty both local
        // buffers first, so editor teardown cannot commit pending text to a live session.
        composition.clear()
        terminal.input.discardComposition()
        super.closeConnection()
    }

    // Never expose remote terminal output to an IME as its editable document.
    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = ""
    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
    override fun getSelectedText(flags: Int): CharSequence? = null
}

package io.github.code_akram.or2.terminal

import android.text.Editable
import android.text.SpannableStringBuilder
import android.view.KeyEvent
import android.view.inputmethod.BaseInputConnection

internal class TerminalInputConnection(private val terminal: TerminalView) : BaseInputConnection(terminal, false) {
    private val composition = SpannableStringBuilder()
    private var active = true

    override fun getEditable(): Editable = composition

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        if (!active) return false
        composition.replace(0, composition.length, text ?: "")
        terminal.input.compose(composition.toString())
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        if (!active) return false
        if (start < 0 || end < start || end > composition.length) return false
        terminal.input.compose(composition.substring(start, end))
        return true
    }

    override fun finishComposingText(): Boolean {
        if (!active) return false
        composition.clear()
        terminal.input.finishComposition()
        return true
    }

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        if (!active) return false
        composition.clear()
        terminal.input.commit(text?.toString() ?: "")
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean =
        active && terminal.input.deleteSurrounding(beforeLength, afterLength)

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean =
        active && terminal.input.deleteSurrounding(beforeLength, afterLength)

    override fun sendKeyEvent(event: KeyEvent): Boolean = active && terminal.handleKey(event)

    internal fun cancelComposition() {
        if (!active) return
        // restartInput may leave queued calls targeting this editor. Retire it, not just its
        // buffer, so a stale candidate cannot restore or commit the cancelled composition.
        active = false
        removeComposingSpans(composition)
        composition.clear()
        composition.clearSpans()
        terminal.input.discardComposition()
    }

    override fun closeConnection() {
        // Also prevents a late close of an old editor from clearing a newer one's overlay.
        cancelComposition()
        super.closeConnection()
    }

    // Never expose remote terminal output to an IME as its editable document.
    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = ""
    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""
    override fun getSelectedText(flags: Int): CharSequence? = null
}

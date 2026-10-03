package io.github.code_akram.or2.terminal

import androidx.compose.ui.text.TextRange
import org.junit.Assert.assertEquals
import org.junit.Test

/** The toolbar's `/` and `@` type into the open composer at its cursor, over a selection, with the cursor after. */
class ComposerTypeTest {
    @Test
    fun aKeyTypesAtTheCursorAndTheCursorFollowsIt() {
        val chrome = TerminalChromeState(composerOpen = true, composerText = "review ")
        chrome.typeInComposer("@")
        assertEquals("review @", chrome.composerText)
        assertEquals(TextRange(8), chrome.composer.selection)

        // In the middle of the text, where the cursor was put.
        chrome.composerText = "fix this"
        chrome.composer.edit { selection = TextRange(0) }
        chrome.typeInComposer("/")
        assertEquals("/fix this", chrome.composerText)
        assertEquals(TextRange(1), chrome.composer.selection)
    }

    @Test
    fun aSelectionIsReplaced() {
        val chrome = TerminalChromeState(composerOpen = true, composerText = "ask claude")
        chrome.composer.edit { selection = TextRange(4, 10) }
        chrome.typeInComposer("@")
        assertEquals("ask @", chrome.composerText)
        assertEquals(TextRange(5), chrome.composer.selection)
    }
}

package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.paste.composerWithPath
import org.junit.Assert.assertEquals
import org.junit.Test

/** A confirmed multi-line send clears what it sent, not an image's path that arrived under the dialog. */
class ComposerSendTest {
    @Test
    fun anUploadedPathThatArrivedUnderTheConfirmationStays() {
        val chrome = TerminalChromeState(composerOpen = true, composerText = "first line\nsecond line")
        // Send asks for confirmation: the dialog holds what was in the composer then.
        val pending = chrome.composerText
        // The upload finishes behind the dialog: its path joins the composer.
        val path = "/home/u/.cache/or2/images/or2-20261002-153012-a1b2c3.png"
        chrome.composerText = composerWithPath(chrome.composerText, path)
        // Confirmed and sent: only the sent text goes; the path is still there to send.
        chrome.composerSent(pending)
        assertEquals(path, chrome.composerText)
    }

    @Test
    fun aSentMessageWithNothingAddedLeavesTheComposerEmpty() {
        val chrome = TerminalChromeState(composerOpen = true, composerText = "a\nb")
        chrome.composerSent("a\nb")
        assertEquals("", chrome.composerText)
    }

    @Test
    fun whatNoLongerStartsWithTheSentTextIsKeptWhole() {
        assertEquals("other", composerAfterSend("other", "a\nb"))
        assertEquals("x /p.png", composerAfterSend("a\nb\nx /p.png", "a\nb\n"))
        assertEquals("", composerAfterSend("", ""))
    }
}

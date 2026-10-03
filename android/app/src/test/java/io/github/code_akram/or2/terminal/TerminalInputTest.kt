package io.github.code_akram.or2.terminal

import android.view.KeyEvent
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.TerminalKey
import org.junit.Assert.*
import org.junit.Test

class TerminalInputTest {
    private val texts = mutableListOf<String>()
    private val keys = mutableListOf<KeyInput>()
    private val input = TerminalInput(texts::add, keys::add)

    @Test fun compositionNeverSendsAndCommitSendsExactlyOnce() {
        input.compose("e")
        input.compose("e\u0301界😀")
        assertTrue(texts.isEmpty())
        assertTrue(keys.isEmpty())
        assertEquals("e\u0301界😀", input.composing)
        input.commit("é界😀\n")
        assertEquals(listOf("é界😀\n"), texts)
        assertEquals("", input.composing)
        input.finishComposition()
        assertEquals(listOf("é界😀\n"), texts) // commit followed by finish cannot duplicate.
    }

    @Test fun explicitFinishWithoutCommitSendsPendingTextOnce() {
        input.compose("abc")
        assertTrue(texts.isEmpty())
        input.finishComposition()
        input.finishComposition()
        assertEquals(listOf("abc"), texts)
        assertEquals("", input.composing)
    }

    @Test fun teardownDiscardsBeforeFinishAndNeverSends() {
        input.compose("must not transmit")
        input.discardComposition()
        input.finishComposition() // BaseInputConnection closes by finishing after the discard.
        assertTrue(texts.isEmpty())
        assertTrue(keys.isEmpty())
        assertEquals("", input.composing)
    }

    @Test fun symbolKeysUseKeyInputAndDoNotCommitActiveComposition() {
        input.compose("pending")
        input.key(TerminalKey.Character("/"))
        assertEquals(TerminalKey.Character("/"), keys.single().key)
        assertTrue(texts.isEmpty())
        assertEquals("pending", input.composing)
    }

    @Test fun stickyModifiersAffectOnlyNextCharacterOrKeyAndMergeHardwareModifiers() {
        input.toggleCtrl()
        input.toggleAlt()
        input.commit("cxy")
        assertEquals(KeyInput(TerminalKey.Character("c"), KeyModifiers(false, true, true, false)), keys.single())
        assertEquals(listOf("xy"), texts)
        input.key(TerminalKey.ArrowLeft)
        assertFalse(keys.last().modifiers.ctrl)
        input.toggleCtrl()
        input.key(TerminalKey.Home, KeyModifiers(true, false, true, true))
        assertEquals(KeyModifiers(true, true, true, true), keys.last().modifiers)
        assertFalse(input.ctrl)
    }

    @Test fun modifiedSupplementaryCharacterIsNotSplitAndNewlineBecomesEnter() {
        input.toggleAlt()
        input.commit("😀z")
        assertEquals(TerminalKey.Character("😀"), keys.single().key)
        assertEquals(listOf("z"), texts)
        input.toggleCtrl()
        input.commit("\n")
        assertEquals(TerminalKey.Enter, keys.last().key)
    }

    @Test fun backwardAndForwardDeletionAreDistinctAndRejectNegativeLengths() {
        assertTrue(input.deleteSurrounding(2, 1))
        assertEquals(listOf(TerminalKey.Backspace, TerminalKey.Backspace, TerminalKey.Delete), keys.map { it.key })
        assertFalse(input.deleteSurrounding(-1, 0))
        assertFalse(input.deleteSurrounding(0, -1))
        assertEquals(3, keys.size)
    }

    @Test fun pasteCountsLogicalLinesAndConfirmsAnyLineBreak() {
        listOf(
            Triple("", 0, false), Triple("echo x", 1, false), Triple("\n", 2, true),
            Triple("echo x\n", 2, true), Triple("a\nb\nc", 3, true),
            Triple("a\r\nb\r\n", 3, true), Triple("a\rb", 2, true),
            Triple("a\n\rb", 3, true), Triple("界😀", 1, false),
        ).forEach { (text, lines, confirm) ->
            assertEquals(lines, pasteLineCount(text))
            assertEquals(confirm, pasteNeedsConfirmation(text, bracketedPaste = false))
            // With bracketed paste on the lines arrive as one paste: nothing is asked.
            assertFalse(pasteNeedsConfirmation(text, bracketedPaste = true))
        }
    }

    @Test fun shiftTabIsExactlyShiftTabAndLeavesTheLatchesArmed() {
        input.toggleCtrl()
        input.toggleAlt()
        input.exactKey(TerminalKey.Tab, KeyModifiers(true, false, false, false))
        assertEquals(listOf(KeyInput(TerminalKey.Tab, KeyModifiers(true, false, false, false))), keys)
        assertTrue(input.ctrl && input.alt)
        // The next key still takes them.
        input.key(TerminalKey.Character("/"))
        assertEquals(KeyInput(TerminalKey.Character("/"), KeyModifiers(false, true, true, false)), keys.last())
        assertFalse(input.ctrl || input.alt)
    }

    @Test fun hardwareNavigationFunctionAndUnicodeMappingsAreDistinct() {
        assertEquals(TerminalKey.Delete, terminalKey(KeyEvent.KEYCODE_FORWARD_DEL, 0))
        assertEquals(TerminalKey.Backspace, terminalKey(KeyEvent.KEYCODE_DEL, 0))
        assertEquals(TerminalKey.Home, terminalKey(KeyEvent.KEYCODE_MOVE_HOME, 0))
        assertEquals(TerminalKey.PageDown, terminalKey(KeyEvent.KEYCODE_PAGE_DOWN, 0))
        assertEquals(TerminalKey.Function(12u), terminalKey(KeyEvent.KEYCODE_F12, 0))
        assertEquals(TerminalKey.Character("界"), terminalKey(KeyEvent.KEYCODE_UNKNOWN, '界'.code))
        assertNull(terminalKey(KeyEvent.KEYCODE_CTRL_LEFT, 0))
        assertNull(terminalKey(KeyEvent.KEYCODE_UNKNOWN, 3))
    }
}

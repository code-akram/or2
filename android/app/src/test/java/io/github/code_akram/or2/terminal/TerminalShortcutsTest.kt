package io.github.code_akram.or2.terminal

import android.view.KeyEvent
import org.junit.Assert.*
import org.junit.Test

class TerminalShortcutsTest {
    private fun ctrlShift(keyCode: Int) = terminalShortcut(keyCode, ctrl = true, shift = true, alt = false, meta = false)

    @Test
    fun ctrlShiftKeysAreTheAppsShortcutsByKeyCode() {
        assertEquals(TerminalShortcut.SwitchTo(0), ctrlShift(KeyEvent.KEYCODE_1))
        assertEquals(TerminalShortcut.SwitchTo(4), ctrlShift(KeyEvent.KEYCODE_5))
        assertEquals(TerminalShortcut.SwitchTo(8), ctrlShift(KeyEvent.KEYCODE_9))
        assertEquals(TerminalShortcut.Close, ctrlShift(KeyEvent.KEYCODE_W))
        assertEquals(TerminalShortcut.Paste, ctrlShift(KeyEvent.KEYCODE_V))
        assertEquals(TerminalShortcut.Copy, ctrlShift(KeyEvent.KEYCODE_C))
        assertEquals(TerminalShortcut.Composer, ctrlShift(KeyEvent.KEYCODE_ENTER))
        assertEquals(TerminalShortcut.Composer, ctrlShift(KeyEvent.KEYCODE_NUMPAD_ENTER))
        // The key, not the character: Shift+/ types `?` on a US layout and something else elsewhere.
        assertEquals(TerminalShortcut.Help, ctrlShift(KeyEvent.KEYCODE_SLASH))
    }

    @Test
    fun everyOtherKeyAndModifierCombinationGoesToTheTerminal() {
        for (keyCode in listOf(KeyEvent.KEYCODE_0, KeyEvent.KEYCODE_A, KeyEvent.KEYCODE_T, KeyEvent.KEYCODE_TAB, KeyEvent.KEYCODE_ESCAPE)) {
            assertNull("$keyCode", ctrlShift(keyCode))
        }
        for ((ctrl, shift, alt, meta) in listOf(
            listOf(true, false, false, false), // Ctrl+W is the shell's delete-word.
            listOf(false, true, false, false),
            listOf(false, false, false, false),
            listOf(true, true, true, false),
            listOf(true, true, false, true),
        )) {
            assertNull(terminalShortcut(KeyEvent.KEYCODE_W, ctrl, shift, alt, meta))
            assertNull(terminalShortcut(KeyEvent.KEYCODE_1, ctrl, shift, alt, meta))
        }
    }

    @Test
    fun theSheetListsEveryShortcutAndGesture() {
        assertEquals(6, KeyboardShortcutRows.size)
        assertTrue(KeyboardShortcutRows.all { (keys, _) -> keys.startsWith("Ctrl+Shift+") })
        assertEquals(4, GestureRows.size)
    }
}

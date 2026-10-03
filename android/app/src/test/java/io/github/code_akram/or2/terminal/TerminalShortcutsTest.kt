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
    fun theSheetListsEveryShortcut() {
        assertEquals(6, KeyboardShortcutRows.size)
        assertTrue(KeyboardShortcutRows.all { (keys, _) -> keys.startsWith("Ctrl+Shift+") })
        // One row per shortcut the matcher knows, in the same words the sheet's own row uses.
        assertEquals("Gestures & shortcuts", KeyboardShortcutRows.toMap()["Ctrl+Shift+/"])
    }

    @Test
    fun theTouchRowsComeFirstAndSayWhatTheViewDoes() {
        val touch = TouchRows.toMap()
        // Taps, in the view's order (tapAction): the keyboard or a click, a link, and a long press that selects.
        assertEquals(TapAction.SHOW_KEYBOARD, tapAction(selecting = false, link = false, mouseTracking = false))
        assertEquals(TapAction.CLICK, tapAction(selecting = false, link = false, mouseTracking = true))
        assertTrue(touch.getValue("Tap").startsWith("Keyboard") && "click" in touch.getValue("Tap"))
        assertEquals(TapAction.OPEN_LINK, tapAction(selecting = false, link = true, mouseTracking = true))
        assertTrue("Tap a link" in touch)
        assertTrue("Copy" in touch.getValue("Long press"))
        // The swipes say the moves swipeNav makes: one finger left is the next window, two up the next session.
        assertEquals(io.github.code_akram.or2.ffi.TargetNav.NextWindow, swipeNav(Swipe.LEFT))
        assertTrue(touch.getValue("Swipe ← / →").startsWith("Next / previous window"))
        assertEquals(io.github.code_akram.or2.ffi.TargetNav.NextSession, swipeNav(Swipe.TWO_UP))
        assertTrue(touch.getValue("Two fingers ↑ / ↓").startsWith("Next / previous session"))
        assertEquals("Pane to the left / right", touch.getValue("Two fingers ← / →"))
        assertEquals("Text size", touch.getValue("Pinch"))
        assertTrue("shell" in SWIPES_NOTE)
    }
}

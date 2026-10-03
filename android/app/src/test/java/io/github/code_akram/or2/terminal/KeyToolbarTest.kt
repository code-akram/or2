package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.KeyModifiers
import io.github.code_akram.or2.ffi.TerminalKey
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Type
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The toolbar's keys, the width of its row from the dimension tokens, and the arrow pad's table. */
class KeyToolbarTest {
    /**
     * A label's drawn width in dp at the default font scale (1 sp = 1 dp): DroidSansMono, the toolbar's mono face, advances
     * 1229 of 2048 units (0.6 em) per character; any other glyph (`⇧`, which may come from a fallback font) counts a full
     * em, so the estimate errs wide.
     */
    private fun labelWidth(label: String): Float =
        label.sumOf { if (it.code < 0x80) 0.6 else 1.0 }.toFloat() * Or2Type.Key.fontSize.value

    /** A key's touch box: the drawn key (at least [Or2Dimens.KeyWidth]; a label plus its padding) and the touch inset. */
    private fun boxWidth(key: ToolbarKey): Float {
        val glyph = if (key.label != null && key.icon != null) KeyGlyph.value else 0f
        val drawn = maxOf(Or2Dimens.KeyWidth.value, key.label?.let { glyph + labelWidth(it) + 2 * Or2Dimens.KeyLabelPadding.value } ?: 0f)
        return drawn + (Or2Dimens.KeyTouchWidth - Or2Dimens.KeyWidth).value
    }

    /** The whole row: the pill's margins and padding, the keys, the gap, and the two toggles. */
    private fun rowWidth(selecting: Boolean): Float =
        2 * Or2Dimens.ToolbarMargin.value + 2 * Or2Dimens.ToolbarPadding.value +
            toolbarKeys(selecting).sumOf { boxWidth(it).toDouble() }.toFloat() +
            ToolbarToggles.sumOf { boxWidth(it).toDouble() }.toFloat()

    @Test
    fun theRowFitsA411DpWidePhoneWithoutScrollingWithOrWithoutASelection() {
        for (selecting in listOf(false, true)) {
            val width = rowWidth(selecting)
            assertTrue("the row is $width dp wide (selecting: $selecting)", width <= 411f)
        }
    }

    @Test
    fun theKeysAreInTheOwnersOrder() {
        assertEquals(
            listOf("Ctrl", "Esc", "Tab", "ShiftTab", "Shift", "Arrows", "Paste", "Slash", "At"),
            toolbarKeys(selecting = false).map { it.tag },
        )
        assertEquals(listOf("Composer", "Keyboard"), ToolbarToggles.map { it.tag })
        // No panes key (the header's green disc opens that sheet) and no history key (a swipe pages back, the button returns).
        assertTrue(ToolbarKey.entries.none { it.tag == "Panes" || it.tag == "History" })
    }

    @Test
    fun aSelectionPutsCopyAndClearFirstInPlaceOfTheTypingKeys() {
        assertEquals(listOf("Copy", "Clear", "Ctrl", "Esc", "Tab", "Arrows", "Paste"), toolbarKeys(selecting = true).map { it.tag })
    }

    @Test
    fun everyKeyHasALabelOrAnIconAndADescription() {
        ToolbarKey.entries.forEach {
            // A label, an icon, or both (`⇧Tab`: the Shift arrow, then `Tab`); never neither.
            assertTrue(it.name, it.label != null || it.icon != null)
            assertTrue(it.name, it.description.isNotBlank())
        }
    }

    @Test
    fun theArrowPadIsATableOfWhatEachKeySends() {
        val keys = PadRows.flatten().associateBy { it.tag }
        assertEquals(listOf(3, 3, 1), PadRows.map { it.size })
        assertEquals(TerminalKey.Backspace, keys.getValue("Backspace").key)
        assertEquals(TerminalKey.Enter, keys.getValue("Enter").key)
        // Clear-line is Ctrl-U, the shell's "clear the line before the cursor".
        assertEquals(TerminalKey.Character("u"), keys.getValue("Clear").key)
        assertEquals(KeyModifiers(false, true, false, false), keys.getValue("Clear").modifiers)
        assertTrue(PadRows.flatten().filter { it.tag != "Clear" }.all { it.modifiers == KeyModifiers(false, false, false, false) })
    }

    @Test
    fun theExtrasHaveTheNavigationKeysAndTheSymbolsButNotTheToolbarsSlash() {
        val extras = PadExtras.toMap()
        assertEquals(TerminalKey.Home, extras.getValue("Home"))
        assertEquals(TerminalKey.PageDown, extras.getValue("PgDn"))
        assertFalse("/ is on the toolbar itself", "/" in extras)
        assertEquals(PadExtras.size, extras.size) // No label twice.
        PadExtras.filter { (label, _) -> label.length == 1 }.forEach { (label, key) -> assertEquals(TerminalKey.Character(label), key) }
    }
}

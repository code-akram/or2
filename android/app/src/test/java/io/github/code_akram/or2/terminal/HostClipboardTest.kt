package io.github.code_akram.or2.terminal

import org.junit.Assert.assertEquals
import org.junit.Test

class HostClipboardTest {
    private var time = 1_000L
    private var enabled = true
    private val written = mutableListOf<String>()
    private val scheduled = mutableListOf<Pair<Long, () -> Unit>>()
    private val clipboard = HostClipboard(
        enabled = { enabled },
        now = { time },
        schedule = { delay, action -> scheduled += (time + delay) to action },
        write = { written += it },
    )

    /** Moves the clock to [to], running what was scheduled up to then. */
    private fun advanceTo(to: Long) {
        while (true) {
            val next = scheduled.filter { it.first <= to }.minByOrNull { it.first } ?: break
            scheduled.remove(next)
            time = next.first
            next.second()
        }
        time = to
    }

    @Test
    fun theFirstWriteGoesAtOnceAndLaterOnesInTheWindowReplaceOnePendingWrite() {
        clipboard.offer(1, "one")
        assertEquals(listOf("one"), written)
        advanceTo(1_100)
        clipboard.offer(1, "two")
        advanceTo(1_200)
        clipboard.offer(1, "three")
        assertEquals(listOf("one"), written) // Held until the window ends.
        assertEquals(1, scheduled.size) // One pending write, not one per offer.
        advanceTo(1_499)
        assertEquals(listOf("one"), written)
        advanceTo(1_500)
        assertEquals(listOf("one", "three"), written)

        // The flush opened a new window: a write right after it waits for its end too.
        clipboard.offer(1, "four")
        advanceTo(1_999)
        assertEquals(listOf("one", "three"), written)
        advanceTo(2_000)
        assertEquals(listOf("one", "three", "four"), written)

        // After a quiet window, the next write goes at once again.
        advanceTo(3_000)
        clipboard.offer(1, "five")
        assertEquals(listOf("one", "three", "four", "five"), written)
    }

    @Test
    fun eachTerminalHasItsOwnWindow() {
        clipboard.offer(1, "a")
        clipboard.offer(2, "b")
        clipboard.offer(1, "c")
        assertEquals(listOf("a", "b"), written)
        advanceTo(1_500)
        assertEquals(listOf("a", "b", "c"), written)
    }

    @Test
    fun theSwitchTurnsItOffEvenForAPendingWrite() {
        enabled = false
        clipboard.offer(1, "off")
        assertEquals(emptyList<String>(), written)
        enabled = true
        clipboard.offer(1, "on")
        clipboard.offer(1, "pending")
        enabled = false
        advanceTo(2_000)
        assertEquals(listOf("on"), written)
    }

    @Test
    fun textOverOneMebibyteAndEmptyTextAreDropped() {
        clipboard.offer(1, "")
        clipboard.offer(2, "a".repeat(HostClipboard.MAX_BYTES + 1))
        // Under the cap in characters, over it in UTF-8 bytes.
        clipboard.offer(3, "é".repeat(HostClipboard.MAX_BYTES / 2 + 1))
        assertEquals(emptyList<String>(), written)
        clipboard.offer(4, "a".repeat(HostClipboard.MAX_BYTES))
        assertEquals(1, written.size)
    }

    @Test
    fun utf8LengthCountsBytes() {
        assertEquals(0, HostClipboard.utf8Length(""))
        assertEquals(3, HostClipboard.utf8Length("abc"))
        assertEquals(2, HostClipboard.utf8Length("é"))
        assertEquals(3, HostClipboard.utf8Length("界"))
        assertEquals(4, HostClipboard.utf8Length("😀"))
        assertEquals("a界😀é".encodeToByteArray().size, HostClipboard.utf8Length("a界😀é"))
    }
}

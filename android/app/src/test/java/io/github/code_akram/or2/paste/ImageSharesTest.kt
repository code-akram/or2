package io.github.code_akram.or2.paste

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.URI

/**
 * A shared image's pending choice of terminal, in saved state: kept over a rotation, not a process death; and
 * a share of several images (`ACTION_SEND_MULTIPLE`): each item checked, at most [MAX_IMAGES], none dropped
 * without a word.
 */
class ImageSharesTest {
    private val uri = "content://com.example.photos/image/42"

    @Test
    fun aPendingShareSurvivesARecreationInTheSameProcess() {
        val saved = savedShare(uri)
        assertArrayEquals(arrayOf(THIS_PROCESS, uri), saved)
        assertEquals(listOf(uri), restoredShare(saved))
        // Several images, in order.
        val several = listOf(uri, "content://com.example.photos/image/43", "content://com.example.photos/image/44")
        assertEquals(several, restoredShare(savedShare(*several.toTypedArray())))
    }

    @Test
    fun aPendingShareIsDroppedWhenTheProcessDiedMeanwhile() {
        val saved = savedShare(uri, process = "a process that died")
        assertNull(restoredShare(saved))
        assertNull(restoredShare(arrayOf(uri)))
        assertNull(restoredShare(emptyArray()))
    }

    /** A share of these streams, schemes read as a URI parser reads them (`/sdcard/x.png` has none). */
    private fun share(items: List<String?>) = shareOf(items) { URI(it).scheme }

    private fun image(n: Int) = "content://com.example.photos/image/$n"

    @Test
    fun aShareOfSeveralContentImagesGivesThemAllInOrder() {
        val three = listOf(image(1), image(2), image(3))
        assertEquals(Share(three), share(three))
        assertNull(shareNote(share(three)))
        // One image, as a single share gives it.
        assertEquals(Share(listOf(image(1))), share(listOf(image(1))))
    }

    @Test
    fun anItemThatIsNoContentImageIsSkippedAndCounted() {
        val shared = share(listOf(image(1), "file:///data/data/io.github.code_akram.or2/files/secret.png", image(2), "/sdcard/x.png", null))
        assertEquals(listOf(image(1), image(2)), shared.images)
        assertEquals(3, shared.refused)
        assertEquals(0, shared.over)
        assertEquals("3 shared items are not images or2 can read", shareNote(shared))
        assertEquals("1 shared item is not an image or2 can read", shareNote(share(listOf("file:///x.png", image(1)))))
        // Nothing readable: still a share, so it is said.
        assertEquals(Share<String>(emptyList(), refused = 1), share(listOf("file:///x.png")))
    }

    @Test
    fun moreThanTenKeepsTheFirstTenAndSaysSo() {
        val twelve = (1..12).map(::image)
        val shared = share(twelve)
        assertEquals(twelve.take(MAX_IMAGES), shared.images)
        assertEquals(2, shared.over)
        assertEquals("At most 10 images at a time: sending the first 10", shareNote(shared))
        // Unreadable items do not take places: the first ten readable ones are kept, and both are said.
        val mixed = share(listOf("file:///x.png") + twelve)
        assertEquals(twelve.take(MAX_IMAGES), mixed.images)
        assertEquals(1, mixed.refused)
        val note = shareNote(mixed)!!
        assertTrue(note, note.startsWith(TOO_MANY_IMAGES) && note.endsWith("1 shared item is not an image or2 can read"))
        // Exactly ten: all kept, nothing to say.
        assertNull(shareNote(share(twelve.take(MAX_IMAGES))))
    }
}

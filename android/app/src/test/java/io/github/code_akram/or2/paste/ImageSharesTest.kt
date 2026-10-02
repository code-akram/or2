package io.github.code_akram.or2.paste

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** A shared image's pending choice of terminal, in saved state: kept over a rotation, not a process death. */
class ImageSharesTest {
    private val uri = "content://com.example.photos/image/42"

    @Test
    fun aPendingShareSurvivesARecreationInTheSameProcess() {
        val saved = savedShare(uri)
        assertArrayEquals(arrayOf(THIS_PROCESS, uri), saved)
        assertEquals(uri, restoredShare(saved))
    }

    @Test
    fun aPendingShareIsDroppedWhenTheProcessDiedMeanwhile() {
        val saved = savedShare(uri, process = "a process that died")
        assertNull(restoredShare(saved))
        assertNull(restoredShare(arrayOf(uri)))
        assertNull(restoredShare(emptyArray()))
    }
}

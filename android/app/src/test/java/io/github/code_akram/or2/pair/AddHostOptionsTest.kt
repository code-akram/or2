package io.github.code_akram.or2.pair

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/** The one add-host chooser that Home's empty state, the inbox's and the "+" sheet all draw. */
class AddHostOptionsTest {
    @Test
    fun easyPairIsTheRecommendedFirstCardAndTheManualFormTheSecond() {
        assertEquals(listOf(AddHostRoute.EASY_PAIR, AddHostRoute.MANUAL), AddHostOptions.map { it.route })
        val (easy, manual) = AddHostOptions
        assertEquals("Fastest" to "Easy pair with QR", easy.kicker to easy.title)
        assertEquals("Recommended · ~1 min", easy.meta)
        assertEquals("SSH-fluent" to "Set up manually", manual.kicker to manual.title)
        assertEquals("~3 min · needs hostname + key", manual.meta)
    }

    @Test
    fun theCardsKeepTheirTagsSoEveryPlaceTagsThemAlike() {
        // `add-host-easy` in the sheet, `home-add-host-easy` on Home, `inbox-add-host-easy` in the inbox.
        assertEquals(listOf("easy", "manual"), AddHostOptions.map { it.tag })
    }

    @Test
    fun noCardSendsThePersonToTheKeysScreenFirst() {
        // Both paths make the key on the phone (Easy pair's review and the host form offer New key).
        assertFalse(AddHostOptions.any { "Add an SSH key" in it.title || "Keys screen" in it.body })
    }
}

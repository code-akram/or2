package io.github.code_akram.or2.session

import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Colors
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** The badge's colours: teal `Mosh`, the `SSH` look, and the grey a quiet link falls back to. */
class TransportBadgeTest {
    @Test
    fun aHealthyMoshBadgeIsTealAndSshIsTheTrackLook() {
        assertEquals(Or2Colors.Teal to Or2Colors.Background, transportBadgeColors(Transport.MOSH, stale = false))
        assertEquals(Or2Colors.SurfaceTrack to Or2Colors.Text.copy(alpha = 0.7f), transportBadgeColors(Transport.SSH, stale = false))
    }

    @Test
    fun aQuietLinkGreysTheBadgeWithMutedText() {
        val grey = Or2Colors.SurfaceTrack to Or2Colors.TextMuted
        assertEquals(grey, transportBadgeColors(Transport.MOSH, stale = true))
        assertEquals(grey, transportBadgeColors(Transport.SSH, stale = true))
        assertNotEquals(transportBadgeColors(Transport.MOSH, stale = false), transportBadgeColors(Transport.MOSH, stale = true))
    }
}

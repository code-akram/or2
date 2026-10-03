package io.github.code_akram.or2.session

import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Colors
import org.junit.Assert.assertEquals
import org.junit.Test

/** The badge's colours: teal `Mosh` with dark text, and the `SSH` track look. */
class TransportBadgeTest {
    @Test
    fun aMoshBadgeIsTealAndSshIsTheTrackLook() {
        assertEquals(Or2Colors.Teal to Or2Colors.Background, transportBadgeColors(Transport.MOSH))
        assertEquals(Or2Colors.SurfaceTrack to Or2Colors.Text, transportBadgeColors(Transport.SSH))
    }
}

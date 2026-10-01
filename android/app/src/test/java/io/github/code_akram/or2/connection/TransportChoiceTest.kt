package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTransport
import org.junit.Assert.*
import org.junit.Test

class TransportChoiceTest {
    private val server = "/usr/bin/mosh-server"

    @Test
    fun autoPicksMoshOnlyWithMoshServerAndNoEarlierFailure() {
        assertEquals(TerminalTransport.MOSH, chooseTransport(TransportPref.AUTO, server, moshRejected = false))
        assertEquals(TerminalTransport.SSH, chooseTransport(TransportPref.AUTO, null, moshRejected = false))
        assertEquals(TerminalTransport.SSH, chooseTransport(TransportPref.AUTO, server, moshRejected = true))
    }

    @Test
    fun explicitChoicesAreHonouredWhateverTheProbeSaid() {
        for (rejected in listOf(false, true)) for (found in listOf(server, null)) {
            assertEquals(TerminalTransport.SSH, chooseTransport(TransportPref.SSH, found, rejected))
            assertEquals(TerminalTransport.MOSH, chooseTransport(TransportPref.MOSH, found, rejected))
        }
    }

    @Test
    fun onlyTimedOutAndNotInstalledFallBack() {
        assertTrue(isMoshFallback(SessionFailure.TimedOut))
        assertTrue(isMoshFallback(SessionFailure.NotInstalled("mosh-server")))
        // A missing tmux or herdr is not mosh's problem: the SSH retry would fail the same way.
        assertFalse(isMoshFallback(SessionFailure.NotInstalled("tmux")))
        assertFalse(isMoshFallback(SessionFailure.NotInstalled("herdr")))
        assertFalse(isMoshFallback(SessionFailure.Unreachable("x")))
        assertFalse(isMoshFallback(SessionFailure.AuthenticationRejected))
        assertFalse(isMoshFallback(SessionFailure.ConnectionLost("x")))
        assertFalse(isMoshFallback(SessionFailure.CommandFailed("x")))
        assertFalse(isMoshFallback(SessionFailure.Internal("x")))
    }

    @Test
    fun theFallbackNoteSaysWhy() {
        assertTrue(moshFallbackNote(SessionFailure.TimedOut).contains("UDP"))
        assertTrue(moshFallbackNote(SessionFailure.NotInstalled("mosh-server")).startsWith("mosh-server is not installed"))
        assertTrue(moshFallbackNote(SessionFailure.TimedOut).endsWith("Using SSH for this connection."))
    }

    @Test
    fun linkHealthGreysOnlyPastFiveSeconds() {
        assertNull(linkStaleLabel(null))
        assertNull(linkStaleLabel(LinkHealth(300uL, 300uL)))
        assertNull(linkStaleLabel(LinkHealth(5000uL, 9000uL))) // Exactly five seconds is still fine.
        assertEquals("Last heard 5 s ago", linkStaleLabel(LinkHealth(5001uL, 5001uL)))
        assertEquals("Last heard 12 s ago", linkStaleLabel(LinkHealth(12_400uL, 20_000uL)))
        assertNull(linkStaleLabel(LinkHealth(400uL, 400uL))) // Recovered.
    }
}

package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.keys.keyErrorMessage
import org.junit.Assert.*
import org.junit.Test

class SessionMessagesTest {
    @Test
    fun everyCloseReasonHasDistinctActionableMessageWithoutDiagnostics() {
        val failures = listOf(
            SessionFailure.Unreachable("diagnostic"), SessionFailure.TimedOut, SessionFailure.HostKeyRejected,
            SessionFailure.UnsupportedHostKey("diagnostic"), SessionFailure.AuthenticationRejected,
            SessionFailure.ShellRejected, SessionFailure.ConnectionLost("diagnostic"),
            SessionFailure.Protocol("diagnostic"), SessionFailure.Internal("diagnostic"),
            SessionFailure.NotInstalled("tmux"), SessionFailure.CommandFailed("diagnostic"),
        )
        val messages = failures.map { sessionMessage(SessionState.Closed(CloseReason.Failed(it))) }
        assertEquals(11, messages.toSet().size)
        assertTrue(messages.none { "diagnostic" in it })
        assertTrue(messages[0].contains("network"))
        assertTrue(messages[4].contains("username"))
        assertTrue(messages[5].contains("shell"))
        assertEquals("tmux is not installed on the host.", messages[9])
        assertEquals("Disconnected", sessionMessage(SessionState.Closed(CloseReason.Disconnected)))
        assertEquals("Remote shell exited (status 23)", sessionMessage(SessionState.Closed(CloseReason.RemoteExited(23u))))
        assertEquals("Remote shell exited", sessionMessage(SessionState.Closed(CloseReason.RemoteExited(null))))
    }

    @Test
    fun synchronousAndImportErrorsAreNotBlankOrMatchedByDiagnosticText() {
        val errors = listOf(HostConnectException.NoAddresses(), HostConnectException.TooManyAddresses(), HostConnectException.InvalidAddress(2u),
            HostConnectException.InvalidUsername(), HostConnectException.InvalidPrivateKey(), HostConnectException.InvalidTrustedHostKey(2u))
        assertEquals(6, errors.map(::hostConnectErrorMessage).toSet().size)
        assertTrue(hostConnectErrorMessage(errors[2]).contains("Address 3")) // Indexes are shown 1-based.
        assertTrue(hostConnectErrorMessage(errors[4]).contains("Import"))
        assertTrue(keyErrorMessage(KeyException.PassphraseRequired()).contains("Enter"))
        assertTrue(keyErrorMessage(KeyException.WrongPassphrase()).contains("Incorrect"))
        assertTrue(keyErrorMessage(KeyException.UnsupportedFormat()).contains("ssh-keygen"))
        assertTrue(keyErrorMessage(KeyException.Malformed()).contains("readable"))
        assertFalse(keyErrorMessage(KeyException.UnsupportedAlgorithm("diagnostic")).contains("diagnostic"))
    }

    @Test
    fun hostErrorsAndStatesAreDistinctAndFreeOfDiagnostics() {
        val errors = listOf(HostException.NotConnected(), HostException.Closed(), HostException.NoHostKeyPrompt(), HostException.HostKeyMismatch(),
            HostException.EmptyDimension(), HostException.InvalidName(), HostException.NotInstalled("tmux"), HostException.CommandFailed("diagnostic"), HostException.PaneNotFound())
        val messages = errors.map(::hostErrorMessage)
        assertEquals(9, messages.toSet().size)
        assertTrue(messages.none { "diagnostic" in it })
        assertEquals("tmux is not installed on the host.", messages[6])
        assertEquals("Connecting\u2026", hostStateMessage(HostState.Connecting))
        assertEquals("Connected", hostStateMessage(HostState.Connected(0u)))
        assertEquals("Disconnected", hostStateMessage(HostState.Closed(CloseReason.Disconnected)))
        // A closed host explains itself exactly as a closed session does.
        assertTrue(hostStateMessage(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))).contains("username"))
        val herdr = listOf(HerdrState.Starting, HerdrState.Closed, HerdrState.Unavailable(HerdrUnavailable.NotInstalled, "diagnostic"),
            HerdrState.Unavailable(HerdrUnavailable.NotRunning, "diagnostic"), HerdrState.Unavailable(HerdrUnavailable.IncompatibleProtocol(9u), "diagnostic"),
            HerdrState.Unavailable(HerdrUnavailable.Failed, "diagnostic"))
        assertEquals(6, herdr.map(::herdrStateMessage).toSet().size)
        assertTrue(herdr.map(::herdrStateMessage).none { "diagnostic" in it })
    }

    private val addresses = listOf(io.github.code_akram.or2.data.HostEndpoint("mac.local", 22), io.github.code_akram.or2.data.HostEndpoint("10.0.0.5", 2222))

    @Test
    fun theCoresPositionsBecomeTheHostsAddressesOneLineEach() {
        assertEquals(
            "mac.local:22 \u00b7 connection refused\n10.0.0.5:2222 \u00b7 no answer within 6 s",
            unreachableDetail("TCP connection failed: address 0: connection refused; address 1: no answer within 6 s", addresses),
        )
        // A race still running when the connect timeout fired.
        assertEquals(
            "mac.local:22 \u00b7 still trying after 20 s\n10.0.0.5:2222 \u00b7 not tried yet",
            unreachableDetail("no address answered within 20 s: address 0: still trying after 20 s; address 1: not tried yet", addresses),
        )
        // One endpoint that tried several resolved addresses says so in its own line.
        assertEquals(
            "mac.local:22 \u00b7 2 addresses: connection refused, no answer within 5 s",
            unreachableDetail("address 0: 2 addresses: connection refused, no answer within 5 s", addresses),
        )
    }

    @Test
    fun aMessageWithoutPositionsOrWithAnUnknownAddressHasNoDetail() {
        assertNull(unreachableDetail("diagnostic", addresses))
        assertNull(unreachableDetail("", addresses))
        assertNull(unreachableDetail("address 5: connection refused", addresses))
        assertNull(unreachableDetail("address 0: connection refused", emptyList()))
    }

    @Test
    fun onlyAnUnreachableCloseHasADetail() {
        fun closed(failure: SessionFailure) = HostState.Closed(CloseReason.Failed(failure))
        assertNotNull(hostFailureDetail(closed(SessionFailure.Unreachable("address 0: connection refused")), addresses))
        assertNull(hostFailureDetail(closed(SessionFailure.TimedOut), addresses))
        assertNull(hostFailureDetail(closed(SessionFailure.ConnectionLost("address 0: x")), addresses))
        assertNull(hostFailureDetail(HostState.Closed(CloseReason.Disconnected), addresses))
        assertNull(hostFailureDetail(HostState.Connected(0u), addresses))
        assertNull(hostFailureDetail(null, addresses))
    }
}

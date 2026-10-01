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
            HostException.EmptyDimension(), HostException.InvalidName(), HostException.NotInstalled("tmux"), HostException.CommandFailed("diagnostic"))
        val messages = errors.map(::hostErrorMessage)
        assertEquals(8, messages.toSet().size)
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
}

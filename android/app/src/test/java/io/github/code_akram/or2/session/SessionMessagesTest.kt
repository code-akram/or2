package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.*
import io.github.code_akram.or2.hosts.hostFieldError
import io.github.code_akram.or2.hosts.validHost
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
        val errors = listOf(ConnectException.InvalidHost(), ConnectException.InvalidPort(), ConnectException.InvalidUsername(),
            ConnectException.InvalidPrivateKey(), ConnectException.InvalidTrustedHostKey(2u), ConnectException.EmptyDimension())
        assertEquals(6, errors.map(::connectErrorMessage).toSet().size)
        assertTrue(connectErrorMessage(errors[3]).contains("Import"))
        assertTrue(keyErrorMessage(KeyException.PassphraseRequired()).contains("Enter"))
        assertTrue(keyErrorMessage(KeyException.WrongPassphrase()).contains("Incorrect"))
        assertTrue(keyErrorMessage(KeyException.UnsupportedFormat()).contains("ssh-keygen"))
        assertTrue(keyErrorMessage(KeyException.Malformed()).contains("readable"))
        assertFalse(keyErrorMessage(KeyException.UnsupportedAlgorithm("diagnostic")).contains("diagnostic"))
    }

    @Test
    fun hostValidationChecksBothPortBoundariesAndAddressControlCharacters() {
        assertTrue(validHost("Label", "fixture.invalid", "1", "fixture"))
        assertTrue(validHost("Label", "fixture.invalid", "65535", "fixture"))
        for (port in listOf("0", "65536", "", "-1", "22x")) assertFalse(validHost("Label", "fixture.invalid", port, "fixture"))
        assertTrue(validHost("Label", " fixture.invalid ", "22", " fixture "))
        assertFalse(validHost("Label", "bad address", "22", "fixture"))
        assertFalse(validHost("Label", "fixture.invalid", "22", "bad name"))
        assertFalse(validHost("Label", "fixture.invalid", "22", "bad\nname"))
        assertEquals("Remove internal whitespace or control characters.", hostFieldError("bad name"))
        assertEquals("Enter a value.", hostFieldError(" \t"))
    }
}

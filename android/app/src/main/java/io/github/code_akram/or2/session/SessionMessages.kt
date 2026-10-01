package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState

fun sessionMessage(state: SessionState): String = when (state) {
    SessionState.Connecting -> "Connecting…"
    is SessionState.AwaitingHostKeyDecision -> "Waiting for host-key approval"
    SessionState.Authenticating -> "Authenticating…"
    SessionState.Connected -> "Connected"
    is SessionState.Closed -> when (val reason = state.reason) {
        CloseReason.Disconnected -> "Disconnected"
        is CloseReason.RemoteExited -> "Remote shell exited" + (reason.exitStatus?.let { " (status $it)" } ?: "")
        is CloseReason.Failed -> when (val failure = reason.failure) {
            is SessionFailure.Unreachable -> "Host unreachable. Check the address and network."
            SessionFailure.TimedOut -> "Connection timed out."
            SessionFailure.HostKeyRejected -> "Host key rejected. No connection was authorized."
            is SessionFailure.UnsupportedHostKey -> "Unsupported host key. Host certificates are not supported."
            SessionFailure.AuthenticationRejected -> "Authentication rejected. Check the username and public-key authorization."
            SessionFailure.ShellRejected -> "The server refused a terminal or shell."
            is SessionFailure.NotInstalled -> "${failure.program} is not installed on the host."
            is SessionFailure.CommandFailed -> "A command on the host failed. Retry or open a plain shell."
            is SessionFailure.ConnectionLost -> "Connection lost. Reconnect when the network is available."
            is SessionFailure.Protocol -> "SSH protocol error."
            is SessionFailure.Internal -> "Internal session error. Disconnect and retry."
        }
    }
}

fun hostConnectErrorMessage(error: HostConnectException): String = when (error) {
    is HostConnectException.NoAddresses -> "Add at least one address to this host."
    is HostConnectException.TooManyAddresses -> "A host can have at most 8 addresses."
    is HostConnectException.InvalidAddress -> "Address ${error.index + 1u} is invalid. Check the hostname (no whitespace) and port (1-65535)."
    is HostConnectException.InvalidUsername -> "Enter a username without control characters."
    is HostConnectException.InvalidPrivateKey -> "Stored private key is unusable. Import or generate a new key."
    is HostConnectException.InvalidTrustedHostKey -> "A stored trusted host key is malformed. Edit or recreate the host."
}

fun hostErrorMessage(error: HostException): String = when (error) {
    is HostException.NotConnected -> "The host is not connected yet."
    is HostException.Closed -> "The connection has closed. Reconnect to continue."
    is HostException.NoHostKeyPrompt -> "This host-key prompt has expired."
    is HostException.HostKeyMismatch -> "The presented host key no longer matches this decision. Disconnect and verify it again."
    is HostException.EmptyDimension -> "Terminal dimensions must be nonzero."
    is HostException.InvalidName -> "That name is not valid here. Use letters, digits, dashes and underscores."
    is HostException.NotInstalled -> "${error.program} is not installed on the host."
    is HostException.CommandFailed -> "A command on the host failed. Retry, or reconnect."
}

/** A closed connection explains itself with the same words as a closed session. */
fun hostStateMessage(state: HostState): String = when (state) {
    HostState.Connecting -> "Connecting\u2026"
    is HostState.AwaitingHostKeyDecision -> "Waiting for host-key approval"
    HostState.Authenticating -> "Authenticating\u2026"
    is HostState.Connected -> "Connected"
    is HostState.Closed -> sessionMessage(SessionState.Closed(state.reason))
}

fun herdrStateMessage(state: HerdrState): String = when (state) {
    HerdrState.Starting -> "Starting\u2026"
    is HerdrState.Live -> "Live"
    HerdrState.Closed -> "Stopped"
    is HerdrState.Unavailable -> when (val reason = state.reason) {
        HerdrUnavailable.NotInstalled -> "herdr is not installed on the host."
        is HerdrUnavailable.IncompatibleProtocol -> "This herdr speaks protocol ${reason.protocol}, which or2 does not support."
        HerdrUnavailable.NotRunning -> "herdr is not running."
        HerdrUnavailable.Failed -> "herdr is unavailable. or2 retries while the host is connected."
    }
}

fun sessionErrorMessage(error: SessionException): String = when (error) {
    is SessionException.NotConnected -> "The session is not connected yet."
    is SessionException.Closed -> "The session has closed. Reconnect to continue."
    is SessionException.NoHostKeyPrompt -> "This host-key prompt has expired."
    is SessionException.HostKeyMismatch -> "The presented host key no longer matches this decision. Disconnect and verify it again."
    is SessionException.EmptyDimension -> "Terminal dimensions must be nonzero."
    is SessionException.InvalidKey -> "The requested terminal key is invalid."
}

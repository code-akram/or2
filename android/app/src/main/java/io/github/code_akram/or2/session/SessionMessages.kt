package io.github.code_akram.or2.session

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.ConnectException
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

fun connectErrorMessage(error: ConnectException): String = when (error) {
    is ConnectException.InvalidHost -> "Invalid host address. Remove whitespace and control characters."
    is ConnectException.InvalidPort -> "Port must be between 1 and 65535."
    is ConnectException.InvalidUsername -> "Enter a username without control characters."
    is ConnectException.InvalidPrivateKey -> "Stored private key is unusable. Import or generate a new key."
    is ConnectException.InvalidTrustedHostKey -> "A stored trusted host key is malformed. Edit or recreate the host."
    is ConnectException.EmptyDimension -> "Terminal dimensions must be nonzero."
}

fun sessionErrorMessage(error: SessionException): String = when (error) {
    is SessionException.NotConnected -> "The session is not connected yet."
    is SessionException.Closed -> "The session has closed. Reconnect to continue."
    is SessionException.NoHostKeyPrompt -> "This host-key prompt has expired."
    is SessionException.HostKeyMismatch -> "The presented host key no longer matches this decision. Disconnect and verify it again."
    is SessionException.EmptyDimension -> "Terminal dimensions must be nonzero."
    is SessionException.InvalidKey -> "The requested terminal key is invalid."
}

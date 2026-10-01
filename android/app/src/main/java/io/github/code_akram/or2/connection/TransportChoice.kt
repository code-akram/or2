package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTransport

/**
 * The transport a new terminal opens over.
 *
 * `SSH` and `MOSH` are the user's explicit choice and are never second-guessed (a mosh failure is
 * shown as it is). `AUTO` uses mosh when the host has `mosh-server` ([moshServer], from the
 * capability probe; null until probed or when absent) unless mosh already failed on this
 * connection ([moshRejected]). [remembered] is the transport the user last had on the same target
 * (reattach): under AUTO it wins, except that mosh still needs a `mosh-server`.
 */
fun chooseTransport(
    pref: TransportPref, moshServer: String?, moshRejected: Boolean, remembered: TerminalTransport? = null,
): TerminalTransport = when (pref) {
    TransportPref.SSH -> TerminalTransport.SSH
    TransportPref.MOSH -> TerminalTransport.MOSH
    TransportPref.AUTO -> when {
        moshServer == null || moshRejected -> TerminalTransport.SSH
        remembered != null -> remembered
        else -> TerminalTransport.MOSH
    }
}

/** AUTO falls back to SSH only for these two failures, and only before the terminal connected. */
fun isMoshFallback(failure: SessionFailure): Boolean =
    failure is SessionFailure.TimedOut || failure is SessionFailure.NotInstalled

/** The muted line under the terminal header after AUTO fell back to SSH. */
fun moshFallbackNote(failure: SessionFailure): String = when (failure) {
    SessionFailure.TimedOut -> "Mosh could not reach the host over UDP. Using SSH for this connection."
    is SessionFailure.NotInstalled -> "${failure.program} is not installed on the host. Using SSH for this connection."
    else -> "Mosh did not start. Using SSH for this connection."
}

/** Link health older than this greys the transport badge (contract: `since_heard_ms > 5000`). */
const val STALE_HEARD_MS = 5_000uL

/** "Last heard 12 s ago" once nothing has been heard for more than five seconds; else null. */
fun linkStaleLabel(health: LinkHealth?): String? {
    if (health == null || health.sinceHeardMs <= STALE_HEARD_MS) return null
    return "Last heard ${health.sinceHeardMs / 1000uL} s ago"
}

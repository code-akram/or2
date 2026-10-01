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
 * connection ([moshRejected]). What a target ran over before (reattach) is deliberately not an
 * input: it would carry an earlier SSH fallback, or a choice made before the probe answered, into
 * connections that should decide afresh.
 */
fun chooseTransport(pref: TransportPref, moshServer: String?, moshRejected: Boolean): TerminalTransport = when (pref) {
    TransportPref.SSH -> TerminalTransport.SSH
    TransportPref.MOSH -> TerminalTransport.MOSH
    TransportPref.AUTO -> if (moshServer == null || moshRejected) TerminalTransport.SSH else TerminalTransport.MOSH
}

/** The program whose absence is mosh itself being unavailable; any other missing program is not a mosh problem. */
const val MOSH_SERVER_PROGRAM = "mosh-server"

/**
 * AUTO falls back to SSH only when UDP is blocked (`TimedOut`) or the host has no `mosh-server`,
 * and only before the terminal connected. A missing `tmux` or `herdr` would fail over SSH the
 * same way, and must not mark mosh as rejected on the connection.
 */
fun isMoshFallback(failure: SessionFailure): Boolean =
    failure is SessionFailure.TimedOut || (failure is SessionFailure.NotInstalled && failure.program == MOSH_SERVER_PROGRAM)

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

package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport

/**
 * What this connection knows about mosh's UDP path to the host ([ActiveHost.udpVerdict]). It is
 * learned per connection and never remembered across connections: a blocked host costs one
 * invisible background attempt, and a user who fixes the firewall gets mosh on the next connection.
 */
enum class UdpVerdict {
    /** No mosh terminal has connected or failed for UDP's sake on this connection yet. */
    UNKNOWN,

    /** A mosh terminal reached `Connected` on this connection. */
    OK,

    /** Under AUTO a mosh start failed `TimedOut` or `NotInstalled { mosh-server }`: terminals use SSH. */
    BLOCKED,
}

/**
 * The program probe's answer about `mosh-server` (`HostConnection.mosh_server()`, API 14): its [path],
 * null when it is not installed or the query failed, and how long the answer took ([roundTripMs]),
 * which is the program probe's round trip when this query ran it.
 */
data class MoshServerAnswer(val path: String?, val roundTripMs: Long)

/** How a new terminal opens. */
data class OpenPlan(
    val transport: TerminalTransport,
    /** Mosh only: the whole start's budget, counted from the open; null keeps Rust's 15 s default. */
    val moshBudgetMs: UInt? = null,
    /** AUTO chose mosh: a `TimedOut` or missing `mosh-server` before `Connected` retries over SSH on the same terminal. */
    val fallbackEligible: Boolean = false,
    /**
     * AUTO opened SSH while UDP is untested: a mosh terminal for the same target starts in the
     * background, and the terminal swaps to it once it is `Connected`.
     */
    val background: Boolean = false,
)

/**
 * The choice table (contracts.md, "Instant opens"). Explicit `SSH` and `MOSH` are never
 * second-guessed (a mosh failure is shown as it is). Under `AUTO` nothing waits for UDP:
 * - `BLOCKED`, or a probe that says there is no `mosh-server`: SSH.
 * - `OK`: mosh, with [AUTO_MOSH_BUDGET_MS] and the SSH fallback in case the link changed since.
 * - `UNKNOWN`, tmux or herdr: SSH at once, and a mosh terminal in the background to swap to.
 * - `UNKNOWN`, the login shell (two shells cannot be swapped): mosh when the probe found
 *   `mosh-server`, with a budget of [shellMoshBudgetMs] and the SSH fallback; SSH while the probe
 *   has not answered.
 *
 * What a target ran over before (reattach) is deliberately not an input.
 */
fun planOpen(pref: TransportPref, target: TerminalTarget, verdict: UdpVerdict, moshServer: MoshServerAnswer?): OpenPlan = when (pref) {
    TransportPref.SSH -> OpenPlan(TerminalTransport.SSH)
    TransportPref.MOSH -> OpenPlan(TerminalTransport.MOSH)
    TransportPref.AUTO -> when {
        verdict == UdpVerdict.BLOCKED -> OpenPlan(TerminalTransport.SSH)
        moshServer != null && moshServer.path == null -> OpenPlan(TerminalTransport.SSH)
        verdict == UdpVerdict.OK -> OpenPlan(TerminalTransport.MOSH, AUTO_MOSH_BUDGET_MS, fallbackEligible = true)
        target != TerminalTarget.Shell -> OpenPlan(TerminalTransport.SSH, background = true)
        moshServer != null -> OpenPlan(TerminalTransport.MOSH, shellMoshBudgetMs(moshServer.roundTripMs), fallbackEligible = true)
        else -> OpenPlan(TerminalTransport.SSH)
    }
}

/** AUTO's mosh budget once UDP is known to work on this connection (the whole start; explicit Mosh keeps 15 s). */
const val AUTO_MOSH_BUDGET_MS = 5_000u

/** The floor of a shell's mosh budget while UDP is untested. */
const val SHELL_MOSH_FLOOR_MS = 700L

/** The ceiling of a shell's mosh budget: never more than an explicit Mosh choice gets. */
const val SHELL_MOSH_CEILING_MS = 15_000L

/**
 * A login shell's mosh budget while UDP is untested: `max(700 ms, 6 × the program probe's round
 * trip)` (a bootstrap is a few round trips, and the first datagram one more), at most 15 s.
 */
fun shellMoshBudgetMs(probeRoundTripMs: Long): UInt =
    (probeRoundTripMs * 6).coerceIn(SHELL_MOSH_FLOOR_MS, SHELL_MOSH_CEILING_MS).toUInt()

/** The program whose absence is mosh itself being unavailable; any other missing program is not a mosh problem. */
const val MOSH_SERVER_PROGRAM = "mosh-server"

/**
 * A mosh start that failed this way says UDP (or mosh) does not work on the host: AUTO falls back
 * to SSH, and the connection's verdict becomes `BLOCKED`. A missing `tmux` or `herdr` would fail
 * over SSH the same way, and says nothing about mosh.
 */
fun isMoshFallback(failure: SessionFailure): Boolean =
    failure is SessionFailure.TimedOut || (failure is SessionFailure.NotInstalled && failure.program == MOSH_SERVER_PROGRAM)

/** The muted line on the host screen while the connection's verdict is `BLOCKED`. */
const val UDP_BLOCKED_LINE =
    "Mosh can't reach this host over UDP, so terminals use SSH. On a Mac, run or2-pair --check for the fix."

/** Link health older than this greys the transport badge (contract: `since_heard_ms > 5000`). */
const val STALE_HEARD_MS = 5_000uL

/** "Last heard 12 s ago" once nothing has been heard for more than five seconds; else null. */
fun linkStaleLabel(health: LinkHealth?): String? {
    if (health == null || health.sinceHeardMs <= STALE_HEARD_MS) return null
    return "Last heard ${health.sinceHeardMs / 1000uL} s ago"
}

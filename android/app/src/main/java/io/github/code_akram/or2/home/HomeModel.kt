package io.github.code_akram.or2.home

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.session.hostStateMessage

/** The status dot on a host card's server icon. */
enum class HostDot { NONE, CONNECTED, ATTENTION, CONNECTING, FAILED }

/**
 * What a host card shows besides the host: connection [progress] replaces the address line in
 * place (with a small spinner in the icon slot), a [failure] explains itself in the danger
 * colour, and the [dot] summarises the connection.
 */
data class HostCardStatus(val progress: String?, val failure: String?, val dot: HostDot) {
    val spinning get() = progress != null && dot == HostDot.CONNECTING
}

/** A host as the Home screen lists it. */
data class HostCard(val host: Host, val status: HostCardStatus, val link: LinkStatus)

/**
 * The card status for a connection [state] (null: no connection). [unlocking] is true between
 * the tap and the key being unlocked (the biometric prompt); [blockedAgents] makes a connected
 * host's dot an attention dot.
 */
fun hostCardStatus(state: HostState?, unlocking: Boolean, blockedAgents: Int): HostCardStatus {
    val link = linkStatus(state)
    return when {
        unlocking && (link == LinkStatus.NOT_CONNECTED || link == LinkStatus.FAILED) ->
            HostCardStatus("Unlocking key…", null, HostDot.CONNECTING)
        state == HostState.Connecting -> HostCardStatus("Checking server…", null, HostDot.CONNECTING)
        state == HostState.Authenticating -> HostCardStatus("Authenticating…", null, HostDot.CONNECTING)
        state is HostState.AwaitingHostKeyDecision -> HostCardStatus(hostStateMessage(state), null, HostDot.ATTENTION)
        link == LinkStatus.CONNECTED ->
            HostCardStatus(null, null, if (blockedAgents > 0) HostDot.ATTENTION else HostDot.CONNECTED)
        link == LinkStatus.FAILED -> HostCardStatus(null, state?.let(::hostStateMessage), HostDot.FAILED)
        else -> HostCardStatus(null, null, HostDot.NONE)
    }
}

/** `user@host:port` of the first (preferred) address, with `+N` for the others. */
fun hostAddressLine(host: Host): String {
    val first = host.addresses.first()
    val more = host.addresses.size - 1
    return "${host.username}@${first.hostname}:${first.port}" + if (more > 0) " +$more" else ""
}

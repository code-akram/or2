package io.github.code_akram.or2.home

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.session.hostFailureDetail
import io.github.code_akram.or2.session.hostStateMessage

/** The status dot on a host card's server icon. */
enum class HostDot { NONE, CONNECTED, ATTENTION, CONNECTING, FAILED }

/**
 * What a host card shows besides the host: connection [progress] replaces the address line in
 * place (with a small spinner in the icon slot), a [failure] explains itself in the danger
 * colour, and the [dot] summarises the connection. A host that was unreachable explains each of its
 * addresses in [detail] (muted mono, one line per address): which one did what.
 */
data class HostCardStatus(
    val progress: String?, val failure: String?, val dot: HostDot, val asleep: Boolean = false, val detail: String? = null,
) {
    val spinning get() = progress != null && dot == HostDot.CONNECTING
}

/** A host as the Home screen lists it; [address] is its address line ([hostAddressLine]). */
data class HostCard(val host: Host, val status: HostCardStatus, val link: LinkStatus, val address: String = hostAddressLine(host))

/**
 * The card status for a connection [state] (null: no connection). [unlocking] is true between
 * the tap and the key being unlocked (the biometric prompt); [blockedAgents] makes a connected
 * host's dot an attention dot.
 */
fun hostCardStatus(
    state: HostState?, unlocking: Boolean, blockedAgents: Int, sleeps: Boolean = false, addresses: List<HostEndpoint> = emptyList(),
): HostCardStatus {
    val link = linkStatus(state, sleeps)
    val detail = hostFailureDetail(state, addresses)
    return when {
        unlocking && link.canConnect ->
            HostCardStatus("Unlocking key…", null, HostDot.CONNECTING)
        state == HostState.Connecting -> HostCardStatus("Checking server…", null, HostDot.CONNECTING)
        state == HostState.Authenticating -> HostCardStatus("Authenticating…", null, HostDot.CONNECTING)
        state is HostState.AwaitingHostKeyDecision -> HostCardStatus(hostStateMessage(state), null, HostDot.ATTENTION)
        link == LinkStatus.CONNECTED ->
            HostCardStatus(null, null, if (blockedAgents > 0) HostDot.ATTENTION else HostDot.CONNECTED)
        link == LinkStatus.FAILED -> HostCardStatus(null, state?.let(::hostStateMessage), HostDot.FAILED, detail = detail)
        // A sleeping host that went quiet: muted, no failure colour, no dot (what each address did still shows).
        link == LinkStatus.ASLEEP -> HostCardStatus(null, null, HostDot.NONE, asleep = true, detail = detail)
        else -> HostCardStatus(null, null, HostDot.NONE)
    }
}

/**
 * `user@host:port` of the address in use while [state] is connected (`HostState.Connected.addressIndex`), else of
 * the first (preferred) one, with `+N` for the others.
 */
fun hostAddressLine(host: Host, state: HostState? = null): String {
    val index = (state as? HostState.Connected)?.addressIndex?.toInt() ?: 0
    val shown = host.addresses.getOrNull(index) ?: host.addresses.first()
    val more = host.addresses.size - 1
    return "${host.username}@${shown.hostname}:${shown.port}" + if (more > 0) " +$more" else ""
}

/**
 * The detail line under a Home session card's title: [cwd], the working directory of the terminal's pane when it is
 * known, else for a herdr terminal [herdrDetail] of its session's live [view], else empty (the card keeps its
 * height). Never `user@host`: the card's pill already names the host.
 */
fun sessionDetail(cwd: String?, view: HerdrView?): String = cwd ?: view?.let(::herdrDetail) ?: ""

/**
 * What a herdr terminal shows, in one line: the focused pane's agent label when an agent runs there, else the
 * focused pane's cwd, else null. herdr's focus is shared, so the focused pane is what the session's terminal shows.
 */
fun herdrDetail(view: HerdrView): String? {
    val focused = view.focusedPaneId ?: return null
    val agent = view.agents.firstOrNull { it.paneId == focused }
    val pane = view.panes.firstOrNull { it.paneId == focused }
    val labels = listOfNotNull(agent?.displayAgent, agent?.name, agent?.agent, pane?.agent)
    return labels.firstOrNull { it.isNotBlank() } ?: listOfNotNull(pane?.cwd, agent?.cwd).firstOrNull { it.isNotBlank() }
}

/**
 * Whether a tap on [host]'s card, or on its session button, starts its connection (the usual unlock): it has a
 * key, nothing is connecting or connected ([link]), and no other unlock runs ([busy]).
 */
fun tapConnects(host: Host, link: LinkStatus, busy: Boolean): Boolean = !busy && host.keyId != null && link.canConnect

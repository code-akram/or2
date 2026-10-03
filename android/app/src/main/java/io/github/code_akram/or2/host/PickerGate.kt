package io.github.code_akram.or2.host

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.home.hostCardStatus
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.linkStatus

/**
 * What the session picker shows instead of its lists while its host is not connected. The picker that a Home host
 * card's header opens shows at once, before its host has connected: first this, then the lists.
 */
sealed interface PickerGate {
    val host: String

    /**
     * Connecting: the host's own progress line, as its Home card shows it (`Unlocking key…`, `Checking server…`,
     * `Authenticating…`), with an accent spinner when [spinning]. A host-key decision waits without one (the
     * trust dialog is up over the sheet).
     */
    data class Connecting(override val host: String, val progress: String, val spinning: Boolean) : PickerGate

    /**
     * Not connected: why ([reason]: a failure in `danger` when [failed], else muted: `Asleep`, `Not connected`),
     * what each address did ([detail], muted mono), and the [action] that moves on. [enabled] is false while
     * another unlock runs.
     */
    data class Stopped(
        override val host: String, val reason: String, val failed: Boolean, val detail: String?, val action: GateAction,
        val enabled: Boolean,
    ) : PickerGate
}

/** The one action a [PickerGate.Stopped] offers. */
enum class GateAction(val label: String) {
    /** A failed or asleep host: connect again (the usual unlock). */
    RETRY("Retry"),

    /** A host that is simply not connected (a cancelled unlock, a disconnect): connect it. */
    CONNECT("Connect"),

    /** A host without a key cannot connect: edit it to choose one. */
    SELECT_KEY("Select a key"),
}

/**
 * The gate for [host] in [state] (null: no connection), or null once it is connected. [unlocking]: the tap is
 * waiting on the biometric unlock; [busy]: an unlock or connect is running, so nothing new can start.
 */
fun pickerGate(host: Host, state: HostState?, unlocking: Boolean, busy: Boolean): PickerGate? {
    val link = linkStatus(state, host.sleeps)
    if (link == LinkStatus.CONNECTED) return null
    val status = hostCardStatus(state, unlocking, 0, host.sleeps, host.addresses)
    status.progress?.let { return PickerGate.Connecting(host.label, it, status.spinning) }
    if (host.keyId == null) {
        return PickerGate.Stopped(host.label, "This host has no SSH key yet.", false, null, GateAction.SELECT_KEY, enabled = true)
    }
    return when (link) {
        LinkStatus.FAILED ->
            PickerGate.Stopped(host.label, status.failure ?: link.label, true, status.detail, GateAction.RETRY, !busy)
        LinkStatus.ASLEEP ->
            PickerGate.Stopped(host.label, LinkStatus.ASLEEP.label, false, status.detail, GateAction.RETRY, !busy)
        else -> PickerGate.Stopped(host.label, LinkStatus.NOT_CONNECTED.label, false, null, GateAction.CONNECT, !busy)
    }
}

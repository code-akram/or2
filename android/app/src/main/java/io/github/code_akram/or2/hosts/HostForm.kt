package io.github.code_akram.or2.hosts

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.TransportPref

/** An address as typed in the form: both fields are text until validated. */
data class AddressDraft(val hostname: String, val port: String) {
    companion object {
        fun of(endpoint: HostEndpoint) = AddressDraft(endpoint.hostname, endpoint.port.toString())
    }
}

fun hostFieldError(value: String): String? = when {
    value.isBlank() -> "Enter a value."
    value.trim().any { it.isWhitespace() || it.isISOControl() } -> "Remove internal whitespace or control characters."
    else -> null
}

fun portError(value: String): String? = if (value.toIntOrNull() in 1..65535) null else "Port must be 1-65535."

fun validAddress(address: AddressDraft) = hostFieldError(address.hostname) == null && portError(address.port) == null

fun validHost(label: String, addresses: List<AddressDraft>, username: String): Boolean =
    label.isNotBlank() && addresses.size in 1..Host.MAX_ADDRESSES && addresses.all(::validAddress) &&
        hostFieldError(username) == null

/** Moves the entry at [index] by [delta] places; out-of-range moves leave the list unchanged. */
fun <T> List<T>.moved(index: Int, delta: Int): List<T> {
    val target = index + delta
    if (index !in indices || target !in indices) return this
    return toMutableList().apply { add(target, removeAt(index)) }
}

/**
 * Whether saving [updated] over [previous] must end a live connection: its destination, login or
 * key changed. A label or inbox change leaves the connection (and its trust) alone.
 */
fun connectionAffectedBy(previous: Host, updated: Host): Boolean =
    previous.addresses != updated.addresses || previous.username != updated.username || previous.keyId != updated.keyId

/** The transport choices in the form's segmented control, in order. */
val TransportChoices = listOf(TransportPref.AUTO, TransportPref.SSH, TransportPref.MOSH)

fun transportLabel(pref: TransportPref) = when (pref) {
    TransportPref.AUTO -> "Auto"
    TransportPref.SSH -> "SSH"
    TransportPref.MOSH -> "Mosh"
}

/** One muted sentence under the control. */
fun transportExplanation(pref: TransportPref) = when (pref) {
    TransportPref.AUTO -> "Mosh when the host has mosh-server; SSH if mosh cannot connect."
    TransportPref.SSH -> "Terminals always use SSH and end when the connection drops."
    TransportPref.MOSH -> "Terminals always use mosh and survive network changes. Needs mosh-server and UDP."
}

/**
 * Under the address list. Mosh pins to the address SSH actually reached (it never resolves again
 * and never switches address), so the one that works from every network should come first.
 */
const val ADDRESS_ORDER_HINT =
    "In order of preference. All are tried; the first to answer wins. Mosh stays on the address SSH " +
        "reached, so list the one that works on every network first."

/** Under the "Host sleeps when idle" toggle. */
const val SLEEPS_EXPLANATION =
    "For a laptop that sleeps: when its connection is lost it shows as asleep, and no reconnect is offered."

/** Under the "Wake probe" toggle: one line. */
const val WAKE_PROBE_EXPLANATION =
    "Knocks on the SSH port before every connect, so a Bonjour Sleep Proxy (an Apple TV, a HomePod) wakes the host."

/**
 * Under the MAC address field: what Wake needs, and the limit the app cannot get round (a Mac with its lid closed on
 * battery turns Wi-Fi off, so no packet or knock reaches it) with the ways out.
 */
const val MAC_ADDRESS_HINT =
    "Optional, for Wake-on-LAN on the same network (the Mac needs \"Wake for network access\"). A closed laptop on " +
        "battery cannot be woken: keep it awake on power (a keep-awake tool, or pmset disablesleep), or run long agents " +
        "on an always-on host."

private val MacPattern = Regex("[0-9A-Fa-f]{2}([:-])[0-9A-Fa-f]{2}(\\1[0-9A-Fa-f]{2}){4}")

/**
 * The MAC address as stored: lowercase with `:`, or null when [text] is blank (no MAC address) or not one. Accepts
 * `aa:bb:cc:dd:ee:ff` or `-` separators (not mixed), any case, surrounding whitespace ignored.
 */
fun normalizedMac(text: String): String? {
    val trimmed = text.trim()
    if (!MacPattern.matches(trimmed)) return null
    return trimmed.lowercase().replace('-', ':')
}

/** The inline error under the MAC address field; null for an empty field (the address is optional) or a valid one. */
fun macError(text: String): String? =
    if (text.isBlank() || normalizedMac(text) != null) null else "Use six hex pairs: aa:bb:cc:dd:ee:ff."

package io.github.code_akram.or2.hosts

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint

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

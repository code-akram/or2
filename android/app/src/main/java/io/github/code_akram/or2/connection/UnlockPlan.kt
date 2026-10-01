package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import kotlinx.coroutines.CancellationException

/** Hosts that share one key record, so one biometric prompt unlocks all of them. */
data class UnlockGroup(val keyId: String, val hosts: List<Host>)

/** [groups] in first-requested order; [withoutKey] cannot connect until a key is selected. */
data class UnlockPlan(val groups: List<UnlockGroup>, val withoutKey: List<Host>)

/** One group per distinct key record, hosts in request order, each host at most once. */
fun planUnlock(hosts: List<Host>): UnlockPlan {
    val unique = hosts.distinctBy { it.id }
    val withoutKey = unique.filter { it.keyId == null }
    val groups = unique.filter { it.keyId != null }
        .groupBy { it.keyId!! }
        .map { (keyId, members) -> UnlockGroup(keyId, members) }
    return UnlockPlan(groups, withoutKey)
}

/**
 * Decrypts a key record behind the user's biometric and hands the plaintext to [block].
 * The implementation wipes the array when [block] returns or throws, and prompts once per call.
 */
interface KeyUnlocker {
    suspend fun <T> withKey(keyId: String, block: suspend (ByteArray) -> T): T
}

/** Thrown for a host that has no key selected. */
class MissingKeyException(val hosts: List<Host>) : Exception("Select a stored key for ${hosts.joinToString { it.label }} first.")

/**
 * Connects every requested host that is not already live: one unlock per distinct key record,
 * every host sharing the key connected from that one decryption. A failed or cancelled unlock
 * ends the batch; a connect error does not stop the other groups. The first error is rethrown at
 * the end, and hosts without a key are reported as [MissingKeyException]. Call on main.
 */
suspend fun connectGrouped(hosts: List<Host>, connections: HostConnections, unlocker: KeyUnlocker) {
    val plan = planUnlock(hosts.filterNot { connections.isLive(it.id) })
    var failure: Exception? = null
    for (group in plan.groups) {
        try {
            unlocker.withKey(group.keyId) { key ->
                try {
                    connections.connect(group.hosts, key)
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Exception) {
                    if (failure == null) failure = error
                }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            if (failure == null) failure = error
            break
        }
    }
    if (failure == null && plan.withoutKey.isNotEmpty()) failure = MissingKeyException(plan.withoutKey)
    failure?.let { throw it }
}

package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host

/** Hosts whose SSH connection was lost, offered one grouped unlock; [prompts] is one biometric per distinct key. */
data class ReconnectOffer(val hosts: List<Host>, val prompts: Int)

/**
 * The offer shown when the app returns to the foreground: stored hosts that are in the inbox, have
 * a key, are not live, and whose connection was lost (it had been up and then failed; a deliberate
 * disconnect or a connect that never worked is not a loss). A host marked as sleeping is never
 * offered: its lost connection reads "asleep". No key is retained to do this in the background: the
 * offer only asks (as a non-modal chip), and accepting runs the usual grouped unlock
 * ([connectGrouped]). Null when there is nothing to offer.
 */
fun reconnectOffer(hosts: List<Host>, connections: Map<Long, ActiveHost>): ReconnectOffer? =
    reconnectOffer(hosts) { id -> connections[id]?.takeIf { it.wasLost && !it.isLive } != null }

/** [lost] says whether a host id's connection was lost and is no longer live. */
fun reconnectOffer(hosts: List<Host>, lost: (Long) -> Boolean): ReconnectOffer? {
    val candidates = hosts.filter { it.showInInbox && it.keyId != null && !it.sleeps && lost(it.id) }
    if (candidates.isEmpty()) return null
    return ReconnectOffer(candidates, planUnlock(candidates).groups.size)
}

package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.PrefStore
import io.github.code_akram.or2.data.Host
import java.security.MessageDigest

/**
 * The `mosh-server`s this app started and has not seen end, as `(host id, pid, destination)` entries
 * in app-private preferences (Rust has no storage). A pid is not a secret and no key is kept: the
 * entry exists only so that a server orphaned by the process's death (its key died with the process,
 * and `mosh-server` has no idle timeout) can be stopped over the next SSH connection to that host
 * (`HostConnection.stop_mosh_server`, which only signals a process the host names `mosh-server`).
 *
 * A pid only names a process on the machine and account that started it, and a stored host keeps its
 * id when its address list or login is edited. Every entry therefore carries [moshIdentity]: a stable
 * digest of the ordered address list, the ports and the username the server was started through.
 * [pids] returns only the entries whose identity equals the host's current one, so a stop is never
 * sent to a destination or account other than the one that created the record (where the same number
 * may be somebody else's live `mosh-server`). An edit of either also purges the host's entries
 * ([purge], see `HostConnections.hostEdited`). Entries written before identities existed carry none and
 * are dropped: they cannot be tied to a destination.
 *
 * Recorded the moment Rust names a mosh session's server (`SessionListener.on_server_pid`: before the
 * server has seen its client, so before `Connected`), and on disk before [record] returns, so a process
 * death at any moment after still leaves the pid to stop. Cleared when the session closes by the user's
 * disconnect or the remote's own exit, when the server was stopped at the next connection, or when its
 * host is deleted. A session that closed `Failed` stays recorded: the stop Rust tried may not have
 * reached the host, and trying again is harmless.
 */
class MoshServerLedger(private val store: PrefStore) {
    private data class Entry(val hostId: Long, val pid: UInt, val identity: String)

    private val entries: MutableSet<Entry> = decode(store.getString(KEY)).toMutableSet()

    /** On disk when this returns (`PrefStore.putStringDurably`, a blocking write): call it off the main thread. */
    @Synchronized
    fun record(host: Host, pid: UInt) {
        if (entries.add(Entry(host.id, pid, host.moshIdentity()))) save(durably = true)
    }

    @Synchronized
    fun clear(host: Host, pid: UInt) {
        if (entries.remove(Entry(host.id, pid, host.moshIdentity()))) save()
    }

    /** Forgets every server of a host that no longer exists, or whose destination or login changed. */
    @Synchronized
    fun purge(hostId: Long) {
        if (entries.removeAll { it.hostId == hostId }) save()
    }

    /** The recorded pids of [host] that were started through its current destination and login, oldest first. */
    @Synchronized
    fun pids(host: Host): List<UInt> {
        val identity = host.moshIdentity()
        return entries.filter { it.hostId == host.id && it.identity == identity }.map { it.pid }
    }

    /** Every recorded pid of [hostId], whatever destination it was started through (diagnostics and tests). */
    @Synchronized
    fun allPids(hostId: Long): List<UInt> = entries.filter { it.hostId == hostId }.map { it.pid }

    /**
     * Only a new record must reach the disk at once. A clear that a process death loses costs one
     * repeated stop at the next connection, which is harmless.
     */
    private fun save(durably: Boolean = false) {
        val text = if (entries.isEmpty()) null else entries.joinToString(",") { "${it.hostId}:${it.pid}:${it.identity}" }
        if (durably) store.putStringDurably(KEY, text) else store.putString(KEY, text)
    }

    private companion object {
        const val KEY = "mosh_servers"

        /** Entries this app did not write are dropped; a zero pid names no process, and an entry needs its identity. */
        fun decode(text: String?): List<Entry> = text.orEmpty().split(",").mapNotNull { entry ->
            val parts = entry.split(":")
            val host = parts.getOrNull(0)?.toLongOrNull()
            val pid = parts.getOrNull(1)?.toUIntOrNull()
            val identity = parts.getOrNull(2)?.takeIf { it.isNotEmpty() && it.all { c -> c in '0'..'9' || c in 'a'..'f' } }
            if (parts.size == 3 && host != null && pid != null && pid != 0u && identity != null) Entry(host, pid, identity) else null
        }
    }
}

/**
 * Who a recorded server belongs to: a digest of the host's ordered address list (names and ports) and
 * its username, the only things that decide which machine and account a pid refers to. The label, key,
 * inbox flag and transport are not part of it (they do not move the server).
 */
fun Host.moshIdentity(): String {
    val text = buildString {
        append(username.length).append(':').append(username)
        for (endpoint in addresses) {
            append('|').append(endpoint.hostname.length).append(':').append(endpoint.hostname).append(':').append(endpoint.port)
        }
    }
    val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
    return digest.take(16).joinToString("") { "%02x".format(it) }
}

/**
 * The recorded servers of a host that are orphans now: those no session of this process still runs
 * on ([live]: a mosh session outlives a lost SSH connection, so a reconnect must not stop its own
 * server). Pure, so the decision is testable on its own.
 */
fun orphanedServers(recorded: List<UInt>, live: Set<UInt>): List<UInt> = recorded.filter { it !in live }

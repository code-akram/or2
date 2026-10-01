package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.PrefStore

/**
 * The `mosh-server`s this app started and has not seen end, as `(host id, pid)` pairs in app-private
 * preferences (Rust has no storage). A pid is not a secret and no key is kept: the entry exists only
 * so that a server orphaned by the process's death (its key died with the process, and `mosh-server`
 * has no idle timeout) can be stopped over the next SSH connection to that host
 * (`HostConnection.stop_mosh_server`, which only signals a process the host names `mosh-server`).
 *
 * Recorded when a mosh session connects; cleared when it closes by the user's disconnect or the
 * remote's own exit, when the server was stopped at the next connection, or when its host is deleted.
 * A session that closed `Failed` stays recorded: the stop Rust tried may not have reached the host, and
 * trying again is harmless.
 */
class MoshServerLedger(private val store: PrefStore) {
    private val entries: MutableSet<Pair<Long, UInt>> = decode(store.getString(KEY)).toMutableSet()

    @Synchronized
    fun record(hostId: Long, pid: UInt) {
        if (entries.add(hostId to pid)) save()
    }

    @Synchronized
    fun clear(hostId: Long, pid: UInt) {
        if (entries.remove(hostId to pid)) save()
    }

    /** Forgets every server of a host that no longer exists. */
    @Synchronized
    fun purge(hostId: Long) {
        if (entries.removeAll { it.first == hostId }) save()
    }

    /** The recorded pids of [hostId], oldest first. */
    @Synchronized
    fun pids(hostId: Long): List<UInt> = entries.filter { it.first == hostId }.map { it.second }

    private fun save() = store.putString(KEY, if (entries.isEmpty()) null else entries.joinToString(",") { "${it.first}:${it.second}" })

    private companion object {
        const val KEY = "mosh_servers"

        /** Entries this app did not write are dropped; a zero pid names no process. */
        fun decode(text: String?): List<Pair<Long, UInt>> = text.orEmpty().split(",").mapNotNull { entry ->
            val parts = entry.split(":")
            val host = parts.getOrNull(0)?.toLongOrNull()
            val pid = parts.getOrNull(1)?.toUIntOrNull()
            if (parts.size == 2 && host != null && pid != null && pid != 0u) host to pid else null
        }
    }
}

/**
 * The recorded servers of a host that are orphans now: those no session of this process still runs
 * on ([live]: a mosh session outlives a lost SSH connection, so a reconnect must not stop its own
 * server). Pure, so the decision is testable on its own.
 */
fun orphanedServers(recorded: List<UInt>, live: Set<UInt>): List<UInt> = recorded.filter { it !in live }

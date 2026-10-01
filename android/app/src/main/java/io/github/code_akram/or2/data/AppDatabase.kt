package io.github.code_akram.or2.data

import androidx.room.ColumnInfo
import androidx.room.Dao
import androidx.room.Database
import androidx.room.Embedded
import androidx.room.Entity
import androidx.room.ForeignKey
import androidx.room.Index
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.Relation
import androidx.room.RoomDatabase
import androidx.room.Transaction
import io.github.code_akram.or2.ffi.PublicKeyInfo
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

/** Which transport a host's terminals prefer. AUTO uses mosh when the host has `mosh-server`, else SSH. */
enum class TransportPref { AUTO, SSH, MOSH }

@Entity(tableName = "keys")
data class KeyRecord(
    @PrimaryKey val id: String,
    val label: String,
    val algorithm: String,
    val openssh: String,
    val fingerprint: String,
    val comment: String,
    val ciphertext: ByteArray,
    val iv: ByteArray,
)

@Entity(
    tableName = "hosts",
    foreignKeys = [ForeignKey(
        entity = KeyRecord::class, parentColumns = ["id"], childColumns = ["keyId"],
        onDelete = ForeignKey.SET_NULL,
    )],
    indices = [Index("keyId")],
)
data class HostRecord(
    @PrimaryKey(autoGenerate = true) val id: Long = 0,
    val label: String,
    val username: String,
    val keyId: String?,
    @ColumnInfo(defaultValue = "1") val showInInbox: Boolean = true,
    @ColumnInfo(defaultValue = "'AUTO'") val transport: TransportPref = TransportPref.AUTO,
    /**
     * The host goes to sleep when idle (a laptop): a lost SSH connection is "asleep", not a failure
     * to retry, and no reconnect is offered for it on return.
     */
    @ColumnInfo(name = "sleeps", defaultValue = "0") val sleeps: Boolean = false,
    /**
     * Epoch milliseconds until which AUTO skips mosh for this host (mosh failed to reach it over UDP);
     * 0 is no memory. Cleared when the transport preference or the addresses change.
     */
    @ColumnInfo(name = "mosh_failed_until", defaultValue = "0") val moshFailedUntil: Long = 0,
)

/** One of a host's addresses, tried in `position` order (0 is preferred). */
@Entity(
    tableName = "host_addresses",
    primaryKeys = ["hostId", "position"],
    foreignKeys = [ForeignKey(
        entity = HostRecord::class, parentColumns = ["id"], childColumns = ["hostId"],
        onDelete = ForeignKey.CASCADE,
    )],
)
data class HostAddressRecord(
    val hostId: Long,
    val position: Int,
    val hostname: String,
    val port: Int,
)

/** A host address as the UI edits it and the holder connects to it. */
data class HostEndpoint(val hostname: String, val port: Int)

/** Room's join of a host and its address rows; [toHost] orders them by position. */
data class HostWithAddresses(
    @Embedded val record: HostRecord,
    @Relation(parentColumn = "id", entityColumn = "hostId") val addresses: List<HostAddressRecord>,
) {
    fun toHost() = Host(record, addresses.sortedBy { it.position }.map { HostEndpoint(it.hostname, it.port) })
}

/** A host with its ordered address list (1..[MAX_ADDRESSES]); trust belongs to the host. */
data class Host(val record: HostRecord, val addresses: List<HostEndpoint>) {
    val id get() = record.id
    val label get() = record.label
    val username get() = record.username
    val keyId get() = record.keyId
    val showInInbox get() = record.showInInbox
    val transport get() = record.transport
    val sleeps get() = record.sleeps
    val moshFailedUntil get() = record.moshFailedUntil

    /** `host:port` summaries for lists and dialogs. */
    val addressSummary get() = addresses.joinToString(", ") { "${it.hostname}:${it.port}" }

    companion object {
        const val MAX_ADDRESSES = 8
    }
}

@Entity(
    tableName = "trusted_host_keys",
    primaryKeys = ["hostId", "openssh"],
    foreignKeys = [ForeignKey(
        entity = HostRecord::class, parentColumns = ["id"], childColumns = ["hostId"],
        onDelete = ForeignKey.CASCADE,
    )],
)
data class TrustedHostKey(
    val hostId: Long,
    val openssh: String,
    val fingerprint: String,
    val algorithm: String,
)

interface TrustStore {
    suspend fun trustedKeys(hostId: Long): List<String>

    /** Replaces the host's trusted keys; fails if its address list changed since [host] was read. */
    suspend fun replaceTrust(host: Host, presented: PublicKeyInfo)
}

/** The per-host memory of a mosh failure that AUTO honours across connections and restarts. */
interface MoshFailureStore {
    /** AUTO skips mosh for [hostId] until [until] (epoch milliseconds). */
    suspend fun markMoshFailed(hostId: Long, until: Long)

    /** Forgets the failure: the next AUTO terminal tries mosh again. */
    suspend fun clearMoshFailure(hostId: Long)
}

@Dao
abstract class AppDao : TrustStore, MoshFailureStore {
    @Transaction
    @Query("SELECT * FROM hosts ORDER BY label COLLATE NOCASE")
    abstract fun hostRows(): Flow<List<HostWithAddresses>>

    @Transaction
    @Query("SELECT * FROM hosts WHERE id = :id")
    abstract suspend fun hostRow(id: Long): HostWithAddresses?

    fun hosts(): Flow<List<Host>> = hostRows().map { rows -> rows.map { it.toHost() } }

    suspend fun host(id: Long): Host? = hostRow(id)?.toHost()

    @Query("SELECT * FROM keys ORDER BY label COLLATE NOCASE")
    abstract fun keys(): Flow<List<KeyRecord>>

    @Query("SELECT * FROM keys WHERE id = :id")
    abstract suspend fun key(id: String): KeyRecord?

    @Insert(onConflict = OnConflictStrategy.ABORT)
    abstract suspend fun insertKey(key: KeyRecord)

    @Query("DELETE FROM keys WHERE id = :id")
    abstract suspend fun deleteKey(id: String)

    @Insert
    abstract suspend fun insertHost(host: HostRecord): Long

    @Insert
    abstract suspend fun insertAddresses(addresses: List<HostAddressRecord>)

    @Query("DELETE FROM host_addresses WHERE hostId = :hostId")
    abstract suspend fun deleteAddresses(hostId: Long)

    // `mosh_failed_until` is deliberately not here: an edit never touches the failure memory except
    // through `saveHost`, which clears it when the transport or the addresses change.
    @Query("UPDATE hosts SET label = :label, username = :username, keyId = :keyId, showInInbox = :showInInbox, transport = :transport, sleeps = :sleeps WHERE id = :id")
    abstract suspend fun updateHost(id: Long, label: String, username: String, keyId: String?, showInInbox: Boolean, transport: TransportPref, sleeps: Boolean)

    @Query("UPDATE hosts SET mosh_failed_until = :until WHERE id = :hostId")
    abstract override suspend fun markMoshFailed(hostId: Long, until: Long)

    override suspend fun clearMoshFailure(hostId: Long) = markMoshFailed(hostId, 0)

    @Query("DELETE FROM hosts WHERE id = :id")
    abstract suspend fun deleteHost(id: Long)

    @Query("SELECT openssh FROM trusted_host_keys WHERE hostId = :hostId")
    abstract override suspend fun trustedKeys(hostId: Long): List<String>

    @Query("DELETE FROM trusted_host_keys WHERE hostId = :hostId")
    abstract suspend fun clearTrust(hostId: Long)

    @Insert
    abstract suspend fun insertTrust(key: TrustedHostKey)

    @Transaction
    override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) {
        val stored = host(host.id)
        check(stored != null && stored.addresses == host.addresses) {
            "Host was edited or deleted while waiting for trust."
        }
        clearTrust(host.id)
        insertTrust(TrustedHostKey(host.id, presented.openssh, presented.fingerprint, presented.algorithm))
    }

    // A different destination must never inherit the old destination's trust: any change to the
    // ordered address list (a hostname, a port, an added, removed or reordered entry) clears it.
    @Transaction
    open suspend fun saveHost(host: Host, previous: Host?) {
        require(host.addresses.size in 1..Host.MAX_ADDRESSES) { "A host needs 1 to ${Host.MAX_ADDRESSES} addresses." }
        if (previous == null) {
            val id = insertHost(host.record)
            insertAddresses(addressRows(id, host.addresses))
        } else {
            val stored = host(host.id) ?: error("Host was deleted.")
            if (host.addresses != stored.addresses) clearTrust(host.id)
            // A different destination or a new preference is a fresh decision about mosh.
            if (host.addresses != stored.addresses || host.transport != stored.transport) clearMoshFailure(host.id)
            updateHost(host.id, host.label, host.username, host.keyId, host.showInInbox, host.transport, host.sleeps)
            deleteAddresses(host.id)
            insertAddresses(addressRows(host.id, host.addresses))
        }
    }

    /**
     * A new host together with its trusted host key, in one transaction (Easy pair): the key came from a
     * pairing code the user scanned, so the first connection must find it already trusted. A different key
     * presented later is the changed-key path, as for any trusted host.
     */
    @Transaction
    open suspend fun saveHostWithTrust(host: Host, trusted: PublicKeyInfo): Long {
        require(host.addresses.size in 1..Host.MAX_ADDRESSES) { "A host needs 1 to ${Host.MAX_ADDRESSES} addresses." }
        val id = insertHost(host.record.copy(id = 0))
        insertAddresses(addressRows(id, host.addresses))
        insertTrust(TrustedHostKey(id, trusted.openssh, trusted.fingerprint, trusted.algorithm))
        return id
    }

    private fun addressRows(hostId: Long, addresses: List<HostEndpoint>) =
        addresses.mapIndexed { position, address -> HostAddressRecord(hostId, position, address.hostname, address.port) }
}

@Database(
    entities = [HostRecord::class, HostAddressRecord::class, KeyRecord::class, TrustedHostKey::class],
    version = 4, exportSchema = true,
)
abstract class AppDatabase : RoomDatabase() {
    abstract fun dao(): AppDao
}

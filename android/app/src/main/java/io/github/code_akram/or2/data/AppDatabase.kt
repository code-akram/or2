package io.github.code_akram.or2.data

import androidx.room.Dao
import androidx.room.Database
import androidx.room.Entity
import androidx.room.ForeignKey
import androidx.room.Index
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.RoomDatabase
import androidx.room.Transaction
import io.github.code_akram.or2.ffi.PublicKeyInfo
import kotlinx.coroutines.flow.Flow

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
    val hostname: String,
    val port: Int,
    val username: String,
    val keyId: String?,
)

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
    suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo)
}

@Dao
abstract class AppDao : TrustStore {
    @Query("SELECT * FROM hosts ORDER BY label COLLATE NOCASE")
    abstract fun hosts(): Flow<List<HostRecord>>

    @Query("SELECT * FROM hosts WHERE id = :id")
    abstract suspend fun host(id: Long): HostRecord?

    @Query("SELECT * FROM keys ORDER BY label COLLATE NOCASE")
    abstract fun keys(): Flow<List<KeyRecord>>

    @Query("SELECT * FROM keys WHERE id = :id")
    abstract suspend fun key(id: String): KeyRecord?

    @Insert(onConflict = OnConflictStrategy.ABORT)
    abstract suspend fun insertKey(key: KeyRecord)

    @Query("DELETE FROM keys WHERE id = :id")
    abstract suspend fun deleteKey(id: String)

    @Insert
    abstract suspend fun insertHost(host: HostRecord)

    @Query("UPDATE hosts SET label = :label, hostname = :hostname, port = :port, username = :username, keyId = :keyId WHERE id = :id")
    abstract suspend fun updateHost(id: Long, label: String, hostname: String, port: Int, username: String, keyId: String?)

    @Query("DELETE FROM hosts WHERE id = :id")
    abstract suspend fun deleteHost(id: Long)

    @Query("SELECT openssh FROM trusted_host_keys WHERE hostId = :hostId")
    abstract override suspend fun trustedKeys(hostId: Long): List<String>

    @Query("DELETE FROM trusted_host_keys WHERE hostId = :hostId")
    abstract suspend fun clearTrust(hostId: Long)

    @Insert
    abstract suspend fun insertTrust(key: TrustedHostKey)

    @Transaction
    override suspend fun replaceTrust(host: HostRecord, presented: PublicKeyInfo) {
        val stored = host(host.id)
        check(stored != null && stored.hostname == host.hostname && stored.port == host.port) {
            "Host was edited or deleted while waiting for trust."
        }
        clearTrust(host.id)
        insertTrust(TrustedHostKey(host.id, presented.openssh, presented.fingerprint, presented.algorithm))
    }

    // A different destination must never inherit the old destination's trust.
    @Transaction
    open suspend fun saveHost(host: HostRecord, previous: HostRecord?) {
        if (previous == null) {
            insertHost(host)
        } else {
            val stored = host(host.id) ?: error("Host was deleted.")
            if (host.hostname != stored.hostname || host.port != stored.port) clearTrust(host.id)
            updateHost(host.id, host.label, host.hostname, host.port, host.username, host.keyId)
        }
    }
}

@Database(entities = [HostRecord::class, KeyRecord::class, TrustedHostKey::class], version = 1, exportSchema = false)
abstract class AppDatabase : RoomDatabase() {
    abstract fun dao(): AppDao
}

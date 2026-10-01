package io.github.code_akram.or2.data

import androidx.room.testing.MigrationTestHelper
import androidx.sqlite.db.framework.FrameworkSQLiteOpenHelperFactory
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.flow.first
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Migrates a populated shipped-M1 (version 1) database through Room itself. The schemas under
 * `app/schemas` are packaged as test assets. Key records and trust must survive: Keystore-bound
 * keys cannot be recreated, so a destructive migration is never acceptable.
 */
@RunWith(AndroidJUnit4::class)
class MigrationDeviceTest {
    private val name = "migration-test"

    @get:Rule
    val helper = MigrationTestHelper(
        InstrumentationRegistry.getInstrumentation(), AppDatabase::class.java, emptyList(), FrameworkSQLiteOpenHelperFactory(),
    )

    @Test
    fun populatedVersion1MigratesWithoutLosingKeysTrustOrHosts() {
        helper.createDatabase(name, 1).use { db ->
            db.execSQL("INSERT INTO keys VALUES ('key-1', 'Phone key', 'ssh-ed25519', 'ssh-ed25519 AAAA', 'SHA256:fp', 'c', x'0a0b0c', x'0102')")
            db.execSQL("INSERT INTO keys VALUES ('key-2', 'Other key', 'ssh-ed25519', 'ssh-ed25519 BBBB', 'SHA256:fq', '', x'ff', x'03')")
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (1, 'Alpha', 'alpha.invalid', 22, 'u1', 'key-1')")
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (5, 'Beta', 'beta.invalid', 2222, 'u2', 'key-2')")
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (9, 'Gamma', '2001:db8::1', 65535, 'u3', NULL)")
            db.execSQL("INSERT INTO trusted_host_keys VALUES (1, 'ssh-ed25519 H1', 'SHA256:h1', 'ssh-ed25519')")
            db.execSQL("INSERT INTO trusted_host_keys VALUES (1, 'ssh-rsa H1R', 'SHA256:h1r', 'ssh-rsa')")
            db.execSQL("INSERT INTO trusted_host_keys VALUES (5, 'ssh-ed25519 H5', 'SHA256:h5', 'ssh-ed25519')")
        }

        // Room validates the migrated schema against 2.json (columns, defaults, foreign keys, indices).
        helper.runMigrationsAndValidate(name, 2, true, MIGRATION_1_2).use { db ->
            db.query("SELECT hostId, position, hostname, port FROM host_addresses ORDER BY hostId").use { cursor ->
                assertEquals(3, cursor.count)
                val rows = buildList {
                    while (cursor.moveToNext()) add(listOf(cursor.getLong(0), cursor.getInt(1).toLong(), cursor.getString(2), cursor.getInt(3).toLong()))
                }
                assertEquals(listOf(
                    listOf(1L, 0L, "alpha.invalid", 22L), listOf(5L, 0L, "beta.invalid", 2222L), listOf(9L, 0L, "2001:db8::1", 65535L),
                ), rows)
            }
            db.query("SELECT id, label, username, keyId, showInInbox FROM hosts ORDER BY id").use { cursor ->
                assertEquals(3, cursor.count)
                cursor.moveToFirst()
                assertEquals("Alpha", cursor.getString(1))
                assertEquals("key-1", cursor.getString(3))
                assertEquals(1, cursor.getInt(4)) // Existing hosts stay in the inbox.
                cursor.moveToLast()
                assertTrue(cursor.isNull(3))
            }
            db.query("SELECT hostId, openssh FROM trusted_host_keys ORDER BY hostId, openssh").use { cursor ->
                assertEquals(3, cursor.count) // Trust survives: the destination did not change.
            }
            db.query("SELECT id, ciphertext, iv FROM keys ORDER BY id").use { cursor ->
                assertEquals(2, cursor.count)
                cursor.moveToFirst()
                assertEquals("key-1", cursor.getString(0))
                assertArrayEquals(byteArrayOf(0x0a, 0x0b, 0x0c), cursor.getBlob(1))
                assertArrayEquals(byteArrayOf(1, 2), cursor.getBlob(2))
            }
            // Cascades are in force on the migrated tables (enabled explicitly: the helper's handle may not have it on).
            db.execSQL("PRAGMA foreign_keys = ON")
            db.execSQL("DELETE FROM hosts WHERE id = 1")
            db.query("SELECT * FROM host_addresses WHERE hostId = 1").use { assertEquals(0, it.count) }
            db.query("SELECT * FROM trusted_host_keys WHERE hostId = 1").use { assertEquals(0, it.count) }
        }
    }

    /** The migration drops columns; no JVM test can tell what SQLite the phone ships, so this does. */
    @Test
    fun theDevicesSqliteSupportsDropColumn() {
        helper.createDatabase(name, 1).use { db ->
            val version = db.query("SELECT sqlite_version()").use { cursor ->
                cursor.moveToFirst()
                cursor.getString(0)
            }
            val (major, minor) = version.split('.').take(2).map { it.toInt() }
            assertTrue("DROP COLUMN needs SQLite 3.35, found $version", major > 3 || (major == 3 && minor >= 35))
        }
    }

    @Test
    fun theMigratedDatabaseOpensThroughRoomWithTheProductionMigration() {
        helper.createDatabase(name, 1).use { db ->
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (1, 'Alpha', 'alpha.invalid', 22, 'u1', NULL)")
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = androidx.room.Room.databaseBuilder(context, AppDatabase::class.java, name).addMigrations(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4).build()
        try {
            kotlinx.coroutines.runBlocking {
                val host = database.dao().host(1)!!
                assertEquals(listOf(HostEndpoint("alpha.invalid", 22)), host.addresses)
                assertTrue(host.showInInbox)
                assertEquals(TransportPref.AUTO, host.transport) // v1 -> v2 -> v3 -> v4 in one open.
                assertFalse(host.sleeps)
                assertEquals(0L, host.moshFailedUntil)
            }
        } finally {
            database.close()
            context.deleteDatabase(name)
        }
    }

    /** v2 -> v3: Room validates the result against 3.json and every host keeps its data and gets AUTO. */
    @Test
    fun populatedVersion2MigratesToVersion3WithAutoTransportAndNoLoss() {
        helper.createDatabase(name, 2).use { db ->
            db.execSQL("INSERT INTO keys VALUES ('key-1', 'Phone key', 'ssh-ed25519', 'ssh-ed25519 AAAA', 'SHA256:fp', 'c', x'0a0b0c', x'0102')")
            db.execSQL("INSERT INTO hosts (id, label, username, keyId, showInInbox) VALUES (1, 'Alpha', 'u1', 'key-1', 1)")
            db.execSQL("INSERT INTO hosts (id, label, username, keyId, showInInbox) VALUES (5, 'Beta', 'u2', NULL, 0)")
            db.execSQL("INSERT INTO host_addresses VALUES (1, 0, 'alpha.invalid', 22)")
            db.execSQL("INSERT INTO host_addresses VALUES (1, 1, 'alpha2.invalid', 2222)")
            db.execSQL("INSERT INTO host_addresses VALUES (5, 0, 'beta.invalid', 22)")
            db.execSQL("INSERT INTO trusted_host_keys VALUES (1, 'ssh-ed25519 H1', 'SHA256:h1', 'ssh-ed25519')")
        }
        helper.runMigrationsAndValidate(name, 3, true, MIGRATION_2_3).use { db ->
            db.query("SELECT id, label, username, keyId, showInInbox, transport FROM hosts ORDER BY id").use { cursor ->
                assertEquals(2, cursor.count)
                cursor.moveToFirst()
                assertEquals("Alpha", cursor.getString(1))
                assertEquals("key-1", cursor.getString(3))
                assertEquals(1, cursor.getInt(4))
                assertEquals("AUTO", cursor.getString(5))
                cursor.moveToLast()
                assertTrue(cursor.isNull(3))
                assertEquals(0, cursor.getInt(4)) // The inbox flag of a hidden host survives.
                assertEquals("AUTO", cursor.getString(5))
            }
            db.query("SELECT * FROM host_addresses").use { assertEquals(3, it.count) }
            db.query("SELECT * FROM trusted_host_keys").use { assertEquals(1, it.count) }
            db.query("SELECT ciphertext, iv FROM keys").use { cursor ->
                cursor.moveToFirst()
                assertArrayEquals(byteArrayOf(0x0a, 0x0b, 0x0c), cursor.getBlob(0))
                assertArrayEquals(byteArrayOf(1, 2), cursor.getBlob(1))
            }
        }
    }

    /** v3 -> v4: Room validates the result against 4.json; hosts keep everything and get no sleep and no mosh memory. */
    @Test
    fun populatedVersion3MigratesToVersion4WithTheNewColumnsAtTheirDefaults() {
        helper.createDatabase(name, 3).use { db ->
            db.execSQL("INSERT INTO keys VALUES ('key-1', 'Phone key', 'ssh-ed25519', 'ssh-ed25519 AAAA', 'SHA256:fp', 'c', x'0a0b0c', x'0102')")
            db.execSQL("INSERT INTO hosts (id, label, username, keyId, showInInbox, transport) VALUES (1, 'Alpha', 'u1', 'key-1', 1, 'MOSH')")
            db.execSQL("INSERT INTO hosts (id, label, username, keyId, showInInbox, transport) VALUES (5, 'Beta', 'u2', NULL, 0, 'SSH')")
            db.execSQL("INSERT INTO host_addresses VALUES (1, 0, 'alpha.invalid', 22)")
            db.execSQL("INSERT INTO host_addresses VALUES (5, 0, 'beta.invalid', 22)")
            db.execSQL("INSERT INTO trusted_host_keys VALUES (1, 'ssh-ed25519 H1', 'SHA256:h1', 'ssh-ed25519')")
        }
        helper.runMigrationsAndValidate(name, 4, true, MIGRATION_3_4).use { db ->
            db.query("SELECT id, label, keyId, showInInbox, transport, sleeps, mosh_failed_until FROM hosts ORDER BY id").use { cursor ->
                assertEquals(2, cursor.count)
                cursor.moveToFirst()
                assertEquals("Alpha", cursor.getString(1))
                assertEquals("key-1", cursor.getString(2))
                assertEquals("MOSH", cursor.getString(4)) // The transport choice survives.
                assertEquals(0, cursor.getInt(5))
                assertEquals(0L, cursor.getLong(6))
                cursor.moveToLast()
                assertTrue(cursor.isNull(2))
                assertEquals(0, cursor.getInt(3))
                assertEquals("SSH", cursor.getString(4))
            }
            db.query("SELECT * FROM host_addresses").use { assertEquals(2, it.count) }
            db.query("SELECT * FROM trusted_host_keys").use { assertEquals(1, it.count) }
            db.query("SELECT ciphertext, iv FROM keys").use { cursor ->
                cursor.moveToFirst()
                assertArrayEquals(byteArrayOf(0x0a, 0x0b, 0x0c), cursor.getBlob(0))
                assertArrayEquals(byteArrayOf(1, 2), cursor.getBlob(1))
            }
        }
    }

    /** The full chain on a shipped-M1 database, then the real DAO reads and writes the new columns. */
    @Test
    fun aVersion1DatabaseReachesVersion4AndTheDaoPersistsTheNewColumns() {
        helper.createDatabase(name, 1).use { db ->
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (1, 'Alpha', 'alpha.invalid', 22, 'u1', NULL)")
        }
        helper.runMigrationsAndValidate(name, 4, true, MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4).close()
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = androidx.room.Room.databaseBuilder(context, AppDatabase::class.java, name).addMigrations(MIGRATION_1_2, MIGRATION_2_3, MIGRATION_3_4).build()
        try {
            kotlinx.coroutines.runBlocking {
                val dao = database.dao()
                val migrated = dao.host(1)!!
                assertEquals(TransportPref.AUTO, migrated.transport)
                dao.saveHost(migrated.copy(record = migrated.record.copy(transport = TransportPref.MOSH)), migrated)
                assertEquals(TransportPref.MOSH, dao.host(1)!!.transport)
                dao.saveHost(Host(HostRecord(0, "Beta", "u", null, true, TransportPref.SSH), listOf(HostEndpoint("beta.invalid", 22))), null)
                assertEquals(listOf(TransportPref.MOSH, TransportPref.SSH), dao.hosts().first().sortedBy { it.label }.map { it.transport })

                // `sleeps` is written by an edit; the mosh failure memory by its own query, and an edit that changes
                // the transport or the addresses forgets it, one that changes neither (a label) keeps it.
                dao.saveHost(migrated.copy(record = migrated.record.copy(transport = TransportPref.MOSH, sleeps = true)), dao.host(1)!!)
                assertTrue(dao.host(1)!!.sleeps)
                dao.markMoshFailed(1, 1_790_000_000_000L)
                assertEquals(1_790_000_000_000L, dao.host(1)!!.moshFailedUntil)
                val stored = dao.host(1)!!
                dao.saveHost(stored.copy(record = stored.record.copy(label = "Renamed")), stored)
                assertEquals(1_790_000_000_000L, dao.host(1)!!.moshFailedUntil)
                dao.saveHost(stored.copy(record = stored.record.copy(transport = TransportPref.AUTO)), dao.host(1)!!)
                assertEquals(0L, dao.host(1)!!.moshFailedUntil)
                dao.markMoshFailed(1, 1_790_000_000_000L)
                val again = dao.host(1)!!
                dao.saveHost(again.copy(addresses = listOf(HostEndpoint("elsewhere.invalid", 22))), again)
                assertEquals(0L, dao.host(1)!!.moshFailedUntil)
                dao.markMoshFailed(1, 5L)
                dao.clearMoshFailure(1)
                assertEquals(0L, dao.host(1)!!.moshFailedUntil)
            }
        } finally {
            database.close()
            context.deleteDatabase(name)
        }
    }
}

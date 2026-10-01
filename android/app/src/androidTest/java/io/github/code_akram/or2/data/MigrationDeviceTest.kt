package io.github.code_akram.or2.data

import androidx.room.testing.MigrationTestHelper
import androidx.sqlite.db.framework.FrameworkSQLiteOpenHelperFactory
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
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

    @Test
    fun theMigratedDatabaseOpensThroughRoomWithTheProductionMigration() {
        helper.createDatabase(name, 1).use { db ->
            db.execSQL("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (1, 'Alpha', 'alpha.invalid', 22, 'u1', NULL)")
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = androidx.room.Room.databaseBuilder(context, AppDatabase::class.java, name).addMigrations(MIGRATION_1_2).build()
        try {
            kotlinx.coroutines.runBlocking {
                val host = database.dao().host(1)!!
                assertEquals(listOf(HostEndpoint("alpha.invalid", 22)), host.addresses)
                assertTrue(host.showInInbox)
            }
        } finally {
            database.close()
            context.deleteDatabase(name)
        }
    }
}

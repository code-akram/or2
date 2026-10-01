package io.github.code_akram.or2.data

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.sql.Connection
import java.sql.DriverManager

/**
 * Runs the real v1 -> v2 migration statements on SQLite (JVM `sqlite-jdbc`, test-only) over a
 * database built from the checked-in v1 schema, with foreign keys on as Room has them. The
 * resulting tables must equal a fresh v2 database built from the checked-in v2 schema. The
 * device `MigrationTest` runs the same migration through Room's `MigrationTestHelper`.
 */
class MigrationSqlTest {
    private val schemas = File("schemas/io.github.code_akram.or2.data.AppDatabase")

    /** The CREATE statements of a Room schema JSON, tables and indices, in file order. */
    private fun createStatements(version: Int): List<String> {
        val text = File(schemas, "$version.json").readText()
        val pattern = Regex("\"tableName\": \"(\\w+)\"|\"createSql\": \"([^\"]*)\"")
        var table = ""
        val statements = mutableListOf<String>()
        for (match in pattern.findAll(text)) {
            match.groups[1]?.let { table = it.value }
            match.groups[2]?.let { statements += it.value.replace("\${TABLE_NAME}", table) }
        }
        return statements
    }

    private fun database(version: Int): Connection {
        val connection = DriverManager.getConnection("jdbc:sqlite::memory:")
        connection.createStatement().use { it.execute("PRAGMA foreign_keys = ON") }
        createStatements(version).forEach { sql -> connection.createStatement().use { it.execute(sql) } }
        return connection
    }

    private fun Connection.rows(sql: String): List<List<Any?>> = createStatement().use { statement ->
        statement.executeQuery(sql).use { rs ->
            val columns = rs.metaData.columnCount
            buildList { while (rs.next()) add((1..columns).map { rs.getObject(it) }) }
        }
    }

    private fun Connection.exec(sql: String) = createStatement().use { it.execute(sql) }

    private fun Connection.shape(): Map<String, List<Any?>> {
        val tables = rows("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").map { it[0] as String }
        return tables.associateWith { table ->
            listOf(
                // cid is positional (a dropped column shifts it), so compare name, type, not null, default, primary key.
                rows("PRAGMA table_info(`$table`)").map { listOf(it[1], it[2], it[3], it[4], it[5]) }.sortedBy { it[0] as String },
                rows("PRAGMA foreign_key_list(`$table`)").map { listOf(it[2], it[3], it[4], it[5], it[6]) },
                rows("PRAGMA index_list(`$table`)").map { listOf(it[1], it[2]) }.sortedBy { it[0] as String },
            )
        }
    }

    private fun migrate(connection: Connection) {
        // Room runs a migration inside one transaction.
        connection.autoCommit = false
        try {
            MIGRATION_1_2_STATEMENTS.forEach { connection.exec(it) }
            connection.commit()
        } catch (error: Throwable) {
            connection.rollback()
            throw error
        } finally {
            connection.autoCommit = true
        }
    }

    private fun populatedV1(): Connection {
        val connection = database(1)
        connection.exec("INSERT INTO keys VALUES ('key-1', 'Phone key', 'ssh-ed25519', 'ssh-ed25519 AAAA', 'SHA256:fp', 'c', x'0a0b0c', x'0102')")
        connection.exec("INSERT INTO keys VALUES ('key-2', 'Other key', 'ssh-ed25519', 'ssh-ed25519 BBBB', 'SHA256:fq', '', x'ff', x'03')")
        connection.exec("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (1, 'Alpha', 'alpha.invalid', 22, 'u1', 'key-1')")
        connection.exec("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (5, 'Beta', 'beta.invalid', 2222, 'u2', 'key-2')")
        connection.exec("INSERT INTO hosts (id, label, hostname, port, username, keyId) VALUES (9, 'Gamma', '2001:db8::1', 65535, 'u3', NULL)")
        connection.exec("INSERT INTO trusted_host_keys VALUES (1, 'ssh-ed25519 H1', 'SHA256:h1', 'ssh-ed25519')")
        connection.exec("INSERT INTO trusted_host_keys VALUES (1, 'ssh-rsa H1R', 'SHA256:h1r', 'ssh-rsa')")
        connection.exec("INSERT INTO trusted_host_keys VALUES (5, 'ssh-ed25519 H5', 'SHA256:h5', 'ssh-ed25519')")
        return connection
    }

    @Test
    fun theSchemasAreCheckedInForBothVersions() {
        assertTrue(File(schemas, "1.json").isFile)
        assertTrue(File(schemas, "2.json").isFile)
        assertEquals(3, createStatements(1).count { it.startsWith("CREATE TABLE") })
        assertTrue(createStatements(1).any { "`hostname`" in it })
        assertEquals(4, createStatements(2).count { it.startsWith("CREATE TABLE") }) // hosts, host_addresses, keys, trusted_host_keys
        assertTrue(createStatements(2).any { "`host_addresses`" in it || "`hostId`, `position`" in it })
        assertFalse(createStatements(2).any { it.contains("`hostname`") && it.contains("`username`") })
    }

    @Test
    fun hostnameAndPortMoveToPositionZeroAndEverythingElseSurvives() {
        populatedV1().use { connection ->
            migrate(connection)
            assertEquals(
                listOf(listOf(1, 0, "alpha.invalid", 22), listOf(5, 0, "beta.invalid", 2222), listOf(9, 0, "2001:db8::1", 65535)),
                connection.rows("SELECT hostId, position, hostname, port FROM host_addresses ORDER BY hostId").map { row -> row.map { (it as? Number)?.toInt() ?: it } },
            )
            // The old columns are gone; hosts keep their identity, login and key; the inbox flag defaults on.
            assertEquals(listOf("id", "label", "username", "keyId", "showInInbox"),
                connection.rows("PRAGMA table_info(hosts)").map { it[1] })
            assertEquals(
                listOf(listOf(1, "Alpha", "u1", "key-1", 1), listOf(5, "Beta", "u2", "key-2", 1), listOf(9, "Gamma", "u3", null, 1)),
                connection.rows("SELECT id, label, username, keyId, showInInbox FROM hosts ORDER BY id").map { row -> row.map { (it as? Number)?.toInt() ?: it } },
            )
            // Trust survives (the destination is unchanged), including several keys for one host.
            assertEquals(listOf(listOf(1, "ssh-ed25519 H1"), listOf(1, "ssh-rsa H1R"), listOf(5, "ssh-ed25519 H5")),
                connection.rows("SELECT hostId, openssh FROM trusted_host_keys ORDER BY hostId, openssh").map { row -> row.map { (it as? Number)?.toInt() ?: it } })
            // Key records are untouched, ciphertext and IV byte for byte.
            val key = connection.rows("SELECT id, ciphertext, iv FROM keys ORDER BY id")
            assertEquals(listOf("key-1", "key-2"), key.map { it[0] })
            assertArrayEquals(byteArrayOf(0x0a, 0x0b, 0x0c), key[0][1] as ByteArray)
            assertArrayEquals(byteArrayOf(1, 2), key[0][2] as ByteArray)
            assertEquals(emptyList<List<Any?>>(), connection.rows("PRAGMA foreign_key_check"))
        }
    }

    @Test
    fun theMigratedSchemaEqualsAFreshVersion2Database() {
        populatedV1().use { migrated ->
            migrate(migrated)
            database(2).use { fresh -> assertEquals(fresh.shape(), migrated.shape()) }
        }
    }

    @Test
    fun foreignKeysStillCascadeAndNullAfterMigrating() {
        populatedV1().use { connection ->
            migrate(connection)
            connection.exec("DELETE FROM hosts WHERE id = 1")
            assertEquals(0, connection.rows("SELECT * FROM host_addresses WHERE hostId = 1").size)
            assertEquals(0, connection.rows("SELECT * FROM trusted_host_keys WHERE hostId = 1").size)
            assertEquals(2, connection.rows("SELECT * FROM host_addresses").size)
            connection.exec("DELETE FROM keys WHERE id = 'key-2'")
            assertNull(connection.rows("SELECT keyId FROM hosts WHERE id = 5")[0][0])
            // A second address for a migrated host sits at the next position.
            connection.exec("INSERT INTO host_addresses VALUES (5, 1, 'beta2.invalid', 22)")
            assertThrows(java.sql.SQLException::class.java) { connection.exec("INSERT INTO host_addresses VALUES (5, 1, 'dup.invalid', 22)") }
            assertThrows(java.sql.SQLException::class.java) { connection.exec("INSERT INTO host_addresses VALUES (404, 0, 'orphan.invalid', 22)") }
        }
    }

    @Test
    fun anEmptyV1DatabaseMigratesToo() {
        database(1).use { connection ->
            migrate(connection)
            assertTrue(connection.rows("SELECT * FROM host_addresses").isEmpty())
            connection.exec("INSERT INTO hosts (id, label, username, keyId) VALUES (1, 'New', 'u', NULL)")
            assertEquals(1, (connection.rows("SELECT showInInbox FROM hosts")[0][0] as Number).toInt())
        }
    }
}

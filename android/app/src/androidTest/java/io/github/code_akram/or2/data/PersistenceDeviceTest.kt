package io.github.code_akram.or2.data

import androidx.room.Room
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.code_akram.or2.ffi.PublicKeyInfo
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PersistenceDeviceTest {
    private val first = PublicKeyInfo("algorithm-a", "public-a", "fingerprint-a", "")
    private val second = PublicKeyInfo("algorithm-b", "public-b", "fingerprint-b", "")

    private fun host(addresses: List<HostEndpoint>, label: String = "Fixture") =
        Host(HostRecord(1, label, "fixture", "ephemeral"), addresses)

    @Test
    fun trustReplacementHostEditingAndForeignKeyCleanup() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).build()
        try {
            val dao = database.dao()
            dao.insertKey(KeyRecord("ephemeral", "Fixture", "test", "public-line", "fingerprint", "", byteArrayOf(4, 8), byteArrayOf(7)))
            val host = host(listOf(HostEndpoint("fixture.invalid", 22)))
            dao.saveHost(host, null)
            assertEquals(host, dao.hosts().first().single())
            dao.replaceTrust(host, first)
            dao.insertTrust(TrustedHostKey(host.id, "additional-line", "additional-fingerprint", "test"))
            dao.replaceTrust(host, second)
            assertEquals(listOf(second.openssh), dao.trustedKeys(host.id))
            val renamed = host.copy(record = host.record.copy(label = "Renamed", username = "another-fixture", showInInbox = false))
            dao.saveHost(renamed, host)
            assertEquals(listOf(second.openssh), dao.trustedKeys(host.id))
            val changed = renamed.copy(addresses = listOf(HostEndpoint("fixture.invalid", 2222)))
            dao.saveHost(changed, renamed)
            assertTrue(dao.trustedKeys(host.id).isEmpty())
            try { dao.replaceTrust(host, first); fail("Old destination must not acquire trust") } catch (_: IllegalStateException) { }
            assertTrue(dao.trustedKeys(host.id).isEmpty())
            dao.replaceTrust(changed, second)
            dao.deleteKey("ephemeral")
            assertNull(dao.hosts().first().single().keyId)
            dao.deleteHost(host.id)
            assertTrue(dao.trustedKeys(host.id).isEmpty())
            assertTrue(dao.hostRows().first().isEmpty())
            assertTrue(database.openHelper.readableDatabase.query("SELECT * FROM host_addresses").use { it.count == 0 }) // Cascade.
        } finally { database.close() }
    }

    @Test
    fun addressListOrderPortsAndAnyChangeClearingTrustRoundTripThroughRoom() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).build()
        try {
            val dao = database.dao()
            dao.insertKey(KeyRecord("ephemeral", "Fixture", "test", "public-line", "fingerprint", "", byteArrayOf(4, 8), byteArrayOf(7)))
            val addresses = listOf(HostEndpoint("b.invalid", 2222), HostEndpoint("a.invalid", 22), HostEndpoint("[::1]", 65535))
            val host = host(addresses)
            dao.saveHost(host, null)
            assertEquals(addresses, dao.host(1)!!.addresses) // Preference order is the stored order, not alphabetical.
            val edits = listOf(
                addresses.reversed(), // reorder
                addresses.take(2), // remove
                addresses + HostEndpoint("c.invalid", 22), // add
                listOf(HostEndpoint("b.invalid", 2223)) + addresses.drop(1), // port
                listOf(HostEndpoint("b2.invalid", 2222)) + addresses.drop(1), // hostname
            )
            for (edit in edits) {
                val current = dao.host(1)!!
                dao.replaceTrust(current, first)
                assertEquals(listOf(first.openssh), dao.trustedKeys(1))
                dao.saveHost(current.copy(addresses = edit), current)
                assertTrue("Changing the address list to $edit must clear trust", dao.trustedKeys(1).isEmpty())
                assertEquals(edit, dao.host(1)!!.addresses)
            }
            val settled = dao.host(1)!!
            dao.replaceTrust(settled, second)
            dao.saveHost(settled.copy(record = settled.record.copy(label = "Only a label")), settled)
            assertEquals(listOf(second.openssh), dao.trustedKeys(1))
        } finally { database.close() }
    }
}

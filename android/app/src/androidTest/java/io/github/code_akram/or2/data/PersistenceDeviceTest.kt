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
    @Test
    fun trustReplacementHostEditingAndForeignKeyCleanup() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val database = Room.inMemoryDatabaseBuilder(context, AppDatabase::class.java).build()
        try {
            val dao = database.dao()
            dao.insertKey(KeyRecord("ephemeral", "Fixture", "test", "public-line", "fingerprint", "", byteArrayOf(4, 8), byteArrayOf(7)))
            val host = HostRecord(1, "Fixture", "fixture.invalid", 22, "fixture", "ephemeral")
            dao.insertHost(host)
            val first = PublicKeyInfo("algorithm-a", "public-a", "fingerprint-a", "")
            val second = PublicKeyInfo("algorithm-b", "public-b", "fingerprint-b", "")
            dao.replaceTrust(host, first)
            dao.insertTrust(TrustedHostKey(host.id, "additional-line", "additional-fingerprint", "test"))
            dao.replaceTrust(host, second)
            assertEquals(listOf(second.openssh), dao.trustedKeys(host.id))
            val renamed = host.copy(label = "Renamed", username = "another-fixture")
            dao.saveHost(renamed, host)
            assertEquals(listOf(second.openssh), dao.trustedKeys(host.id))
            val changed = renamed.copy(port = 2222)
            dao.saveHost(changed, renamed)
            assertTrue(dao.trustedKeys(host.id).isEmpty())
            try { dao.replaceTrust(host, first); fail("Old destination must not acquire trust") } catch (_: IllegalStateException) { }
            assertTrue(dao.trustedKeys(host.id).isEmpty())
            dao.replaceTrust(changed, second)
            dao.deleteKey("ephemeral")
            assertNull(dao.hosts().first().single().keyId)
            dao.deleteHost(host.id)
            assertTrue(dao.trustedKeys(host.id).isEmpty())
        } finally { database.close() }
    }
}

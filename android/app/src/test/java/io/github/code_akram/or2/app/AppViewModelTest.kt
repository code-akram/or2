package io.github.code_akram.or2.app

import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TrustedHostKey
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AppViewModelTest {
    private class FakeDao : AppDao() {
        val events = mutableListOf<String>()
        var failDelete = false
        val hostsFlow = MutableStateFlow<List<HostRecord>>(emptyList())
        val keysFlow = MutableStateFlow<List<KeyRecord>>(emptyList())
        override fun hosts() = hostsFlow
        override fun keys() = keysFlow
        override suspend fun host(id: Long) = hostsFlow.value.find { it.id == id }
        override suspend fun key(id: String) = keysFlow.value.find { it.id == id }
        override suspend fun insertKey(key: KeyRecord) = Unit
        override suspend fun deleteKey(id: String) {
            if (failDelete) error("storage failure")
            events += "record:$id"
        }
        override suspend fun insertHost(host: HostRecord) { hostsFlow.value += host }
        override suspend fun updateHost(id: Long, label: String, hostname: String, port: Int, username: String, keyId: String?) = Unit
        override suspend fun deleteHost(id: Long) { hostsFlow.value = hostsFlow.value.filterNot { it.id == id } }
        override suspend fun trustedKeys(hostId: Long) = emptyList<String>()
        override suspend fun clearTrust(hostId: Long) = Unit
        override suspend fun insertTrust(key: TrustedHostKey) = Unit
    }

    @Test
    fun keyDeletionDestroysVaultOnlyAfterDatabaseSucceeds() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) { id -> dao.events += "vault:$id" }
            model.deleteKey("ephemeral")
            assertEquals(listOf("record:ephemeral", "vault:ephemeral"), dao.events)
            dao.events.clear()
            dao.failDelete = true
            model.deleteKey("ephemeral")
            assertTrue(dao.events.isEmpty())
            assertTrue(model.message.value!!.contains("Storage"))
            model.message(null)
            assertNull(model.message.value)
        } finally { Dispatchers.resetMain() }
    }
}

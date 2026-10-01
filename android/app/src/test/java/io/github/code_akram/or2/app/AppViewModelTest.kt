package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.FakeDao
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class AppViewModelTest {
    private fun draft(hostname: String = "fixture.invalid", username: String = "fixture-user", port: Int = 2222, id: Long = 7) =
        Host(HostRecord(id, "Fixture", username, null), listOf(HostEndpoint(hostname, port)))

    @Test
    fun savingAndEditingTrimEveryHostnameAndTheUsernameBeforePersistence() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) {}
            val untrimmed = Host(
                HostRecord(0, "Fixture", " fixture-user \t", null),
                listOf(HostEndpoint(" \tfixture.invalid\n", 2222), HostEndpoint(" second.invalid ", 22)),
            )
            model.saveHost(untrimmed, null)
            val saved = dao.records.value.single()
            assertEquals("fixture-user", saved.username)
            assertEquals(listOf(HostEndpoint("fixture.invalid", 2222), HostEndpoint("second.invalid", 22)),
                dao.addresses.value.sortedBy { it.position }.map { HostEndpoint(it.hostname, it.port) })
            val stored = Host(saved, dao.addresses.value.sortedBy { it.position }.map { HostEndpoint(it.hostname, it.port) })
            model.saveHost(stored.copy(addresses = listOf(HostEndpoint(" fixture-two.invalid ", 22)),
                record = stored.record.copy(username = " other-user ")), stored)
            assertEquals(listOf(HostEndpoint("fixture-two.invalid", 22)), dao.addresses.value.map { HostEndpoint(it.hostname, it.port) })
            assertEquals("other-user", dao.records.value.single().username)
            assertNull(model.message.value)
        } finally { Dispatchers.resetMain() }
    }

    @Test
    fun internalWhitespaceInAnyAddressOrTheUsernameNeverPersistsAndExplainsTheError() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) {}
            for (whitespace in listOf(" ", "\t", "\n", " ", "\u0000")) {
                val invalid = listOf(
                    draft(hostname = "fix${whitespace}ture.invalid"),
                    draft(username = "fix${whitespace}ture"),
                    draft().copy(addresses = listOf(HostEndpoint("ok.invalid", 22), HostEndpoint("fix${whitespace}ture.invalid", 22))),
                )
                for (host in invalid) {
                    model.message(null)
                    model.saveHost(host, null)
                    assertTrue(dao.records.value.isEmpty())
                    assertTrue(model.message.value!!.contains("without internal whitespace or control characters"))
                }
            }
        } finally { Dispatchers.resetMain() }
    }

    @Test
    fun badPortsAndMissingOrTooManyAddressesNeverPersist() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) {}
            for (host in listOf(draft(port = 0), draft(port = 65536), draft().copy(addresses = emptyList()),
                draft().copy(addresses = List(9) { HostEndpoint("h$it", 22) }))) {
                model.message(null)
                model.saveHost(host, null)
                assertTrue(dao.records.value.isEmpty())
                assertNotNull(model.message.value)
            }
        } finally { Dispatchers.resetMain() }
    }

    @Test
    fun theAfterSaveHookRunsOnlyForAValidHostOnceItIsWritten() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) {}
            var calls = 0
            model.saveHost(draft(hostname = "bad host"), null) { calls++ }
            assertEquals(0, calls)
            model.saveHost(draft(id = 0), null) { calls++; assertEquals(1, dao.records.value.size) }
            assertEquals(1, calls)
            assertEquals(1, dao.records.value.size)
        } finally { Dispatchers.resetMain() }
    }

    @Test
    fun aFailedWriteLeavesTheConnectionAloneAndSaysSo() {
        Dispatchers.setMain(UnconfinedTestDispatcher())
        try {
            val dao = FakeDao()
            val model = AppViewModel(dao) {}
            model.saveHost(draft(id = 0), null)
            val stored = Host(dao.records.value.single(), listOf(HostEndpoint("fixture.invalid", 2222)))
            var calls = 0
            dao.failSave = true
            model.saveHost(stored.copy(addresses = listOf(HostEndpoint("other.invalid", 22))), stored) { calls++ }
            assertEquals(0, calls)
            assertTrue(model.message.value!!.contains("Storage"))
            dao.failDelete = true
            model.deleteHost(stored) { calls++ }
            assertEquals(0, calls)
            assertEquals(1, dao.records.value.size)
            dao.failDelete = false
            model.deleteHost(stored) { calls++ }
            assertEquals(1, calls)
            assertTrue(dao.records.value.isEmpty())
        } finally { Dispatchers.resetMain() }
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

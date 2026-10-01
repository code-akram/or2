package io.github.code_akram.or2.data

import io.github.code_akram.or2.connection.FakeDao
import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.ffi.PublicKeyInfo
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

/**
 * The DAO's transactional logic (`saveHost`, `replaceTrust`) over an in-memory fake of its
 * primitives. The SQL itself is covered by [MigrationSqlTest] and the device persistence test.
 */
class HostRecordsTest {
    private val first = PublicKeyInfo("algorithm-a", "public-a", "fingerprint-a", "")
    private val second = PublicKeyInfo("algorithm-b", "public-b", "fingerprint-b", "")
    private val twoAddresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222))

    @Test
    fun savingStoresTheOrderedAddressListAndReadsItBack() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses), null)
        val stored = dao.hosts().first().single()
        assertEquals(twoAddresses, stored.addresses)
        assertEquals(listOf(0, 1), dao.addresses.value.map { it.position })
        assertTrue(stored.showInInbox)
        assertThrows(IllegalArgumentException::class.java) { runBlocking { dao.saveHost(testHost(id = 0, addresses = emptyList()), null) } }
        assertThrows(IllegalArgumentException::class.java) {
            runBlocking { dao.saveHost(testHost(id = 0, addresses = List(9) { HostEndpoint("h$it", 22) }), null) }
        }
    }

    @Test
    fun anyChangeToTheAddressListOrPortsClearsTrustButOtherEditsKeepIt() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses), null)
        var host = dao.host(1)!!
        dao.replaceTrust(host, first)
        assertEquals(listOf(first.openssh), dao.trustedKeys(1))

        // Label, username, key and inbox flag do not touch trust.
        val renamed = host.copy(record = host.record.copy(label = "Renamed", username = "other", keyId = "k2", showInInbox = false))
        dao.saveHost(renamed, host)
        assertEquals(listOf(first.openssh), dao.trustedKeys(1))
        host = dao.host(1)!!
        assertEquals("Renamed", host.label)
        assertFalse(host.showInInbox)
        assertEquals(twoAddresses, host.addresses)

        val edits = listOf(
            "port" to listOf(HostEndpoint("a.invalid", 2022), HostEndpoint("b.invalid", 2222)),
            "hostname" to listOf(HostEndpoint("a.invalid", 22), HostEndpoint("c.invalid", 2222)),
            "added" to twoAddresses + HostEndpoint("c.invalid", 22),
            "removed" to twoAddresses.take(1),
            "reordered" to twoAddresses.reversed(),
        )
        for ((name, addresses) in edits) {
            dao.clearTrust(1)
            dao.replaceTrust(host, first)
            val edited = host.copy(addresses = addresses)
            dao.saveHost(edited, host)
            assertTrue("$name edit must clear trust", dao.trustedKeys(1).isEmpty())
            assertEquals(addresses, dao.host(1)!!.addresses)
            dao.saveHost(host, edited) // Restore the original list for the next case.
            host = dao.host(1)!!
            assertEquals(twoAddresses, host.addresses)
        }
    }

    @Test
    fun trustCannotBeStoredForAStaleDestinationOrADeletedHost() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses), null)
        val stale = dao.host(1)!!
        dao.replaceTrust(stale, first)
        dao.replaceTrust(stale, second) // Replaces, never accumulates.
        assertEquals(listOf(second.openssh), dao.trustedKeys(1))
        dao.saveHost(stale.copy(addresses = twoAddresses.take(1)), stale)
        assertTrue(dao.trustedKeys(1).isEmpty())
        assertThrows(IllegalStateException::class.java) { runBlocking { dao.replaceTrust(stale, first) } }
        assertTrue(dao.trustedKeys(1).isEmpty())
        dao.deleteHost(1)
        assertNull(dao.host(1))
        assertThrows(IllegalStateException::class.java) { runBlocking { dao.replaceTrust(stale, first) } }
        assertThrows(IllegalStateException::class.java) { runBlocking { dao.saveHost(stale, stale) } }
    }

    // --- the follow-up: `sleeps` and the mosh failure memory ------------------------------------

    private val until = 1_800_086_400_000L

    @Test
    fun theSleepsFlagIsSavedWithTheHostAndTheFailureMemoryByItsOwnWrite() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses, sleeps = true), null)
        var host = dao.host(1)!!
        assertTrue(host.sleeps)
        assertEquals(0L, host.moshFailedUntil)
        dao.markMoshFailed(1, until)
        host = dao.host(1)!!
        assertEquals(until, host.moshFailedUntil)

        // An edit that changes neither the transport nor the addresses keeps the memory, and `sleeps` is an ordinary field.
        dao.saveHost(host.copy(record = host.record.copy(label = "Renamed", sleeps = false)), host)
        val renamed = dao.host(1)!!
        assertEquals(until, renamed.moshFailedUntil)
        assertFalse(renamed.sleeps)
        assertEquals("Renamed", renamed.label)
    }

    @Test
    fun aNewTransportPreferenceOrADifferentDestinationForgetsTheMoshFailure() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses), null)
        var host = dao.host(1)!!

        dao.markMoshFailed(1, until)
        host = dao.host(1)!!
        dao.saveHost(host.copy(record = host.record.copy(transport = TransportPref.SSH)), host)
        assertEquals(0L, dao.host(1)!!.moshFailedUntil) // A new preference is a fresh decision.

        host = dao.host(1)!!
        dao.markMoshFailed(1, until)
        host = dao.host(1)!!
        dao.saveHost(host.copy(addresses = twoAddresses.reversed()), host)
        assertEquals(0L, dao.host(1)!!.moshFailedUntil) // Another address may have UDP open.

        dao.markMoshFailed(1, until)
        dao.clearMoshFailure(1)
        assertEquals(0L, dao.host(1)!!.moshFailedUntil)
    }
}

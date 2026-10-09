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

    // --- Easy pair: a host and its trusted key in one step ---------------------------------------

    @Test
    fun aPairedHostIsStoredWithItsAddressesAndTrustedKeyAndIsNeverGivenAnIdByTheCaller() = runBlocking<Unit> {
        val dao = FakeDao()
        val id = dao.saveHostWithTrust(testHost(id = 99, addresses = twoAddresses), first)
        assertEquals(1L, id) // The caller's id is ignored: a new row.
        val stored = dao.host(id)!!
        assertEquals(twoAddresses, stored.addresses)
        assertEquals(listOf(first.openssh), dao.trustedKeys(id))
        assertEquals("algorithm-a", dao.trust.single().algorithm)
        // The first connection's trust check finds the key: nothing to prompt about.
        assertEquals(1, dao.hosts().first().size)
    }

    @Test
    fun aPairedHostStillNeedsAValidAddressList() = runBlocking<Unit> {
        val dao = FakeDao()
        assertThrows(IllegalArgumentException::class.java) { runBlocking { dao.saveHostWithTrust(testHost(id = 0, addresses = emptyList()), first) } }
        assertTrue(dao.hosts().first().isEmpty())
        assertTrue(dao.trust.isEmpty())
    }

    // --- the follow-up: `sleeps` ------------------------------------------------------------------

    @Test
    fun theSleepsFlagIsSavedWithTheHostAndEditedLikeAnyField() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses, sleeps = true), null)
        val host = dao.host(1)!!
        assertTrue(host.sleeps)

        dao.saveHost(host.copy(record = host.record.copy(label = "Renamed", sleeps = false)), host)
        val renamed = dao.host(1)!!
        assertFalse(renamed.sleeps)
        assertEquals("Renamed", renamed.label)
    }

    // --- M4: waking a sleeping host ---------------------------------------------------------------

    @Test
    fun theMacAddressAndWakeProbeAreSavedAndEditedWithoutTouchingTrust() = runBlocking<Unit> {
        val dao = FakeDao()
        dao.saveHost(testHost(id = 0, addresses = twoAddresses), null)
        var host = dao.host(1)!!
        assertNull(host.macAddress)
        assertFalse(host.wakeProbe)
        dao.replaceTrust(host, first)

        dao.saveHost(host.copy(record = host.record.copy(macAddress = "aa:bb:cc:dd:ee:ff", wakeProbe = true)), host)
        host = dao.host(1)!!
        assertEquals("aa:bb:cc:dd:ee:ff", host.macAddress)
        assertTrue(host.wakeProbe)
        // Neither names a destination: the host key stays trusted.
        assertEquals(listOf(first.openssh), dao.trustedKeys(1))

        dao.saveHost(host.copy(record = host.record.copy(macAddress = null, wakeProbe = false)), host)
        host = dao.host(1)!!
        assertNull(host.macAddress)
        assertFalse(host.wakeProbe)
        assertEquals(listOf(first.openssh), dao.trustedKeys(1))
    }
}

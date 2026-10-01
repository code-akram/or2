package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.data.HostEndpoint
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The record of mosh servers this app started: per host and destination, durable, and tolerant of what it did not write. */
class MoshServerLedgerTest {
    private val one = testHost(id = 1)
    private val two = testHost(id = 2)

    @Test
    fun recordsAreKeptPerHostAndSurviveANewInstance() {
        val store = MemoryPrefStore()
        val ledger = MoshServerLedger(store)
        ledger.record(one, 100u)
        ledger.record(one, 101u)
        ledger.record(two, 100u)
        ledger.record(one, 100u) // Twice is once.
        assertEquals(listOf(100u, 101u), ledger.pids(one))
        assertEquals(listOf(100u), ledger.pids(two))
        assertEquals(emptyList<UInt>(), ledger.pids(testHost(id = 3)))

        val again = MoshServerLedger(store)
        assertEquals(listOf(100u, 101u), again.pids(one))
        assertEquals(listOf(100u), again.pids(two))
    }

    @Test
    fun clearingOnePidLeavesTheOthersAndTheLastClearRemovesTheEntry() {
        val store = MemoryPrefStore()
        val ledger = MoshServerLedger(store)
        ledger.record(one, 100u)
        ledger.record(two, 100u)
        ledger.clear(one, 100u)
        ledger.clear(one, 555u) // Not recorded: nothing happens.
        assertEquals(emptyList<UInt>(), ledger.pids(one))
        assertEquals(listOf(100u), MoshServerLedger(store).pids(two))
        ledger.clear(two, 100u)
        assertNull(store.getString("mosh_servers"))
    }

    @Test
    fun purgingAHostForgetsOnlyItsServers() {
        val ledger = MoshServerLedger(MemoryPrefStore())
        ledger.record(one, 100u)
        ledger.record(one, 101u)
        ledger.record(two, 200u)
        ledger.purge(1)
        assertEquals(emptyList<UInt>(), ledger.pids(one))
        assertEquals(listOf(200u), ledger.pids(two))
    }

    @Test
    fun aRecordIsOnlyReturnedForTheDestinationAndLoginThatCreatedIt() {
        val store = MemoryPrefStore()
        val ledger = MoshServerLedger(store)
        ledger.record(one, 100u)
        // The same stored host (same id) after an edit of its address list, a port, or its login.
        val moved = one.copy(addresses = listOf(HostEndpoint("elsewhere.invalid", 2222)))
        val otherPort = one.copy(addresses = listOf(HostEndpoint(one.addresses.single().hostname, 22)))
        val otherLogin = one.copy(record = one.record.copy(username = "another"))
        for (edited in listOf(moved, otherPort, otherLogin)) {
            assertEquals(emptyList<UInt>(), ledger.pids(edited))
            assertEquals(emptyList<UInt>(), MoshServerLedger(store).pids(edited))
            ledger.clear(edited, 100u) // Cannot clear what another destination recorded either.
        }
        // A label or key edit is not a move.
        val renamed = one.copy(record = one.record.copy(label = "Renamed", keyId = "other-key", showInInbox = false))
        assertEquals(listOf(100u), ledger.pids(renamed))
        assertEquals(listOf(100u), ledger.allPids(1))
    }

    @Test
    fun entriesThisAppDidNotWriteAreIgnored() {
        val store = MemoryPrefStore()
        val identity = one.moshIdentity()
        store.putString(
            "mosh_servers",
            // Valid ones; entries without an identity (written before identities existed), a zero pid, a
            // non-hex identity, extra fields, a negative id and an empty entry are not trusted.
            "1:100:$identity,garbage,2:x:$identity,3:0:$identity,:5:$identity,4:6:7:8,1:9,2:10:ZZ,5:4294967295:$identity,-1:9:$identity,1:11:,",
        )
        val ledger = MoshServerLedger(store)
        assertEquals(listOf(100u), ledger.pids(one))
        assertEquals(listOf(4294967295u), ledger.pids(testHost(id = 5)))
        assertEquals(listOf(9u), ledger.pids(testHost(id = -1)))
        assertEquals(emptyList<UInt>(), ledger.pids(two))
        assertEquals(emptyList<UInt>(), ledger.pids(testHost(id = 3)))
        assertEquals(emptyList<UInt>(), ledger.pids(testHost(id = 4)))
        assertEquals(listOf(100u), ledger.allPids(1))
    }
}

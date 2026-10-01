package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The record of mosh servers this app started: per host, durable, and tolerant of what it did not write. */
class MoshServerLedgerTest {
    @Test
    fun recordsAreKeptPerHostAndSurviveANewInstance() {
        val store = MemoryPrefStore()
        val ledger = MoshServerLedger(store)
        ledger.record(1, 100u)
        ledger.record(1, 101u)
        ledger.record(2, 100u)
        ledger.record(1, 100u) // Twice is once.
        assertEquals(listOf(100u, 101u), ledger.pids(1))
        assertEquals(listOf(100u), ledger.pids(2))
        assertEquals(emptyList<UInt>(), ledger.pids(3))

        val again = MoshServerLedger(store)
        assertEquals(listOf(100u, 101u), again.pids(1))
        assertEquals(listOf(100u), again.pids(2))
    }

    @Test
    fun clearingOnePidLeavesTheOthersAndTheLastClearRemovesTheEntry() {
        val store = MemoryPrefStore()
        val ledger = MoshServerLedger(store)
        ledger.record(1, 100u)
        ledger.record(2, 100u)
        ledger.clear(1, 100u)
        ledger.clear(1, 555u) // Not recorded: nothing happens.
        assertEquals(emptyList<UInt>(), ledger.pids(1))
        assertEquals(listOf(100u), MoshServerLedger(store).pids(2))
        ledger.clear(2, 100u)
        assertNull(store.getString("mosh_servers"))
    }

    @Test
    fun purgingAHostForgetsOnlyItsServers() {
        val ledger = MoshServerLedger(MemoryPrefStore())
        ledger.record(1, 100u)
        ledger.record(1, 101u)
        ledger.record(2, 200u)
        ledger.purge(1)
        assertEquals(emptyList<UInt>(), ledger.pids(1))
        assertEquals(listOf(200u), ledger.pids(2))
    }

    @Test
    fun entriesThisAppDidNotWriteAreIgnored() {
        val store = MemoryPrefStore()
        store.putString("mosh_servers", "1:100,garbage,2:x,3:0,:5,4:6:7,-1:9,5:4294967295,")
        val ledger = MoshServerLedger(store)
        assertEquals(listOf(100u), ledger.pids(1))
        assertEquals(listOf(4294967295u), ledger.pids(5))
        assertEquals(emptyList<UInt>(), ledger.pids(2))
        assertEquals(emptyList<UInt>(), ledger.pids(3))
        assertEquals(emptyList<UInt>(), ledger.pids(4))
        assertEquals(listOf(9u), ledger.pids(-1))
    }
}

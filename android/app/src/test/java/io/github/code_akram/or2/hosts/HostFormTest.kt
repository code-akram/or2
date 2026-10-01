package io.github.code_akram.or2.hosts

import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import org.junit.Assert.*
import org.junit.Test

class HostFormTest {
    private val ok = AddressDraft("fixture.invalid", "22")

    @Test
    fun validationChecksEveryAddressBothPortBoundariesAndTheCount() {
        assertTrue(validHost("Label", listOf(ok), "fixture"))
        assertTrue(validHost("Label", listOf(AddressDraft("a", "1"), AddressDraft("b", "65535")), "fixture"))
        assertTrue(validHost("Label", listOf(AddressDraft(" fixture.invalid ", "22")), " fixture "))
        for (port in listOf("0", "65536", "", "-1", "22x")) assertFalse(validHost("Label", listOf(AddressDraft("h", port)), "fixture"))
        assertFalse(validHost("Label", listOf(ok, AddressDraft("bad address", "22")), "fixture"))
        assertFalse(validHost("Label", listOf(ok, AddressDraft("h", "0")), "fixture"))
        assertFalse(validHost("Label", emptyList(), "fixture"))
        assertTrue(validHost("Label", List(8) { ok }, "fixture"))
        assertFalse(validHost("Label", List(9) { ok }, "fixture"))
        assertFalse(validHost(" ", listOf(ok), "fixture"))
        assertFalse(validHost("Label", listOf(ok), "bad name"))
        assertFalse(validHost("Label", listOf(ok), "bad\nname"))
        assertEquals("Remove internal whitespace or control characters.", hostFieldError("bad name"))
        assertEquals("Enter a value.", hostFieldError(" \t"))
        assertNull(portError("22"))
        assertEquals("Port must be 1-65535.", portError("0"))
    }

    @Test
    fun reorderingMovesOneEntryAndIgnoresOutOfRangeMoves() {
        val list = listOf("a", "b", "c")
        assertEquals(listOf("b", "a", "c"), list.moved(1, -1))
        assertEquals(listOf("a", "c", "b"), list.moved(1, 1))
        assertEquals(list, list.moved(0, -1))
        assertEquals(list, list.moved(2, 1))
        assertEquals(list, list.moved(5, 1))
        assertEquals(AddressDraft("h", "22"), AddressDraft.of(HostEndpoint("h", 22)))
    }

    @Test
    fun onlyDestinationLoginOrKeyChangesAffectALiveConnection() {
        val previous = testHost(addresses = listOf(HostEndpoint("a", 22), HostEndpoint("b", 22)))
        assertFalse(connectionAffectedBy(previous, previous.copy(record = previous.record.copy(label = "Renamed"))))
        assertFalse(connectionAffectedBy(previous, previous.copy(record = previous.record.copy(showInInbox = false))))
        assertTrue(connectionAffectedBy(previous, previous.copy(addresses = listOf(HostEndpoint("a", 22)))))
        assertTrue(connectionAffectedBy(previous, previous.copy(addresses = listOf(HostEndpoint("b", 22), HostEndpoint("a", 22)))))
        assertTrue(connectionAffectedBy(previous, previous.copy(addresses = listOf(HostEndpoint("a", 2222), HostEndpoint("b", 22)))))
        assertTrue(connectionAffectedBy(previous, previous.copy(record = previous.record.copy(username = "other"))))
        assertTrue(connectionAffectedBy(previous, previous.copy(record = HostRecord(previous.id, "Fixture", "fixture", "another-key"))))
    }
}

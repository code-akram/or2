package io.github.code_akram.or2.hosts

import io.github.code_akram.or2.connection.testHost
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TransportPref
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

    @Test
    fun theTransportControlOffersAutoSshMoshInThatOrderWithAnExplanationForEach() {
        assertEquals(listOf("Auto", "SSH", "Mosh"), TransportChoices.map(::transportLabel))
        assertEquals(TransportPref.entries, TransportChoices) // Nothing the form cannot express.
        val notes = TransportChoices.map(::transportExplanation)
        assertEquals(3, notes.toSet().size)
        assertTrue(notes[0].contains("SSH if mosh cannot connect"))
    }

    @Test
    fun aTransportChangeDoesNotEndALiveConnection() {
        val previous = testHost()
        assertFalse(connectionAffectedBy(previous, previous.copy(record = previous.record.copy(transport = TransportPref.MOSH))))
    }

    @Test
    fun theAddressHintTellsTheUserToListTheAddressThatWorksEverywhereFirst() {
        assertTrue(ADDRESS_ORDER_HINT.contains("list the one that works on every network first"))
        assertTrue(ADDRESS_ORDER_HINT.contains("Mosh stays on the address SSH reached"))
        assertTrue(ADDRESS_ORDER_HINT.startsWith("In order of preference."))
    }

    @Test
    fun theSleepsFlagIsExplainedAndDoesNotEndALiveConnection() {
        assertTrue(SLEEPS_EXPLANATION.contains("asleep"))
        assertTrue(SLEEPS_EXPLANATION.contains("no reconnect is offered"))
        val previous = testHost()
        assertFalse(connectionAffectedBy(previous, previous.copy(record = previous.record.copy(sleeps = true))))
    }

    // --- M4: the MAC address for Wake-on-LAN ------------------------------------------------------

    @Test
    fun macAddressesAcceptColonsOrDashesInAnyCaseAndAreStoredLowercaseWithColons() {
        for (text in listOf("aa:bb:cc:dd:ee:ff", "AA:BB:CC:DD:EE:FF", "aa-bb-cc-dd-ee-ff", "Aa-Bb-cC-dD-eE-Ff", " aa:bb:cc:dd:ee:ff ")) {
            assertEquals(text, "aa:bb:cc:dd:ee:ff", normalizedMac(text))
            assertNull(text, macError(text))
        }
        assertEquals("01:23:45:67:89:ab", normalizedMac("01-23-45-67-89-AB"))
    }

    @Test
    fun anEmptyMacAddressIsNoneAndAnythingElseIsAnInlineError() {
        assertNull(macError(""))
        assertNull(macError("  "))
        assertNull(normalizedMac(""))
        for (text in listOf(
            "aa:bb:cc:dd:ee", "aa:bb:cc:dd:ee:ff:00", "aa:bb-cc:dd:ee:ff", "aabbccddeeff", "aa:bb:cc:dd:ee:fg",
            "a:bb:cc:dd:ee:fff", "aa.bb.cc.dd.ee.ff", "aa:bb:cc:dd:ee:f", "aa bb cc dd ee ff",
        )) {
            assertNull(text, normalizedMac(text))
            assertEquals(text, "Use six hex pairs: aa:bb:cc:dd:ee:ff.", macError(text))
        }
    }

    @Test
    fun wakeSettingsAreNoConnectionChange() {
        val previous = testHost()
        val woken = previous.copy(record = previous.record.copy(macAddress = "aa:bb:cc:dd:ee:ff", wakeProbe = true))
        assertFalse(connectionAffectedBy(previous, woken))
    }
}

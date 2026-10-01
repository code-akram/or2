package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.HostEndpoint
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class UnlockPlanTest {
    private fun host(id: Long, key: String?) = testHost(id, "Host $id", key, listOf(HostEndpoint("h$id.invalid", 22)))

    @Test
    fun hostsGroupByKeyRecordInRequestOrderAndEachHostAppearsOnce() {
        val plan = planUnlock(listOf(host(1, "a"), host(2, "b"), host(3, "a"), host(1, "a"), host(4, null), host(5, "b")))
        assertEquals(listOf("a", "b"), plan.groups.map { it.keyId })
        assertEquals(listOf(listOf(1L, 3L), listOf(2L, 5L)), plan.groups.map { group -> group.hosts.map { it.id } })
        assertEquals(listOf(4L), plan.withoutKey.map { it.id })
        assertEquals(UnlockPlan(emptyList(), emptyList()), planUnlock(emptyList()))
    }

    /** Records each unlock and what the block saw; wipes like the real unlocker. */
    private class FakeUnlocker(val failFor: Set<String> = emptySet()) : KeyUnlocker {
        val prompts = mutableListOf<String>()
        var lastArray: ByteArray? = null
        override suspend fun <T> withKey(keyId: String, block: suspend (ByteArray) -> T): T {
            prompts += keyId
            if (keyId in failFor) throw IllegalStateException("cancelled")
            val bytes = byteArrayOf(1, 2, 3)
            lastArray = bytes
            try {
                return block(bytes)
            } finally {
                bytes.fill(0)
            }
        }
    }

    @Test
    fun oneUnlockPerDistinctKeyConnectsEveryHostSharingIt() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val seen = mutableListOf<Pair<String, List<Byte>>>()
        val holder = HostConnections({ request, _ ->
            seen += request.addresses[0].host to request.privateKey.toList()
            FakePort()
        }, FakeTrust(), dispatcher, dispatcher)
        val unlocker = FakeUnlocker()
        connectGrouped(listOf(host(1, "a"), host(2, "b"), host(3, "a")), holder, unlocker)
        assertEquals(listOf("a", "b"), unlocker.prompts) // Three hosts, two prompts.
        assertEquals(listOf("h1.invalid", "h3.invalid", "h2.invalid"), seen.map { it.first })
        assertTrue(seen.all { it.second == listOf<Byte>(1, 2, 3) }) // Every connect saw the key.
        assertArrayEquals(ByteArray(3), unlocker.lastArray) // Wiped once the group was done.
        assertEquals(setOf(1L, 2L, 3L), holder.hosts.value.keys)
        holder.hosts.value.keys.forEach(holder::dismissHost)
    }

    @Test
    fun liveHostsAreSkippedSoNoPromptIsWasted() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val holder = HostConnections({ _, _ -> FakePort() }, FakeTrust(), dispatcher, dispatcher)
        val unlocker = FakeUnlocker()
        connectGrouped(listOf(host(1, "a")), holder, unlocker)
        connectGrouped(listOf(host(1, "a"), host(2, "b")), holder, unlocker)
        assertEquals(listOf("a", "b"), unlocker.prompts)
        connectGrouped(listOf(host(1, "a"), host(2, "b")), holder, unlocker)
        assertEquals(listOf("a", "b"), unlocker.prompts) // Nothing left to connect.
        holder.hosts.value.keys.forEach(holder::dismissHost)
    }

    @Test
    fun aCancelledUnlockEndsTheBatchButAConnectErrorDoesNot() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val holder = HostConnections({ request, _ ->
            if (request.addresses[0].host == "h1.invalid") throw io.github.code_akram.or2.ffi.HostConnectException.InvalidPrivateKey()
            FakePort()
        }, FakeTrust(), dispatcher, dispatcher)
        val unlocker = FakeUnlocker()
        // A connect error in the first group still lets the second group connect.
        val failure = runCatching { connectGrouped(listOf(host(1, "a"), host(2, "b")), holder, unlocker) }.exceptionOrNull()
        assertTrue(failure is io.github.code_akram.or2.ffi.HostConnectException.InvalidPrivateKey)
        assertEquals(listOf("a", "b"), unlocker.prompts)
        assertEquals(setOf(2L), holder.hosts.value.keys)
        holder.dismissHost(2)

        // A cancelled prompt stops before the next key is asked for.
        val cancelling = FakeUnlocker(failFor = setOf("a"))
        val stopped = runCatching { connectGrouped(listOf(host(3, "a"), host(4, "b")), holder, cancelling) }.exceptionOrNull()
        assertTrue(stopped is IllegalStateException)
        assertEquals(listOf("a"), cancelling.prompts)
        assertTrue(holder.hosts.value.isEmpty())
    }

    @Test
    fun hostsWithoutAKeyAreReportedAfterTheOthersConnect() = runTest {
        val dispatcher = UnconfinedTestDispatcher(testScheduler)
        val holder = HostConnections({ _, _ -> FakePort() }, FakeTrust(), dispatcher, dispatcher)
        val unlocker = FakeUnlocker()
        val failure = runCatching { connectGrouped(listOf(host(1, null), host(2, "a")), holder, unlocker) }.exceptionOrNull()
        assertTrue(failure is MissingKeyException)
        assertEquals(listOf(1L), (failure as MissingKeyException).hosts.map { it.id })
        assertEquals(setOf(2L), holder.hosts.value.keys)
        holder.dismissHost(2)
    }
}

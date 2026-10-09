package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.inbox.LinkStatus
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.delay
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test
import java.net.InetAddress

/** Home's Wake on fakes (the packet, the probe, the unlock and the connect attempts), and what feeds it. */
@OptIn(ExperimentalCoroutinesApi::class)
class WakeTest {
    private val mac = "aa:bb:cc:dd:ee:ff"
    private val addresses = listOf(HostEndpoint("mac.local", 22), HostEndpoint("192.0.2.20", 22))

    /** A [Waker] whose packet and probe are logged in [events], on the test clock. */
    private class Rig(scope: TestScope, val events: MutableList<String> = mutableListOf(), failPacket: Boolean = false) {
        val waker = Waker(
            sendPacket = { mac, broadcasts ->
                events += "packet:$mac:${broadcasts.joinToString(",")}@${scope.testScheduler.currentTime}"
                if (failPacket) error("no network")
            },
            probe = { addresses -> events += "probe:${addresses.size}@${scope.testScheduler.currentTime}" },
            broadcasts = { listOf("192.168.1.255") },
            monotonicMs = { scope.testScheduler.currentTime },
        )

        /** A connection whose unlock is logged and whose attempts take [attemptMs] each and answer [outcomes] in turn. */
        fun connection(outcomes: List<WakeAttempt>, attemptMs: Long = 6_000, statusSeen: MutableList<WakeStatus?>? = null, hostId: Long = 7) =
            WakeConnection { attempts ->
                events += "unlock"
                val left = outcomes.toMutableList()
                attempts {
                    statusSeen?.add(waker.status.value[hostId])
                    delay(attemptMs)
                    val outcome = left.removeAt(0)
                    events += "connect:$outcome"
                    outcome
                }
                events += "wipe"
            }
    }

    @Test
    fun wakeSendsThePacketAndProbesBeforeTheUnlockThenConnects() = runTest {
        val rig = Rig(this)
        val host = testHost(macAddress = mac, wakeProbe = true, sleeps = true, addresses = addresses)
        val seen = mutableListOf<WakeStatus?>()
        rig.waker.wake(host, rig.connection(listOf(WakeAttempt.ANSWERED), statusSeen = seen))
        assertEquals(
            listOf("packet:$mac:192.168.1.255@0", "probe:2@0", "unlock", "connect:ANSWERED", "wipe"),
            rig.events,
        )
        assertEquals(listOf(WakeStatus.WAKING), seen) // "Waking…" on the card while it connects.
        assertTrue(rig.waker.status.value.isEmpty())
    }

    @Test
    fun aHostWithoutAMacGetsNoPacketAndOneWithoutTheProbeNoKnock() = runTest {
        val probeOnly = Rig(this)
        probeOnly.waker.wake(testHost(wakeProbe = true), probeOnly.connection(listOf(WakeAttempt.ANSWERED)))
        assertEquals(listOf("probe:1@0", "unlock", "connect:ANSWERED", "wipe"), probeOnly.events)

        val macOnly = Rig(this)
        val start = currentTime
        macOnly.waker.wake(testHost(macAddress = mac), macOnly.connection(listOf(WakeAttempt.ANSWERED)))
        assertEquals(listOf("packet:$mac:192.168.1.255@$start", "unlock", "connect:ANSWERED", "wipe"), macOnly.events)
    }

    @Test
    fun nobodyAnsweringIsRetriedWithTheOneUnlockAndThePacketResent() = runTest {
        val rig = Rig(this)
        val host = testHost(macAddress = mac, wakeProbe = true, sleeps = true)
        rig.waker.wake(host, rig.connection(listOf(WakeAttempt.NO_ANSWER, WakeAttempt.NO_ANSWER, WakeAttempt.ANSWERED)))
        assertEquals(
            listOf(
                "packet:$mac:192.168.1.255@0", "probe:1@0", "unlock",
                // 6 s per attempt, 2 s apart; the probe is not repeated (every connect of the host runs it itself).
                "connect:NO_ANSWER", "packet:$mac:192.168.1.255@8000",
                "connect:NO_ANSWER", "packet:$mac:192.168.1.255@16000",
                "connect:ANSWERED", "wipe",
            ),
            rig.events,
        )
        assertTrue(rig.waker.status.value.isEmpty())
    }

    @Test
    fun aSleepingHostThatNeverAnswersGivesUpAfter30sAndSaysWhy() = runTest {
        val rig = Rig(this)
        val host = testHost(macAddress = mac, sleeps = true)
        rig.waker.wake(host, rig.connection(List(10) { WakeAttempt.NO_ANSWER }))
        // Attempts at 0, 8, 16 and 24 s; the one ending at 30 s is the last: no attempt starts after 30 s.
        assertEquals(4, rig.events.count { it == "connect:NO_ANSWER" })
        assertEquals(30_000, currentTime)
        assertEquals("wipe", rig.events.last())
        assertEquals(mapOf(host.id to WakeStatus.CANT_WAKE), rig.waker.status.value)
        assertEquals("Can't wake: it may be asleep with the lid closed or on battery", CANT_WAKE_MESSAGE)

        // A new connect of the host forgets it.
        rig.waker.clear(host.id)
        assertTrue(rig.waker.status.value.isEmpty())
    }

    @Test
    fun aHostThatDoesNotSleepStopsAfter30sWithItsOwnFailure() = runTest {
        val rig = Rig(this)
        rig.waker.wake(testHost(macAddress = mac, sleeps = false), rig.connection(List(10) { WakeAttempt.NO_ANSWER }))
        assertEquals(4, rig.events.count { it == "connect:NO_ANSWER" })
        assertTrue(rig.waker.status.value.isEmpty()) // The card shows the connection's own failure.
    }

    @Test
    fun aRefusalEndsTheWakeAtOnce() = runTest {
        val rig = Rig(this)
        rig.waker.wake(testHost(macAddress = mac, sleeps = true), rig.connection(listOf(WakeAttempt.FAILED, WakeAttempt.ANSWERED)))
        assertEquals(1, rig.events.count { it.startsWith("connect:") })
        assertTrue(rig.waker.status.value.isEmpty())
    }

    @Test
    fun aPacketThatCannotBeSentStillConnects() = runTest {
        val rig = Rig(this, failPacket = true)
        rig.waker.wake(testHost(macAddress = mac), rig.connection(listOf(WakeAttempt.ANSWERED)))
        assertEquals(listOf("packet:$mac:192.168.1.255@0", "unlock", "connect:ANSWERED", "wipe"), rig.events)
    }

    @Test
    fun aCancelledUnlockEndsTheWakeAndIsReported() = runTest {
        val rig = Rig(this)
        val host = testHost(macAddress = mac, sleeps = true)
        val failure = runCatching { rig.waker.wake(host) { _ -> error("biometric cancelled") } }.exceptionOrNull()
        assertEquals("biometric cancelled", failure?.message)
        assertTrue(rig.waker.status.value.isEmpty())
    }

    @Test
    fun clearLeavesAWakeInProgressAlone() = runTest {
        val rig = Rig(this)
        val host = testHost(macAddress = mac)
        val seen = mutableListOf<WakeStatus?>()
        rig.waker.wake(host, WakeConnection { attempts ->
            attempts {
                rig.waker.clear(host.id)
                seen += rig.waker.status.value[host.id]
                WakeAttempt.ANSWERED
            }
        })
        assertEquals(listOf(WakeStatus.WAKING), seen)
    }

    // --- what one attempt came to, and when Wake is offered -----------------------------------------

    @Test
    fun aSettledConnectionReadsAsAnAttemptsOutcome() {
        fun failed(failure: SessionFailure) = HostState.Closed(CloseReason.Failed(failure))
        assertEquals(WakeAttempt.ANSWERED, wakeAttemptOf(HostState.Connected(0u)))
        assertEquals(WakeAttempt.ANSWERED, wakeAttemptOf(testPrompt))
        assertEquals(WakeAttempt.NO_ANSWER, wakeAttemptOf(failed(SessionFailure.Unreachable("no route"))))
        assertEquals(WakeAttempt.NO_ANSWER, wakeAttemptOf(failed(SessionFailure.TimedOut)))
        assertEquals(WakeAttempt.NO_ANSWER, wakeAttemptOf(failed(SessionFailure.ConnectionLost("reset"))))
        assertEquals(WakeAttempt.FAILED, wakeAttemptOf(failed(SessionFailure.AuthenticationRejected)))
        assertEquals(WakeAttempt.FAILED, wakeAttemptOf(failed(SessionFailure.HostKeyRejected)))
        assertEquals(WakeAttempt.FAILED, wakeAttemptOf(HostState.Closed(CloseReason.Disconnected)))
    }

    @Test
    fun wakeIsOfferedForAHostAsleepOrNotConnectedThatCanBeWoken() {
        val woken = listOf(testHost(macAddress = mac), testHost(wakeProbe = true), testHost(macAddress = mac, wakeProbe = true))
        for (host in woken) {
            for (link in listOf(LinkStatus.ASLEEP, LinkStatus.NOT_CONNECTED, LinkStatus.FAILED)) assertTrue("$link", canWake(host, link))
            for (link in listOf(LinkStatus.CONNECTED, LinkStatus.CONNECTING, LinkStatus.NEEDS_HOST_KEY)) assertFalse("$link", canWake(host, link))
        }
        assertFalse(canWake(testHost(), LinkStatus.ASLEEP)) // Neither a MAC address nor the probe: nothing to wake it with.
    }

    // --- the subnet-directed broadcast of the current network ---------------------------------------

    private fun ip(text: String) = InetAddress.getByName(text)

    @Test
    fun theDirectedBroadcastSetsEveryHostBitOfTheLinkAddress() {
        assertEquals("192.168.1.255", directedBroadcast(ip("192.168.1.20"), 24))
        assertEquals("10.255.255.255", directedBroadcast(ip("10.1.2.3"), 8))
        assertEquals("172.16.15.255", directedBroadcast(ip("172.16.5.4"), 20))
        assertEquals("192.168.1.131", directedBroadcast(ip("192.168.1.129"), 30))
        assertEquals("255.255.255.255", directedBroadcast(ip("200.1.2.3"), 1))
        assertEquals("127.255.255.255", directedBroadcast(ip("1.2.3.4"), 1))
    }

    @Test
    fun aLinkWithoutABroadcastAddressGivesNone() {
        // Point-to-point and single-address links (mobile data hands those out), no prefix, and IPv6 (no broadcast).
        assertNull(directedBroadcast(ip("100.64.10.7"), 32))
        assertNull(directedBroadcast(ip("100.64.10.7"), 31))
        assertNull(directedBroadcast(ip("192.168.1.20"), 0))
        assertNull(directedBroadcast(ip("fe80::1"), 64))
        assertNull(directedBroadcast(ip("2001:db8::5"), 64))
    }

    @Test
    fun aNetworksBroadcastsAreListedOnceEachInOrder() {
        assertEquals(
            listOf("192.168.1.255", "10.147.17.255"),
            directedBroadcasts(
                listOf(
                    ip("fe80::1") to 64, ip("192.168.1.20") to 24, ip("192.168.1.21") to 24, ip("100.64.0.1") to 32,
                    ip("10.147.17.3") to 24,
                ),
            ),
        )
        assertEquals(emptyList<String>(), directedBroadcasts(emptyList()))
    }

    // --- the wake probe before every connect, over the app's connections ----------------------------

    private fun TestScope.holder(
        connector: HostConnector, probe: suspend (List<HostEndpoint>) -> Unit,
        main: CoroutineDispatcher = StandardTestDispatcher(testScheduler),
        worker: CoroutineDispatcher = UnconfinedTestDispatcher(testScheduler),
    ) = HostConnections(connector, FakeTrust(), main, worker, wakeProbe = probe)

    @Test
    fun aHostWithTheProbeOnIsKnockedOnBeforeEveryConnectAndOthersAreNot() = runTest {
        val events = mutableListOf<String>()
        val holder = holder(
            connector = { request, listener ->
                events += "connect:${request.addresses.size}"
                listener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.Unreachable("no route"))))
                FakePort()
            },
            probe = { endpoints ->
                delay(1_500)
                events += "probe:${endpoints.joinToString { "${it.hostname}:${it.port}" }}"
            },
        )
        val probed = testHost(id = 1, wakeProbe = true, addresses = addresses)
        holder.connect(probed, byteArrayOf(1))
        assertEquals(listOf("probe:mac.local:22, 192.0.2.20:22", "connect:2"), events)
        testScheduler.runCurrent()

        // A reconnect (the same host after its connection failed) knocks again.
        holder.connect(probed, byteArrayOf(1))
        assertEquals(listOf("probe:mac.local:22, 192.0.2.20:22", "connect:2", "probe:mac.local:22, 192.0.2.20:22", "connect:2"), events)

        events.clear()
        holder.connect(testHost(id = 2), byteArrayOf(1))
        assertEquals(listOf("connect:1"), events)
    }

    @Test
    fun aWakeAttemptConnectsWithACopyOfTheKeyAndReadsTheSettledState() = runTest {
        var script: HostState = HostState.Closed(CloseReason.Failed(SessionFailure.TimedOut))
        val requests = mutableListOf<ByteArray>()
        val holder = holder(
            connector = { request, listener ->
                requests += request.privateKey
                listener.onHostStateChanged(HostState.Authenticating)
                listener.onHostStateChanged(script)
                FakePort().also { it.nativeState = script }
            },
            probe = {},
        )
        val host = testHost(id = 3)
        val key = byteArrayOf(4, 5, 6)
        assertEquals(WakeAttempt.NO_ANSWER, holder.wakeAttempt(host, key))
        assertArrayEquals(byteArrayOf(4, 5, 6), key) // The caller's key serves the next attempt...
        assertArrayEquals(ByteArray(3), requests.single()) // ...while the copy the connect was given is wiped.

        script = HostState.Connected(0u)
        assertEquals(WakeAttempt.ANSWERED, holder.wakeAttempt(host, key))
        assertEquals(2, requests.size)
    }
}

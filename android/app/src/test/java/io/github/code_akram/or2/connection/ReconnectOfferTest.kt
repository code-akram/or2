package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionFailure
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import io.github.code_akram.or2.ffi.HostListener
import org.junit.Assert.*
import org.junit.Test

/** The grouped reconnect offered when the app returns and an inbox host's SSH connection was lost. */
@OptIn(ExperimentalCoroutinesApi::class)
class ReconnectOfferTest {
    private val a = testHost(1, "Alpha", keyId = "k1")
    private val b = testHost(2, "Beta", keyId = "k1")
    private val c = testHost(3, "Gamma", keyId = "k2")

    @Test
    fun oneBiometricPerDistinctKeyAmongTheLostHosts() {
        val offer = reconnectOffer(listOf(a, b, c)) { true }!!
        assertEquals(listOf(a, b, c), offer.hosts)
        assertEquals(2, offer.prompts) // k1 once for Alpha and Beta, k2 once.
        assertEquals(1, reconnectOffer(listOf(a, b)) { true }!!.prompts)
        assertEquals(listOf(planUnlock(offer.hosts).groups.size), listOf(offer.prompts)) // The same grouping the connect uses.
    }

    @Test
    fun onlyLostInboxHostsWithAKeyAreOffered() {
        val hidden = testHost(4, "Hidden", keyId = "k1", showInInbox = false)
        val keyless = testHost(5, "Keyless", keyId = null)
        assertNull(reconnectOffer(listOf(hidden, keyless, a)) { false })
        val offer = reconnectOffer(listOf(hidden, keyless, a, b)) { it == 1L || it == 4L || it == 5L }!!
        assertEquals(listOf(a), offer.hosts)
        assertNull(reconnectOffer(emptyList()) { true })
    }

    private suspend fun TestScope.connectedHost(host: io.github.code_akram.or2.data.Host): Pair<HostConnections, HostListener> {
        val port = FakePort()
        var listener: HostListener? = null
        val holder = HostConnections({ _, l -> listener = l; port }, FakeTrust(), StandardTestDispatcher(testScheduler), UnconfinedTestDispatcher(testScheduler))
        holder.connect(host, byteArrayOf(1))
        listener!!.onHostStateChanged(HostState.Connected(0u))
        advanceUntilIdle()
        return holder to listener!!
    }

    @Test
    fun liveHostsDeliberateDisconnectsAndNeverConnectedHostsAreNotLost() = runTest {
        val (holder, listener) = connectedHost(a)
        assertNull(reconnectOffer(listOf(a), holder.hosts.value)) // Still connected.

        listener.onHostStateChanged(HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset"))))
        advanceUntilIdle()
        assertEquals(listOf(a), reconnectOffer(listOf(a, b), holder.hosts.value)!!.hosts)

        val (user, userListener) = connectedHost(a)
        user.disconnect(a.id)
        userListener.onHostStateChanged(HostState.Closed(CloseReason.Disconnected))
        advanceUntilIdle()
        assertNull(reconnectOffer(listOf(a), user.hosts.value))
    }

    @Test
    fun aSleepingHostIsNeverOffered() {
        val laptop = testHost(6, "MacBook", keyId = "k1", sleeps = true)
        assertNull(reconnectOffer(listOf(laptop)) { true }) // Lost, but it sleeps: "asleep", not an offer.
        val offer = reconnectOffer(listOf(laptop, a, b)) { true }!!
        assertEquals(listOf(a, b), offer.hosts)
        assertEquals(1, offer.prompts)
    }

    @Test
    fun theChipSaysWhoAndHowManyFingerprints() {
        assertEquals("Reconnect Alpha \u00b7 1 fingerprint", io.github.code_akram.or2.app.reconnectChipLabel(reconnectOffer(listOf(a)) { true }!!))
        assertEquals("Reconnect 2 hosts \u00b7 1 fingerprint", io.github.code_akram.or2.app.reconnectChipLabel(reconnectOffer(listOf(a, b)) { true }!!))
        assertEquals("Reconnect 3 hosts \u00b7 2 fingerprints", io.github.code_akram.or2.app.reconnectChipLabel(reconnectOffer(listOf(a, b, c)) { true }!!))
    }
}

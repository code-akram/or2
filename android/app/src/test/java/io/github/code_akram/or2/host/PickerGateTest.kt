package io.github.code_akram.or2.host

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** What the session picker opened from Home shows before its host is connected: progress, or why not with an action. */
class PickerGateTest {
    private fun host(keyId: String? = "k", sleeps: Boolean = false) =
        Host(HostRecord(1, "nas", "dev", keyId, true, sleeps = sleeps), listOf(HostEndpoint("nas.invalid", 22)))

    @Test
    fun aConnectedHostShowsTheLists() {
        assertNull(pickerGate(host(), HostState.Connected(0u), unlocking = false, busy = false))
        assertNull(pickerGate(host(), HostState.Connected(0u), unlocking = true, busy = true))
    }

    @Test
    fun aConnectingHostShowsItsCardsProgressLineWithASpinner() {
        assertEquals(PickerGate.Connecting("nas", "Unlocking key…", spinning = true), pickerGate(host(), null, unlocking = true, busy = true))
        assertEquals(PickerGate.Connecting("nas", "Checking server…", true), pickerGate(host(), HostState.Connecting, false, busy = false))
        assertEquals(PickerGate.Connecting("nas", "Authenticating…", true), pickerGate(host(), HostState.Authenticating, true, busy = false))
        // A host-key decision waits on the dialog, not on the network: no spinner.
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("ssh-ed25519", "k", "SHA256:x", ""), emptyList())
        assertEquals(PickerGate.Connecting("nas", "Waiting for host-key approval", false), pickerGate(host(), prompt, false, false))
    }

    @Test
    fun aFailureSaysWhyAndOffersRetryOnceNothingElseIsUnlocking() {
        val failed = HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))
        val gate = pickerGate(host(), failed, unlocking = false, busy = false) as PickerGate.Stopped
        assertEquals("Authentication rejected. Check the username and public-key authorization.", gate.reason)
        assertEquals(true, gate.failed)
        assertEquals(GateAction.RETRY, gate.action)
        assertEquals(true, gate.enabled)
        assertEquals(false, (pickerGate(host(), failed, unlocking = false, busy = true) as PickerGate.Stopped).enabled)
    }

    @Test
    fun anAsleepHostIsMutedAndRetries() {
        val lost = HostState.Closed(CloseReason.Failed(SessionFailure.ConnectionLost("reset")))
        val gate = pickerGate(host(sleeps = true), lost, unlocking = false, busy = false) as PickerGate.Stopped
        assertEquals("Asleep", gate.reason)
        assertEquals(false, gate.failed)
        assertEquals(GateAction.RETRY, gate.action)
    }

    @Test
    fun aHostThatIsSimplyNotConnectedOffersToConnectAndOneWithoutAKeyToSelectOne() {
        // A cancelled unlock leaves no connection; a disconnect leaves a Disconnected one.
        val idle = pickerGate(host(), null, unlocking = false, busy = false) as PickerGate.Stopped
        assertEquals(PickerGate.Stopped("nas", "Not connected", false, null, GateAction.CONNECT, true), idle)
        assertEquals(idle, pickerGate(host(), HostState.Closed(CloseReason.Disconnected), unlocking = false, busy = false))
        val keyless = pickerGate(host(keyId = null), null, unlocking = false, busy = true) as PickerGate.Stopped
        assertEquals(GateAction.SELECT_KEY, keyless.action)
        assertEquals(true, keyless.enabled) // Editing the host needs no unlock.
    }
}

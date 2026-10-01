package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostAddressRecord
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.HostWithAddresses
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.data.TrustedHostKey
import io.github.code_akram.or2.ffi.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow

fun testHost(
    id: Long = 7, label: String = "Fixture", keyId: String? = "ephemeral",
    addresses: List<HostEndpoint> = listOf(HostEndpoint("fixture.invalid", 2222)), showInInbox: Boolean = true,
    transport: TransportPref = TransportPref.AUTO,
) = Host(HostRecord(id, label, "fixture", keyId, showInInbox, transport), addresses)

val testPublicKey = PublicKeyInfo("test-algorithm", "test-public-line", "test-fingerprint", "")
val testPrompt = HostState.AwaitingHostKeyDecision(testPublicKey, emptyList())

class FakeTrust(val events: MutableList<String> = mutableListOf()) : TrustStore {
    var lines = listOf("prior-line-one", "prior-line-two")
    var fail = false
    var persistedHost: Host? = null
    var gate: CompletableDeferred<Unit>? = null
    var replacements = 0
    override suspend fun trustedKeys(hostId: Long) = lines
    override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) {
        replacements++
        gate?.await()
        if (fail) error("storage failure")
        persistedHost = host
        lines = listOf(presented.openssh)
        events += "persist"
    }
}

class FakeSession(val events: MutableList<String> = mutableListOf(), val transport: TerminalTransport = TerminalTransport.SSH) : SessionInterface, AutoCloseable {
    var destroyed = false
    var roams = 0
    var nativeState: SessionState = SessionState.Connecting
    val lastFrame = TerminalFrame(1uL, 2u, 1u, true,
        listOf(CellStyle(0xffffffu, 0u, null, Underline.NONE, false, false, false, false, false)),
        listOf(TerminalRow(0u, false, listOf(TerminalCell("L", CellWidth.NARROW, 0u), TerminalCell("R", CellWidth.NARROW, 0u)))),
        null, 0u, Scrollback(1uL, 0uL))
    var pending: TerminalFrame? = lastFrame
    override fun transport() = transport
    override fun roam() { roams++ }
    override fun approveHostKey(fingerprint: String) = Unit
    override fun rejectHostKey() = Unit
    override fun disconnect() {
        check(!destroyed) { "Session object has already been destroyed" }
        events += "disconnect"
    }
    override fun close() { destroyed = true; events += "close" }
    override fun requestFullFrame() = Unit
    override fun resize(columns: UShort, rows: UShort) = Unit
    override fun scroll(scroll: ViewportScroll) = Unit
    override fun sendKey(input: KeyInput) = Unit
    override fun sendText(text: String) = Unit
    override fun submitText(text: String) = Unit
    override fun state() = nativeState
    override fun takeFrame(): TerminalFrame? {
        check(!destroyed) { "Session object has already been destroyed" }
        return pending.also { pending = null }
    }
}

class FakeWatch(val events: MutableList<String> = mutableListOf()) : HerdrWatchInterface, AutoCloseable {
    var current: HerdrState = HerdrState.Starting
    var stops = 0
    var closes = 0
    var stopFailure: Exception? = null
    override fun state() = current
    override fun stop() { stops++; events += "stop"; stopFailure?.let { throw it } }
    override fun close() { closes++; events += "close" }
}

/** One scripted host connection: records what the holder asks and lets tests drive callbacks. */
class FakePort(val events: MutableList<String> = mutableListOf()) : HostPort {
    var nativeState: HostState = HostState.Connecting
    var approved: String? = null
    var destroyed = false
    var caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, "C.UTF-8", listOf(HerdrSessionInfo("default", true, true)))
    var capsFailure: Exception? = null
    /** While set, the capability probe is unanswered: `capabilities()` waits for it to complete. */
    var capsGate: CompletableDeferred<Unit>? = null
    var tmux = listOf<TmuxSession>()
    var openFailure: Exception? = null
    var watchFailure: Exception? = null
    var watchStopFailure: Exception? = null
    var capabilityCalls = 0
    /** `focus:<session>:<pane>` entries go to [events] in call order; a failure is thrown after the call is recorded. */
    val focused = mutableListOf<Pair<String?, String>>()
    val focusFailures = mutableMapOf<String, Exception>()
    var focusGate: CompletableDeferred<Unit>? = null
    val terminals = mutableListOf<Triple<TerminalTarget, SessionListener, FakeSession>>()

    /** The transport each `openTerminal` call asked for, in call order. */
    val transports = mutableListOf<TerminalTransport>()
    val watches = mutableListOf<Triple<String?, HerdrListener, FakeWatch>>()

    override fun state() = nativeState
    override fun approveHostKey(fingerprint: String) { approved = fingerprint; events += "approve" }
    override fun rejectHostKey() { events += "reject" }
    override fun disconnect() {
        check(!destroyed) { "Host connection object has already been destroyed" }
        events += "disconnect"
    }
    override fun close() { destroyed = true; events += "close" }
    override fun openTerminal(target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, listener: SessionListener): SessionInterface {
        check(!destroyed) { "Host connection object has already been destroyed" }
        openFailure?.let { throw it }
        transports += transport
        return FakeSession(transport = transport).also { terminals += Triple(target, listener, it) }
    }
    override suspend fun capabilities(): HostCapabilities {
        capabilityCalls++
        capsGate?.await()
        capsFailure?.let { throw it }
        return caps
    }
    override suspend fun listTmuxSessions() = tmux
    override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface {
        check(!destroyed) { "Host connection object has already been destroyed" }
        watchFailure?.let { throw it }
        return FakeWatch(events).also { it.stopFailure = watchStopFailure; watches += Triple(session, listener, it) }
    }
    override suspend fun focusHerdrPane(session: String?, paneId: String) {
        events += "focus:$session:$paneId"
        focusGate?.await()
        focusFailures[paneId]?.let { throw it }
        focused += session to paneId
    }
}

/** An in-memory [AppDao]: the same transactional `saveHost`/`replaceTrust` logic over fake rows. */
class FakeDao : AppDao() {
    val events = mutableListOf<String>()
    var failDelete = false
    var failSave = false
    val records = MutableStateFlow<List<HostRecord>>(emptyList())
    val addresses = MutableStateFlow<List<HostAddressRecord>>(emptyList())
    val trust = mutableListOf<TrustedHostKey>()
    val keysFlow = MutableStateFlow<List<KeyRecord>>(emptyList())
    private var nextId = 1L

    private fun rows() = records.value.map { record ->
        HostWithAddresses(record, addresses.value.filter { it.hostId == record.id })
    }

    override fun hostRows() = kotlinx.coroutines.flow.combine(records, addresses) { _, _ -> rows() }
    override suspend fun hostRow(id: Long) = rows().find { it.record.id == id }
    override fun keys() = keysFlow
    override suspend fun key(id: String) = keysFlow.value.find { it.id == id }
    override suspend fun insertKey(key: KeyRecord) = Unit
    override suspend fun deleteKey(id: String) {
        if (failDelete) error("storage failure")
        events += "record:$id"
    }
    override suspend fun insertHost(host: HostRecord): Long {
        if (failSave) error("storage failure")
        val id = if (host.id == 0L) nextId++ else host.id
        records.value += host.copy(id = id)
        return id
    }
    override suspend fun insertAddresses(addresses: List<HostAddressRecord>) { this.addresses.value += addresses }
    override suspend fun deleteAddresses(hostId: Long) { addresses.value = addresses.value.filterNot { it.hostId == hostId } }
    override suspend fun updateHost(id: Long, label: String, username: String, keyId: String?, showInInbox: Boolean, transport: TransportPref) {
        if (failSave) error("storage failure")
        records.value = records.value.map { if (it.id == id) HostRecord(id, label, username, keyId, showInInbox, transport) else it }
    }
    override suspend fun deleteHost(id: Long) {
        if (failDelete) error("storage failure")
        records.value = records.value.filterNot { it.id == id }
        addresses.value = addresses.value.filterNot { it.hostId == id }
        trust.removeAll { it.hostId == id }
    }
    override suspend fun trustedKeys(hostId: Long) = trust.filter { it.hostId == hostId }.map { it.openssh }
    override suspend fun clearTrust(hostId: Long) { trust.removeAll { it.hostId == hostId } }
    override suspend fun insertTrust(key: TrustedHostKey) { trust += key }
}

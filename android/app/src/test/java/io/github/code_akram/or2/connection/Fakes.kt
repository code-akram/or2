package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.MemoryPrefStore
import io.github.code_akram.or2.app.PrefStore
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
    transport: TransportPref = TransportPref.AUTO, sleeps: Boolean = false,
    macAddress: String? = null, wakeProbe: Boolean = false,
) = Host(HostRecord(id, label, "fixture", keyId, showInInbox, transport, sleeps, macAddress = macAddress, wakeProbe = wakeProbe), addresses)

/** Whether any terminal has not closed and was not dismissed. */
fun HostConnections.hasOpenSession(): Boolean = terminals.value.any { !it.retired && it.state.value !is SessionState.Closed }

/** Every pid the [MoshServerLedger] in [store] holds for [hostId], whatever destination it was started through. */
fun recordedServerPids(store: PrefStore, hostId: Long): List<UInt> = store.getString("mosh_servers").orEmpty().split(",")
    .map { it.split(":") }.filter { it.size == 3 && it[0] == hostId.toString() }.map { it[1].toUInt() }

val testPublicKey =PublicKeyInfo("test-algorithm", "test-public-line", "test-fingerprint", "")
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

    /** What `serverPid()` answers: the `mosh-server` a mosh session started; null for SSH. */
    var pid: UInt? = null

    /** Its listener was told [pid] (`FakePort.serverStarted`). */
    var pidAnnounced = false
    val lastFrame = TerminalFrame(1uL, 2u, 1u, true,
        listOf(CellStyle(0xffffffu, 0u, null, Underline.NONE, false, false, false, false, false)),
        listOf(TerminalRow(0u, false, listOf(TerminalCell("L", CellWidth.NARROW, 0u), TerminalCell("R", CellWidth.NARROW, 0u)))),
        null, 0u, Scrollback(1uL, 0uL), TerminalModes(false, false))
    var pending: TerminalFrame? = lastFrame

    /** What `clientId()` answers: one of its own for a tmux terminal (see [FakePort.openTerminal]). */
    var client: String? = null
    override fun transport() = transport
    override fun serverPid() = pid
    override fun clientId() = client
    override fun roam() { roams++ }
    private var disconnected = false

    /** Input the session took, in order (`text:`, `key:`, `submit:`, `scroll:`); after `disconnect` input is refused as Rust does. */
    val inputs = mutableListOf<String>()
    override fun disconnect() {
        check(!destroyed) { "Session object has already been destroyed" }
        disconnected = true
        events += "disconnect"
    }
    override fun close() { destroyed = true; events += "close" }
    override fun requestFullFrame() = Unit
    override fun resize(columns: UShort, rows: UShort) = Unit
    private fun take(input: String) {
        check(!destroyed) { "Session object has already been destroyed" }
        if (disconnected) throw SessionException.NotConnected()
        inputs += input
    }
    override fun scroll(scroll: ViewportScroll) = take("scroll:$scroll")
    override fun mouseClick(column: UShort, row: UShort) = take("click:$column,$row")
    override fun sendKey(input: KeyInput) = take("key:${input.key}")
    override fun sendText(text: String) = take("text:$text")
    override fun submitText(text: String) = take("submit:$text")
    override fun pasteText(text: String) = take("paste:$text")
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
/** One `read_history` call as [FakePort] received it. */
data class HistoryRead(val target: TerminalTarget, val paneId: String?, val clientId: String?, val lines: UInt)

class FakePort(val events: MutableList<String> = mutableListOf()) : HostPort {
    var nativeState: HostState = HostState.Connecting
    var approved: String? = null
    var destroyed = false
    var caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, listOf(HerdrSessionInfo("default", true, true)))
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

    /** The mosh budget (`moshBudgetMs`) each `openTerminal` call passed, in call order. */
    val budgets = mutableListOf<UInt?>()

    /** The `mosh-server` pid each mosh session this port opens reports; null makes a session report none. */
    var nextServerPid: UInt? = null

    /** Pids `stopMoshServer` was asked for, in call order; a failure is thrown after the call is recorded. */
    val stopped = mutableListOf<UInt>()
    var stopFailure: Exception? = null
    val watches = mutableListOf<Triple<String?, HerdrListener, FakeWatch>>()

    override fun state() = nativeState
    override fun approveHostKey(fingerprint: String) { approved = fingerprint; events += "approve" }
    override fun rejectHostKey() { events += "reject" }
    override fun disconnect() {
        check(!destroyed) { "Host connection object has already been destroyed" }
        events += "disconnect"
    }
    override fun close() { destroyed = true; events += "close" }
    override fun openTerminal(
        target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, moshBudgetMs: UInt?, listener: SessionListener,
    ): SessionInterface {
        check(!destroyed) { "Host connection object has already been destroyed" }
        openFailure?.let { throw it }
        transports += transport
        budgets += moshBudgetMs
        return FakeSession(transport = transport).also {
            if (transport == TerminalTransport.MOSH) it.pid = nextServerPid
            // Like Rust: every tmux session has a client id of its own.
            if (target is TerminalTarget.Tmux) it.client = "client-${terminals.size}"
            terminals += Triple(target, listener, it)
        }
    }
    override suspend fun capabilities(): HostCapabilities {
        capabilityCalls++
        capsGate?.await()
        capsFailure?.let { throw it }
        return caps
    }
    /** While set, `mosh_server()` is unanswered until it completes. */
    var moshServerGate: CompletableDeferred<Unit>? = null
    var moshServerFailure: Exception? = null

    /** The next this many `mosh_server()` calls fail (a transient failure), then [moshServerFailure] decides. */
    var moshServerFailuresLeft = 0
    var moshServerCalls = 0
    override suspend fun moshServer(): String? {
        moshServerCalls++
        moshServerGate?.await()
        if (moshServerFailuresLeft > 0) {
            moshServerFailuresLeft--
            throw HostException.CommandFailed("no free channel")
        }
        moshServerFailure?.let { throw it }
        return caps.moshServer
    }

    /**
     * What Rust does once the mosh session [index] opened has started its server (if it names one):
     * `on_server_pid`, on its own thread, before that session can report `Connected`. Once per session.
     */
    fun serverStarted(index: Int) {
        val (_, listener, session) = terminals[index]
        val pid = session.pid ?: return
        if (session.pidAnnounced) return
        session.pidAnnounced = true
        listener.onServerPid(pid)
    }
    var directories: List<String> = emptyList()
    var directoriesGate: CompletableDeferred<Unit>? = null
    var directoriesFailure: HostException? = null
    var directoryCalls = 0
    override suspend fun recentDirectories(): List<String> {
        directoryCalls++
        directoriesGate?.await()
        directoriesFailure?.let { throw it }
        return directories
    }
    override suspend fun listTmuxSessions() = tmux
    override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface {
        check(!destroyed) { "Host connection object has already been destroyed" }
        watchFailure?.let { throw it }
        return FakeWatch(events).also { it.stopFailure = watchStopFailure; watches += Triple(session, listener, it) }
    }
    override suspend fun stopMoshServer(pid: UInt) {
        events += "stop:$pid"
        stopped += pid
        stopFailure?.let { throw it }
    }
    /** `scroll_target` calls in order: the target, the herdr pane passed and the scroll. */
    val scrolls = mutableListOf<Triple<TerminalTarget, String?, TargetScroll>>()
    /** While set, a `scroll_target` call waits for it; a failure is thrown after the call is recorded. */
    var scrollGate: CompletableDeferred<Unit>? = null
    var scrollFailure: Exception? = null
    /** The client id each `scroll_target` call carried, in the same order as [scrolls]. */
    val scrollClients = mutableListOf<String?>()
    override suspend fun scrollTarget(target: TerminalTarget, paneId: String?, scroll: TargetScroll, clientId: String?) {
        scrolls += Triple(target, paneId, scroll)
        scrollClients += clientId
        scrollGate?.await()
        scrollFailure?.let { throw it }
    }

    /** `read_history` calls in order: the target, the herdr pane, the client id and the line count. */
    val historyReads = mutableListOf<HistoryRead>()
    /** What `read_history` answers; [historyFailure] is thrown instead (after the call is recorded) while set. */
    var history = HistoryText("", false)
    var historyFailure: Exception? = null
    var historyGate: CompletableDeferred<Unit>? = null
    override suspend fun readHistory(target: TerminalTarget, paneId: String?, clientId: String?, lines: UInt): HistoryText {
        historyReads += HistoryRead(target, paneId, clientId, lines)
        historyGate?.await()
        historyFailure?.let { throw it }
        return history
    }

    /** `navigate` calls in order; a failure is thrown after the call is recorded. */
    val navigations = mutableListOf<Triple<TerminalTarget, String?, TargetNav>>()

    /** The client id each `navigate` call carried, in the same order as [navigations]. */
    val navigationClients = mutableListOf<String?>()
    var navigateFailure: Exception? = null
    var navigateGate: CompletableDeferred<Unit>? = null
    override suspend fun navigate(target: TerminalTarget, paneId: String?, nav: TargetNav, clientId: String?) {
        events += "navigate"
        navigateGate?.await()
        navigations += Triple(target, paneId, nav)
        navigationClients += clientId
        navigateFailure?.let { throw it }
    }
    /** `reply_to_pane` calls in order: session, pane and text (the agent in [replyAgents]); a failure is thrown after the call is recorded. */
    val replies = mutableListOf<Triple<String?, String, String>>()
    val replyAgents = mutableListOf<AgentIdentity>()
    var replyRoute = ReplyRoute.PROMPTED
    var replyFailure: Exception? = null
    var replyGate: CompletableDeferred<Unit>? = null
    override suspend fun replyToPane(
        session: String?, paneId: String, agent: AgentIdentity, text: String,
    ): ReplyRoute {
        events += "reply:$session:$paneId"
        replies += Triple(session, paneId, text)
        replyAgents += agent
        replyGate?.await()
        replyFailure?.let { throw it }
        return replyRoute
    }

    /** What `permission_prompt` answers (each call is recorded in [prompts]); [promptFailure] is thrown instead. */
    var permission: PermissionPrompt? = null
    var promptFailure: Exception? = null
    val prompts = mutableListOf<Pair<String, AgentIdentity>>()
    override suspend fun permissionPrompt(session: String?, paneId: String, agent: AgentIdentity): PermissionPrompt? {
        events += "permission:$session:$paneId"
        prompts += paneId to agent
        promptFailure?.let { throw it }
        return permission
    }

    /** `answer_permission` calls in order: pane, seq and answer; a failure is thrown after the call is recorded. */
    val answers = mutableListOf<Triple<String, ULong, PermissionAnswer>>()
    var answerFailure: Exception? = null
    override suspend fun answerPermission(
        session: String?, paneId: String, agent: AgentIdentity, seq: ULong, answer: PermissionAnswer,
    ) {
        events += "answer:$session:$paneId"
        answers += Triple(paneId, seq, answer)
        answerFailure?.let { throw it }
    }

    /** `upload_image` calls in order (the extension and the size); [uploadGate] holds each, [uploadFailure] is thrown after. */
    val uploads = mutableListOf<Pair<String, Int>>()
    var uploadGate: CompletableDeferred<Unit>? = null
    var uploadFailure: Exception? = null
    override suspend fun uploadImage(bytes: ByteArray, extension: String): String {
        uploads += extension to bytes.size
        uploadGate?.await()
        uploadFailure?.let { throw it }
        return "/home/u/.cache/or2/images/or2-${uploads.size}.$extension"
    }
    /** `install_herdr_integration` ids in call order; a failure is thrown after the call is recorded. */
    val installs = mutableListOf<String>()
    var installFailure: Exception? = null
    var installGate: CompletableDeferred<Unit>? = null
    override suspend fun installHerdrIntegration(id: String) {
        installs += id
        installGate?.await()
        installFailure?.let { throw it }
    }

    /** What `herdr_integrations` answers (it is counted in [integrationCalls]); [integrationsFailure] is thrown instead. */
    var integrations = listOf<HerdrIntegration>()
    var integrationsFailure: Exception? = null
    var integrationCalls = 0
    override suspend fun herdrIntegrations(): List<HerdrIntegration> {
        integrationCalls++
        integrationsFailure?.let { throw it }
        return integrations
    }
    override suspend fun focusHerdrPane(session: String?, paneId: String) {
        events += "focus:$session:$paneId"
        focusGate?.await()
        focusFailures[paneId]?.let { throw it }
        focused += session to paneId
    }

    /** The tabs `focusHerdrTab` was asked for, in order; a [focusFailures] entry for the tab id is thrown. */
    val focusedTabs = mutableListOf<Pair<String?, String>>()
    override suspend fun focusHerdrTab(session: String?, tabId: String) {
        events += "focus-tab:$session:$tabId"
        focusFailures[tabId]?.let { throw it }
        focusedTabs += session to tabId
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
    override suspend fun updateHost(
        id: Long, label: String, username: String, keyId: String?, showInInbox: Boolean, transport: TransportPref, sleeps: Boolean,
        macAddress: String?, wakeProbe: Boolean,
    ) {
        if (failSave) error("storage failure")
        records.value = records.value.map {
            if (it.id == id) {
                it.copy(
                    label = label, username = username, keyId = keyId, showInInbox = showInInbox, transport = transport, sleeps = sleeps,
                    macAddress = macAddress, wakeProbe = wakeProbe,
                )
            } else it
        }
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

/**
 * A [PrefStore] with a disk: `putString` acts like `SharedPreferences.apply()` (read back at once, on [disk]
 * only after [flush]), `putStringDurably` like `commit()` (on [disk], with every earlier write, before it
 * returns). [disk] is what a new process would read after this one died.
 */
class DiskPrefStore : PrefStore {
    private val memory = MemoryPrefStore()
    val disk = MemoryPrefStore()
    private val pending = mutableListOf<(PrefStore) -> Unit>()

    override fun getBoolean(key: String) = memory.getBoolean(key)
    override fun putBoolean(key: String, value: Boolean) { memory.putBoolean(key, value); pending += { it.putBoolean(key, value) } }
    override fun getString(key: String) = memory.getString(key)
    override fun putString(key: String, value: String?) { memory.putString(key, value); pending += { it.putString(key, value) } }
    override fun putStringDurably(key: String, value: String?) { putString(key, value); flush() }

    /** The queued writes reach the disk (what `apply()` does some time later). */
    fun flush() {
        pending.forEach { it(disk) }
        pending.clear()
    }
}

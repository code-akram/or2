package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.HerdrListener
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrUnavailable
import io.github.code_akram.or2.ffi.HerdrWatchInterface
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostConnection
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ffi.connectHost
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.yield

/**
 * The host connection as the holder uses it. The generated `HostConnection` returns the concrete
 * `Session` and `HerdrWatch` classes, which cannot be faked on the JVM; this port returns their
 * interfaces. Production wraps the native object ([NativeHostPort]); tests supply fakes.
 */
interface HostPort : AutoCloseable {
    fun state(): HostState
    fun approveHostKey(fingerprint: String)
    fun rejectHostKey()
    fun disconnect()
    fun openTerminal(target: TerminalTarget, columns: UShort, rows: UShort, listener: SessionListener): SessionInterface
    suspend fun capabilities(): HostCapabilities
    suspend fun listTmuxSessions(): List<TmuxSession>
    fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface
}

class NativeHostPort(private val connection: HostConnection) : HostPort {
    override fun state() = connection.state()
    override fun approveHostKey(fingerprint: String) = connection.approveHostKey(fingerprint)
    override fun rejectHostKey() = connection.rejectHostKey()
    override fun disconnect() = connection.disconnect()
    override fun openTerminal(target: TerminalTarget, columns: UShort, rows: UShort, listener: SessionListener): SessionInterface =
        connection.openTerminal(target, columns, rows, listener)
    override suspend fun capabilities() = connection.capabilities()
    override suspend fun listTmuxSessions() = connection.listTmuxSessions()
    override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface =
        connection.watchHerdr(session, listener)
    override fun close() = connection.close()
}

/** Validates synchronously, then everything arrives through [HostListener]. */
fun interface HostConnector {
    fun connect(request: HostConnectRequest, listener: HostListener): HostPort

    companion object {
        /** The production connector: `connect_host` over UniFFI. */
        val Native = HostConnector { request, listener -> NativeHostPort(connectHost(request, listener)) }
    }
}

/** One herdr session watched on a host; its view feeds the inbox. */
class HerdrSessionWatch internal constructor(
    /** The name passed to `watch_herdr`: null for the default session. */
    val session: String?,
    /** The name the session is listed under (the default session has one too). */
    val name: String,
) {
    internal val mutableState = MutableStateFlow<HerdrState>(HerdrState.Starting)
    internal var handle: HerdrWatchInterface? = null
    val state = mutableState.asStateFlow()
}

/** One SSH connection to a host. Mutated only on the holder's main dispatcher. */
class ActiveHost internal constructor(val host: Host) {
    internal val ready = CompletableDeferred<HostPort>()
    internal val mutableState = MutableStateFlow<HostState>(HostState.Connecting)
    internal val mutableHasConnected = MutableStateFlow(false)
    internal val mutablePort = MutableStateFlow<HostPort?>(null)
    internal val mutableCapabilities = MutableStateFlow<HostCapabilities?>(null)
    internal val mutableCapabilitiesError = MutableStateFlow<String?>(null)
    internal val mutableWatches = MutableStateFlow<List<HerdrSessionWatch>>(emptyList())
    internal var retired = false
    internal var destroyed = false
    internal var disconnectRequested = false
    val state = mutableState.asStateFlow()
    val hasConnected = mutableHasConnected.asStateFlow()

    /** What the capability probe found; null until the host is connected and has answered. */
    val capabilities = mutableCapabilities.asStateFlow()

    /** A diagnostic when the capability probe failed. */
    val capabilitiesError = mutableCapabilitiesError.asStateFlow()
    val watches = mutableWatches.asStateFlow()

    /** True until the host has closed; a live host is never connected a second time. */
    val isLive get() = mutableState.value !is HostState.Closed && !disconnectRequested
}

/** One terminal session on a host connection. Several may be open per host. */
class ActiveTerminal internal constructor(val id: Long, val host: Host, val target: TerminalTarget) {
    internal val mutableState = MutableStateFlow<SessionState>(SessionState.Connecting)
    internal val mutableHasConnected = MutableStateFlow(false)
    internal val mutableFrames = MutableSharedFlow<Unit>(replay = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    internal val mutableHandle = MutableStateFlow<SessionInterface?>(null)
    internal var displays = 0
    internal var retired = false
    internal var destroyed = false
    internal var disconnectRequested = false
    val state = mutableState.asStateFlow()
    val hasConnected = mutableHasConnected.asStateFlow()
    val frameReady = mutableFrames.asSharedFlow()
    val handle = mutableHandle.asStateFlow()

    /** Short label for the session switcher. */
    val title: String
        get() = when (target) {
            TerminalTarget.Shell -> "shell"
            is TerminalTarget.Tmux -> "tmux ${target.sessionName}"
            is TerminalTarget.Herdr -> "herdr" + (target.session?.let { " $it" } ?: "") + (target.paneId?.let { " $it" } ?: "")
        }
}

/**
 * Application-owned: at most one [HostPort] per host and any number of terminals per
 * connection. All bookkeeping and listener delivery are confined to the main dispatcher.
 *
 * Host-key decisions, trust persistence and private-key wiping follow M1's session holder, per
 * host. Terminals keep M1's per-session lifecycle: the final frame stays readable through
 * `Closed`, and a display lease delays native `close()` until the screen leaves composition.
 */
class HostConnections(
    private val connector: HostConnector,
    private val trust: TrustStore,
    main: CoroutineDispatcher = Dispatchers.Main.immediate,
    private val worker: CoroutineDispatcher = Dispatchers.Default,
) {
    private val scope = CoroutineScope(SupervisorJob() + main)
    private val mutableHosts = MutableStateFlow<Map<Long, ActiveHost>>(emptyMap())
    private val mutableTerminals = MutableStateFlow<List<ActiveTerminal>>(emptyList())
    private var nextTerminalId = 1L

    /** Connections by host id, including closed ones the user has not dismissed or replaced. */
    val hosts = mutableHosts.asStateFlow()

    /** Open terminals in creation order, including closed ones the user has not dismissed. */
    val terminals = mutableTerminals.asStateFlow()

    fun host(hostId: Long): ActiveHost? = mutableHosts.value[hostId]

    fun terminal(id: Long): ActiveTerminal? = mutableTerminals.value.find { it.id == id }

    fun isLive(hostId: Long) = mutableHosts.value[hostId]?.isLive == true

    /** See [connect] for a list. */
    suspend fun connect(host: Host, privateKey: ByteArray) = connect(listOf(host), privateKey)

    /**
     * Call on main. Connects each host that has no live connection, in order, from one
     * decrypted key. The caller transfers the array: it is wiped after the last `connect_host`
     * call returns or throws, never between calls (the request keeps a reference to it). A
     * host whose connect fails does not stop the others; the first failure is rethrown after all
     * were attempted. Cancellation stops at once.
     */
    suspend fun connect(hosts: List<Host>, privateKey: ByteArray) {
        var failure: Exception? = null
        try {
            for (host in hosts) {
                try {
                    connectOne(host, privateKey)
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Exception) {
                    if (failure == null) failure = error
                }
            }
        } finally {
            privateKey.fill(0)
        }
        failure?.let { throw it }
    }

    private suspend fun connectOne(host: Host, privateKey: ByteArray) {
        if (isLive(host.id)) return
        var attempt: ActiveHost? = null
        try {
            val keys = trust.trustedKeys(host.id)
            // Re-check after the suspend: another connect may have started meanwhile.
            if (isLive(host.id)) return
            val current = ActiveHost(host)
            attempt = current
            replace(host.id, current)
            val listener = object : HostListener {
                override fun onHostStateChanged(state: HostState) {
                    scope.launch {
                        // Preserve a transient Connected even if the UI observes only Closed.
                        if (state is HostState.Connected) current.mutableHasConnected.value = true
                        current.mutableState.value = state
                        if (state is HostState.Connected) probe(current)
                    }
                }
            }
            val request = HostConnectRequest(
                host.addresses.map { HostAddress(it.hostname, it.port.toUShort()) }, host.username, privateKey, keys,
            )
            val port = withContext(worker) {
                connector.connect(request, listener).also {
                    // Store before returning through the cancellable dispatcher boundary.
                    current.mutablePort.value = it
                    current.ready.complete(it)
                }
            }
            // Disconnect/dismiss may have happened while the synchronous factory was running.
            if (mutableHosts.value[host.id] !== current) retireHost(current)
            else if (current.disconnectRequested) port.disconnect()
        } catch (error: Exception) {
            attempt?.ready?.completeExceptionally(error)
            if (attempt != null && mutableHosts.value[host.id] === attempt) mutableHosts.value -= host.id
            attempt?.let(::retireHost)
            throw error
        }
    }

    private fun replace(hostId: Long, current: ActiveHost) {
        val previous = mutableHosts.value[hostId]
        mutableHosts.value += hostId to current
        previous?.let(::retireHost)
    }

    /** Persist trust, bound to the presented key, and only then approve it. */
    suspend fun approve(current: ActiveHost, prompt: HostState.AwaitingHostKeyDecision) {
        val port = current.ready.await()
        // Native state can already be Closed while its listener delivery is queued on main.
        check(owns(current) && !current.disconnectRequested && current.state.value == prompt && port.state() == prompt) {
            "Host-key prompt has expired."
        }
        trust.replaceTrust(current.host, prompt.presented)
        // Persistence failure must never cause approval.
        if (owns(current) && !current.disconnectRequested && current.state.value == prompt) {
            port.approveHostKey(prompt.presented.fingerprint)
        }
    }

    suspend fun reject(current: ActiveHost) {
        val port = current.ready.await()
        if (owns(current)) port.rejectHostKey()
    }

    private fun owns(current: ActiveHost) = mutableHosts.value[current.host.id] === current

    /** Ends the connection; its terminals and watches close, and the closed state stays visible. */
    fun disconnect(hostId: Long) {
        val current = mutableHosts.value[hostId] ?: return
        current.disconnectRequested = true
        current.mutablePort.value?.disconnect()
    }

    /** Forgets a connection (disconnecting it first). Terminals keep their own lifecycle. */
    fun dismissHost(hostId: Long) {
        val current = mutableHosts.value[hostId] ?: return
        mutableHosts.value -= hostId
        retireHost(current)
    }

    /**
     * Host deleted or its destination changed: disconnect and forget the connection, and
     * dismiss its terminals when the host is gone ([closeTerminals]).
     */
    fun release(hostId: Long, closeTerminals: Boolean) {
        dismissHost(hostId)
        if (closeTerminals) mutableTerminals.value.filter { it.host.id == hostId }.forEach(::dismissTerminal)
    }

    private fun retireHost(current: ActiveHost) {
        if (current.destroyed) return
        current.retired = true
        current.disconnectRequested = true
        current.mutablePort.value?.let { port ->
            current.mutableWatches.value.forEach { stopWatch(it) }
            port.disconnect()
            current.destroyed = true
            port.close()
        }
        current.mutableWatches.value = emptyList()
    }

    private fun stopWatch(watch: HerdrSessionWatch) {
        val handle = watch.handle ?: return
        watch.handle = null
        try {
            handle.stop()
        } finally {
            (handle as? AutoCloseable)?.close()
        }
    }

    // --- capabilities and herdr watches -------------------------------------------------

    private suspend fun probe(current: ActiveHost) {
        try {
            val port = current.ready.await()
            val caps = port.capabilities()
            if (!owns(current) || current.retired) return
            current.mutableCapabilities.value = caps
            current.mutableCapabilitiesError.value = null
            syncWatches(current, port, caps)
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            if (owns(current)) current.mutableCapabilitiesError.value = error.message ?: error::class.simpleName
        }
    }

    /** Re-queries capabilities (new herdr sessions may have started) and updates the watches. */
    suspend fun refresh(current: ActiveHost) {
        if (current.state.value is HostState.Connected) probe(current)
    }

    /** One watch per running herdr session; sessions that vanished lose theirs. */
    private fun syncWatches(current: ActiveHost, port: HostPort, caps: HostCapabilities) {
        val running = if (caps.herdr == null) emptyList() else caps.herdrSessions.filter { it.running }
        val wanted = running.map { if (it.isDefault) null else it.name }.toSet()
        val kept = current.mutableWatches.value.filter { watch ->
            (watch.session in wanted).also { if (!it) stopWatch(watch) }
        }.toMutableList()
        for (info in running) {
            val session = if (info.isDefault) null else info.name
            if (kept.any { it.session == session }) continue
            val watch = HerdrSessionWatch(session, info.name)
            val listener = object : HerdrListener {
                override fun onHerdrStateChanged(state: HerdrState) {
                    scope.launch { watch.mutableState.value = state }
                }
            }
            try {
                watch.handle = port.watchHerdr(session, listener)
            } catch (error: HostException) {
                watch.mutableState.value = HerdrState.Unavailable(HerdrUnavailable.Failed, error.message.orEmpty())
            }
            kept += watch
        }
        current.mutableWatches.value = kept
    }

    /** tmux sessions on the host, most recently active first. */
    suspend fun listTmuxSessions(current: ActiveHost): List<TmuxSession> = current.ready.await().listTmuxSessions()

    // --- terminals ----------------------------------------------------------------------

    /** Call on main. Opens a terminal on a connected host; throws [HostException] if it cannot. */
    fun openTerminal(current: ActiveHost, target: TerminalTarget): ActiveTerminal {
        if (!owns(current) || current.retired) throw HostException.Closed()
        val port = current.mutablePort.value ?: throw HostException.NotConnected()
        val terminal = ActiveTerminal(nextTerminalId, current.host, target)
        val listener = object : SessionListener {
            override fun onStateChanged(state: SessionState) {
                scope.launch {
                    // Preserve a transient Connected even if the UI observes only Closed.
                    if (state == SessionState.Connected) terminal.mutableHasConnected.value = true
                    terminal.mutableState.value = state
                }
            }

            override fun onFrameReady() {
                scope.launch { terminal.mutableFrames.emit(Unit) }
            }
        }
        val session = port.openTerminal(target, 80u, 24u, listener)
        nextTerminalId++
        terminal.mutableHandle.value = session
        mutableTerminals.value += terminal
        return terminal
    }

    fun disconnectTerminal(terminal: ActiveTerminal) {
        terminal.disconnectRequested = true
        terminal.mutableHandle.value?.disconnect()
    }

    /** Retire ownership, but never destroy a handle still leased by a composed terminal screen. */
    fun dismissTerminal(terminal: ActiveTerminal) {
        if (terminal !in mutableTerminals.value) return
        mutableTerminals.value -= terminal
        retireTerminal(terminal)
    }

    internal fun attachDisplay(terminal: ActiveTerminal) { terminal.displays++ }

    internal fun detachDisplay(terminal: ActiveTerminal) {
        check(terminal.displays > 0)
        terminal.displays--
        // Child terminal effects must finish disposal before native destruction.
        scope.launch { yield(); closeRetired(terminal) }
    }

    private fun retireTerminal(terminal: ActiveTerminal) {
        if (terminal.destroyed) return
        terminal.retired = true
        terminal.disconnectRequested = true
        terminal.mutableHandle.value?.disconnect()
        closeRetired(terminal)
    }

    private fun closeRetired(terminal: ActiveTerminal) {
        val session = terminal.mutableHandle.value ?: return
        if (terminal.retired && terminal.displays == 0 && !terminal.destroyed) {
            terminal.destroyed = true
            (session as? AutoCloseable)?.close()
        }
    }
}

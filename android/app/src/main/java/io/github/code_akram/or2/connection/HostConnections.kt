package io.github.code_akram.or2.connection

import io.github.code_akram.or2.app.TerminalActivations
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.MoshFailureStore
import io.github.code_akram.or2.data.TransportPref
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.CloseReason
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
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
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
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
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

    /**
     * [moshBudgetMs] (API 9, mosh only): the whole mosh start must reach `Connected` within it,
     * counted from this call, or the session closes `TimedOut` with its server stopped; null keeps
     * the 15 s default.
     */
    fun openTerminal(
        target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, moshBudgetMs: UInt?, listener: SessionListener,
    ): SessionInterface
    suspend fun capabilities(): HostCapabilities
    suspend fun listTmuxSessions(): List<TmuxSession>
    fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface

    /** API 6: resolves once herdr acknowledged the focus; `PaneNotFound` when the pane is gone. */
    suspend fun focusHerdrPane(session: String?, paneId: String)
}

class NativeHostPort(private val connection: HostConnection) : HostPort {
    override fun state() = connection.state()
    override fun approveHostKey(fingerprint: String) = connection.approveHostKey(fingerprint)
    override fun rejectHostKey() = connection.rejectHostKey()
    override fun disconnect() = connection.disconnect()
    override fun openTerminal(
        target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, moshBudgetMs: UInt?, listener: SessionListener,
    ): SessionInterface = connection.openTerminal(target, transport, columns, rows, moshBudgetMs, listener)
    override suspend fun capabilities() = connection.capabilities()
    override suspend fun listTmuxSessions() = connection.listTmuxSessions()
    override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface =
        connection.watchHerdr(session, listener)
    override suspend fun focusHerdrPane(session: String?, paneId: String) = connection.focusHerdrPane(session, paneId)
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

    /**
     * Set when mosh failed on this connection under AUTO (UDP blocked, no `mosh-server`): later
     * AUTO terminals on it go straight to SSH. The note explains why, in muted text.
     */
    internal var moshFallbackNote: String? = null

    /**
     * Epoch milliseconds until which AUTO skips mosh for this host, from an earlier connection's
     * failure ([Host.moshFailedUntil], persisted) or this one's. 0 is no memory.
     */
    internal var moshPausedUntil = host.moshFailedUntil

    /** AUTO must not try mosh on this connection now: it failed here, or recently on this host. */
    internal fun moshRejected(now: Long) = moshFallbackNote != null || moshPausedUntil > now

    /** Whether herdr watches should run: the host's inbox flag, which can change on a live connection. */
    internal var watching = host.showInInbox

    /**
     * The host's transport preference, which can change on a live connection ([host] is the
     * snapshot taken when it connected): terminals opened afterwards follow it.
     */
    internal var transportPref = host.transport
    val state = mutableState.asStateFlow()
    val hasConnected = mutableHasConnected.asStateFlow()

    /** What the capability probe found; null until the host is connected and has answered. */
    val capabilities = mutableCapabilities.asStateFlow()

    /** A diagnostic when the capability probe failed. */
    val capabilitiesError = mutableCapabilitiesError.asStateFlow()
    val watches = mutableWatches.asStateFlow()

    /** True until the host has closed; a live host is never connected a second time. */
    val isLive get() = mutableState.value !is HostState.Closed && !disconnectRequested

    /**
     * The SSH connection was up and then failed (network loss, a reset): worth offering a
     * reconnect. A deliberate disconnect, a remote exit and a connect that never succeeded are not.
     */
    val wasLost: Boolean
        get() {
            val state = mutableState.value
            return mutableHasConnected.value && !disconnectRequested && state is HostState.Closed && state.reason is CloseReason.Failed
        }
}

/** One terminal session on a host connection. Several may be open per host. */
class ActiveTerminal internal constructor(val id: Long, val host: Host, val target: TerminalTarget) {
    internal val mutableState = MutableStateFlow<SessionState>(SessionState.Connecting)
    internal val mutableHasConnected = MutableStateFlow(false)
    internal val mutableFrames = MutableSharedFlow<Unit>(replay = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    internal val mutableHandle = MutableStateFlow<SessionInterface?>(null)
    internal val mutableTransport = MutableStateFlow(TerminalTransport.SSH)
    internal val mutableLinkHealth = MutableStateFlow<LinkHealth?>(null)
    internal val mutableNote = MutableStateFlow<String?>(null)
    internal var displays = 0
    internal var retired = false
    internal var destroyed = false
    internal var disconnectRequested = false

    /** Which session object's callbacks count: a fallback to SSH replaces the handle and bumps this. */
    internal var attempt = 0

    /** AUTO chose mosh, so a `TimedOut` or `NotInstalled` before the first frame retries over SSH. */
    internal var fallbackEligible = false
    val state = mutableState.asStateFlow()
    val hasConnected = mutableHasConnected.asStateFlow()
    val frameReady = mutableFrames.asSharedFlow()
    val handle = mutableHandle.asStateFlow()

    /** The transport the session really runs over (the handle's own answer), SSH after a fallback. */
    val transport = mutableTransport.asStateFlow()

    /** Mosh only: the latest link health, null before the first report. */
    val linkHealth = mutableLinkHealth.asStateFlow()

    /** A muted explanation (AUTO fell back to SSH), or null. */
    val note = mutableNote.asStateFlow()

    /** Short label for the session switcher. */
    val title: String get() = targetTitle(target)
}

/** Short label for a terminal target: `shell`, `tmux main`, `herdr work w1:p2`. */
fun targetTitle(target: TerminalTarget): String = when (target) {
    TerminalTarget.Shell -> "shell"
    is TerminalTarget.Tmux -> "tmux ${target.sessionName}"
    is TerminalTarget.Herdr -> "herdr" + (target.session?.let { " $it" } ?: "") + (target.paneId?.let { " $it" } ?: "")
}

/** The user's own closing of a host or terminal (disconnect, close, delete, remote shell exit). */
interface UserCloseListener {
    fun hostClosed(hostId: Long)
    fun terminalClosed(hostId: Long, target: TerminalTarget)
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
    /** Where AUTO's per-host memory of a mosh failure is kept (Room); null remembers nothing. */
    private val moshFailures: MoshFailureStore? = null,
    /** Wall-clock milliseconds, for the failure memory's expiry. */
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val scope = CoroutineScope(SupervisorJob() + main)

    /** Told when the user (not the network) ends a host or terminal, so reattach forgets it. */
    var userClose: UserCloseListener? = null
    private val mutableHosts = MutableStateFlow<Map<Long, ActiveHost>>(emptyMap())
    private val mutableTerminals = MutableStateFlow<List<ActiveTerminal>>(emptyList())
    private var nextTerminalId = 1L

    /** Opening, reusing and switching to agent terminals, with the pane focus each needs first. */
    val activations = TerminalActivations(this, scope)

    /** Connections by host id, including closed ones the user has not dismissed or replaced. */
    val hosts = mutableHosts.asStateFlow()

    /** Open terminals in creation order, including closed ones the user has not dismissed. */
    val terminals = mutableTerminals.asStateFlow()

    fun host(hostId: Long): ActiveHost? = mutableHosts.value[hostId]

    fun terminal(id: Long): ActiveTerminal? = mutableTerminals.value.find { it.id == id }

    /** A terminal on [hostId] for exactly [target] that has not closed or been told to, if any. */
    fun findOpenTerminal(hostId: Long, target: TerminalTarget): ActiveTerminal? = mutableTerminals.value.find {
        it.host.id == hostId && it.target == target && !it.retired && !it.disconnectRequested &&
            it.state.value !is SessionState.Closed
    }

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
                        if (state is HostState.Closed) releaseWatches(current)
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
        } catch (error: Throwable) {
            // Errors too (a missing native library, an out-of-memory while lowering): an attempt
            // that never produced a live connection must not stay listed as Connecting.
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
        userClose?.hostClosed(hostId)
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
        if (closeTerminals) {
            userClose?.hostClosed(hostId)
            mutableTerminals.value.filter { it.host.id == hostId }.forEach(::dismissTerminal)
        }
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

    /** Never throws: one watch that fails to stop must not abort a sync or a connection's teardown. */
    private fun stopWatch(watch: HerdrSessionWatch) {
        val handle = watch.handle ?: return
        watch.handle = null
        try {
            handle.stop()
        } catch (_: Exception) {
            // The watch ends with its connection anyway.
        } finally {
            try {
                (handle as? AutoCloseable)?.close()
            } catch (_: Exception) {
            }
        }
    }

    /**
     * The host closed: every watch is over, so release their native handles now. The watches stay
     * listed with their final state; the connection object itself stays readable until the host
     * is replaced or dismissed.
     */
    private fun releaseWatches(current: ActiveHost) = current.mutableWatches.value.forEach(::stopWatch)

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

    /** Re-queries capabilities and brings the watches in line with them. */
    suspend fun refresh(current: ActiveHost) {
        if (current.state.value is HostState.Connected) probe(current)
    }

    /**
     * Follows the host's inbox flag on a live connection: turning it off stops every watch (a
     * hidden host should not cost channels and radio wakeups), turning it on starts them again.
     */
    fun setWatching(hostId: Long, watching: Boolean) {
        val current = mutableHosts.value[hostId] ?: return
        if (current.watching == watching) return
        current.watching = watching
        if (current.retired || current.state.value !is HostState.Connected) return
        val port = current.mutablePort.value ?: return
        val caps = current.mutableCapabilities.value ?: return
        syncWatches(current, port, caps)
    }

    /**
     * Follows the host's transport preference on a live connection: terminals opened afterwards
     * use it, terminals already open keep what they run over. A changed preference is a fresh
     * decision, so the memory of an earlier mosh failure on this connection ([ActiveHost.moshFallbackNote])
     * is dropped with it.
     */
    fun setTransport(hostId: Long, pref: TransportPref) {
        val current = mutableHosts.value[hostId] ?: return
        if (current.transportPref == pref) return
        current.transportPref = pref
        current.moshFallbackNote = null
        // Room's `saveHost` clears the stored memory in the same write as the new preference.
        current.moshPausedUntil = 0
    }

    /**
     * With herdr installed, the default session (watched unnamed) and every listed session get a
     * watch whether or not it is running: the watch reports `NotRunning` and retries by itself, so
     * a session started later is picked up without another probe (the connection caches the
     * probe, so a refresh cannot see it). Sessions no longer listed lose theirs.
     */
    private fun syncWatches(current: ActiveHost, port: HostPort, caps: HostCapabilities) {
        val active = current.watching && caps.herdr != null && current.state.value !is HostState.Closed
        val listed = if (active) caps.herdrSessions else emptyList()
        val wanted = if (active) {
            listOf(HerdrWatchSpec(null, listed.find { it.isDefault }?.name ?: "default")) +
                listed.filterNot { it.isDefault }.map { HerdrWatchSpec(it.name, it.name) }
        } else {
            emptyList()
        }
        val sessions = wanted.map { it.session }.toSet()
        val kept = current.mutableWatches.value.filter { watch ->
            (watch.session in sessions).also { if (!it) stopWatch(watch) }
        }.toMutableList()
        for (spec in wanted) {
            if (kept.any { it.session == spec.session }) continue
            val watch = HerdrSessionWatch(spec.session, spec.name)
            val listener = object : HerdrListener {
                override fun onHerdrStateChanged(state: HerdrState) {
                    scope.launch { watch.mutableState.value = state }
                }
            }
            try {
                watch.handle = port.watchHerdr(spec.session, listener)
            } catch (error: HostException) {
                watch.mutableState.value = HerdrState.Unavailable(HerdrUnavailable.Failed, error.message.orEmpty())
            }
            kept += watch
        }
        current.mutableWatches.value = kept
    }

    /**
     * Focuses [paneId] in herdr [session] (null: the default session) and returns once herdr
     * acknowledged. herdr's focus is shared state, so a reused agent terminal shows whichever pane
     * is focused now: the app calls this before showing an agent terminal again (see
     * `TerminalActivations`). Throws [HostException] (`PaneNotFound` when the pane has gone).
     */
    suspend fun focusHerdrPane(current: ActiveHost, session: String?, paneId: String) {
        if (!owns(current) || current.retired) throw HostException.Closed()
        val port = current.mutablePort.value ?: throw HostException.NotConnected()
        port.focusHerdrPane(session, paneId)
    }

    private class HerdrWatchSpec(val session: String?, val name: String)

    /** tmux sessions on the host, most recently active first. */
    suspend fun listTmuxSessions(current: ActiveHost): List<TmuxSession> = current.ready.await().listTmuxSessions()

    // --- terminals ----------------------------------------------------------------------

    /**
     * Call on main. Opens a terminal on a connected host; throws [HostException] if it cannot.
     * The transport follows the host's current preference ([chooseTransport]); under AUTO that
     * needs the capability probe, so callers that can suspend call [awaitTransportChoice] first
     * (a probe that has not answered counts as no `mosh-server`). Under AUTO a mosh terminal gets a
     * [AUTO_MOSH_BUDGET_MS] budget for its whole start, and one that fails with `TimedOut` or a missing
     * `mosh-server` before it connected is retried over SSH on the same [ActiveTerminal]; a timeout is
     * also remembered for the host for [MOSH_PAUSE_MS], and AUTO skips mosh until then.
     */
    fun openTerminal(current: ActiveHost, target: TerminalTarget): ActiveTerminal {
        if (!owns(current) || current.retired) throw HostException.Closed()
        val port = current.mutablePort.value ?: throw HostException.NotConnected()
        val pref = current.transportPref
        val moshServer = current.capabilities.value?.moshServer
        val now = clock()
        val choice = chooseTransport(pref, moshServer, current.moshRejected(now))
        val terminal = ActiveTerminal(nextTerminalId, current.host, target)
        terminal.fallbackEligible = pref == TransportPref.AUTO && choice == TerminalTransport.MOSH
        if (pref == TransportPref.AUTO && moshServer != null && choice == TerminalTransport.SSH) {
            terminal.mutableNote.value = current.moshFallbackNote ?: MOSH_PAUSED_NOTE
        }
        // AUTO gives mosh a short budget (the whole start, bootstrap included) so a blocked UDP path
        // costs seconds before SSH takes over; an explicit Mosh keeps the 15 s default.
        val budget = if (terminal.fallbackEligible) AUTO_MOSH_BUDGET_MS else null
        val session = port.openTerminal(target, choice, 80u, 24u, budget, sessionListener(terminal, current, attempt = 0))
        nextTerminalId++
        terminal.mutableTransport.value = session.transport()
        terminal.mutableHandle.value = session
        mutableTerminals.value += terminal
        return terminal
    }

    private fun sessionListener(terminal: ActiveTerminal, current: ActiveHost, attempt: Int) = object : SessionListener {
        override fun onStateChanged(state: SessionState) {
            scope.launch { sessionState(terminal, current, attempt, state) }
        }

        override fun onFrameReady() {
            scope.launch { if (attempt == terminal.attempt) terminal.mutableFrames.emit(Unit) }
        }

        override fun onLinkHealth(health: LinkHealth) {
            scope.launch { if (attempt == terminal.attempt) terminal.mutableLinkHealth.value = health }
        }
    }

    private fun sessionState(terminal: ActiveTerminal, current: ActiveHost, attempt: Int, state: SessionState) {
        // A replaced session (the mosh attempt after a fallback) is over; nothing it says counts.
        if (attempt != terminal.attempt) return
        if (state is SessionState.Closed && fallBackToSsh(terminal, current, state)) return
        // Preserve a transient Connected even if the UI observes only Closed.
        if (state == SessionState.Connected) terminal.mutableHasConnected.value = true
        terminal.mutableState.value = state
        // Nothing is heard on a closed session: its last health must not keep saying "Last heard N s ago".
        if (state is SessionState.Closed) terminal.mutableLinkHealth.value = null
        // A shell the user exited is over: reattach must not offer it again.
        if (state is SessionState.Closed && state.reason is CloseReason.RemoteExited) {
            userClose?.terminalClosed(terminal.host.id, terminal.target)
        }
    }

    /** Returns true when the mosh attempt was replaced by an SSH one (the closed state is not shown). */
    private fun fallBackToSsh(terminal: ActiveTerminal, current: ActiveHost, state: SessionState.Closed): Boolean {
        val failure = (state.reason as? CloseReason.Failed)?.failure ?: return false
        if (!terminal.fallbackEligible || !isMoshFallback(failure) || terminal.mutableHasConnected.value) return false
        if (terminal.disconnectRequested || terminal.retired || !owns(current) || current.retired) return false
        val port = current.mutablePort.value ?: return false
        val previous = terminal.mutableHandle.value
        val previousAttempt = terminal.attempt
        val note = moshFallbackNote(failure)
        terminal.attempt = previousAttempt + 1
        terminal.fallbackEligible = false
        terminal.mutableState.value = SessionState.Connecting
        try {
            val session = port.openTerminal(terminal.target, TerminalTransport.SSH, 80u, 24u, null,
                sessionListener(terminal, current, terminal.attempt))
            current.moshFallbackNote = note
            if (failure is SessionFailure.TimedOut) rememberMoshFailure(current)
            terminal.mutableNote.value = note
            terminal.mutableLinkHealth.value = null
            terminal.mutableTransport.value = session.transport()
            terminal.mutableHandle.value = session
        } catch (error: HostException) {
            // The retry could not even start (the host is gone): show the mosh failure as it was.
            terminal.attempt = previousAttempt
            terminal.mutableState.value = state
            return true
        }
        // The old object has nothing left to say; release it once its observers have moved on.
        if (previous != null) scope.launch { yield(); (previous as? AutoCloseable)?.close() }
        return true
    }

    /**
     * UDP did not get through: AUTO skips mosh for this host for [MOSH_PAUSE_MS] (also across
     * connections and restarts, through [moshFailures]). A missing `mosh-server` is not remembered:
     * the capability probe already says so on every connection, and installing it must just work.
     */
    private fun rememberMoshFailure(current: ActiveHost) {
        val until = clock() + MOSH_PAUSE_MS
        current.moshPausedUntil = until
        val store = moshFailures ?: return
        scope.launch {
            try {
                store.markMoshFailed(current.host.id, until)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                // Without the memory the next connection simply tries mosh once more.
            }
        }
    }

    /**
     * Waits (at most [timeoutMs]) for the capability probe of [current], so a terminal opened
     * right after connecting can choose mosh. Returns at once when the probe has answered.
     */
    suspend fun awaitCapabilities(current: ActiveHost, timeoutMs: Long = 3_000) {
        if (current.capabilities.value != null || current.capabilitiesError.value != null) return
        withTimeoutOrNull(timeoutMs) {
            merge(current.capabilities, current.capabilitiesError).filterNotNull().first()
        }
    }

    /**
     * Call before [openTerminal]: under AUTO the transport depends on the capability probe, so a
     * tap right after connecting waits for it (briefly) instead of silently choosing SSH. An
     * explicit SSH or Mosh preference never waits.
     */
    suspend fun awaitTransportChoice(current: ActiveHost) {
        if (current.transportPref == TransportPref.AUTO) awaitCapabilities(current)
    }

    /** The user ends this terminal. */
    fun disconnectTerminal(terminal: ActiveTerminal) {
        userClose?.terminalClosed(terminal.host.id, terminal.target)
        terminal.disconnectRequested = true
        terminal.mutableHandle.value?.disconnect()
    }

    /** True while any terminal has not closed: the foreground service and the battery prompt care. */
    fun hasOpenSession(): Boolean = mutableTerminals.value.any { !it.retired && it.state.value !is SessionState.Closed }

    /**
     * The notification's "Disconnect all": ends every terminal and every host connection (their
     * closed states stay visible, as after any disconnect). The service stops once all report Closed.
     */
    fun disconnectAll() {
        mutableTerminals.value.filter { !it.retired && it.state.value !is SessionState.Closed }.forEach(::disconnectTerminal)
        mutableHosts.value.values.filter { it.isLive }.forEach { disconnect(it.host.id) }
    }

    /** Retire ownership, but never destroy a handle still leased by a composed terminal screen. */
    fun dismissTerminal(terminal: ActiveTerminal) {
        if (terminal !in mutableTerminals.value) return
        userClose?.terminalClosed(terminal.host.id, terminal.target)
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

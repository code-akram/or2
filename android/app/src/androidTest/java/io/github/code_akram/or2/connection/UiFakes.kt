package io.github.code_akram.or2.connection

import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.*

/** Fakes for the device UI tests: no network, no Keystore, no production database. */
fun uiHost(
    id: Long = 1, label: String = "Fixture", keyId: String? = "fixture-key", showInInbox: Boolean = true,
    addresses: List<HostEndpoint> = listOf(HostEndpoint("fixture.invalid", 22)), sleeps: Boolean = false,
) = Host(HostRecord(id, label, "fixture-user", keyId, showInInbox, sleeps = sleeps), addresses)

class UiTrust : TrustStore {
    override suspend fun trustedKeys(hostId: Long) = emptyList<String>()
    override suspend fun replaceTrust(host: Host, presented: PublicKeyInfo) = Unit
}

class UiSession(private val initial: SessionState = SessionState.Connected) : SessionInterface, AutoCloseable {
    var listener: SessionListener? = null
    var destroyed = false
    var closes = 0
    var pending: TerminalFrame? = null
    var frameTakes = 0
    val frame = TerminalFrame(1uL, 2u, 1u, true,
        listOf(CellStyle(0xffffffu, 0u, null, Underline.NONE, false, false, false, false, false)),
        listOf(TerminalRow(0u, false, listOf(TerminalCell("L", CellWidth.NARROW, 0u), TerminalCell("R", CellWidth.NARROW, 0u)))),
        null, 0u, Scrollback(1uL, 0uL), TerminalModes(false, false))

    override fun takeFrame(): TerminalFrame? {
        check(!destroyed)
        frameTakes++
        return pending.also { pending = null }
    }
    override fun requestFullFrame() { check(!destroyed); pending = frame; listener?.onFrameReady() }
    override fun disconnect() { check(!destroyed); listener?.onStateChanged(SessionState.Closed(CloseReason.Disconnected)) }
    override fun close() { destroyed = true; closes++ }
    override fun resize(columns: UShort, rows: UShort) = Unit
    override fun sendText(text: String) = Unit
    override fun submitText(text: String) = Unit
    override fun sendKey(input: KeyInput) = Unit
    override fun scroll(scroll: ViewportScroll) = Unit
    override fun state() = initial
    override fun transport() = TerminalTransport.SSH
    override fun serverPid(): UInt? = null
    override fun roam() = Unit
    override fun approveHostKey(fingerprint: String) = Unit
    override fun rejectHostKey() = Unit
}

class UiWatch : HerdrWatchInterface, AutoCloseable {
    var current: HerdrState = HerdrState.Starting
    override fun state() = current
    override fun stop() = Unit
    override fun close() = Unit
}

/** A host connection that reports what it is told; sessions it opens are [UiSession]s. */
class UiPort(
    private val sessionState: SessionState = SessionState.Connected,
    private val sessionFor: (TerminalTarget) -> UiSession = { UiSession(sessionState) },
) : HostPort {
    var hostListener: HostListener? = null
    var native: HostState = HostState.Connecting
    var caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, "C.UTF-8", listOf(HerdrSessionInfo("default", true, true)))
    var tmux = listOf(TmuxSession("main", 2u, 1u, 0L, 10L))
    val sessions = mutableListOf<Pair<TerminalTarget, UiSession>>()
    val watchListeners = mutableListOf<HerdrListener>()

    override fun state() = native
    override fun approveHostKey(fingerprint: String) = Unit
    override fun rejectHostKey() = Unit
    override fun disconnect() { hostListener?.onHostStateChanged(HostState.Closed(CloseReason.Disconnected)) }
    override fun close() = Unit
    override fun openTerminal(target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, moshBudgetMs: UInt?, listener: SessionListener): SessionInterface {
        val session = sessionFor(target)
        session.listener = listener
        sessions += target to session
        if (sessionState != SessionState.Connecting) listener.onStateChanged(sessionState)
        return session
    }
    override suspend fun capabilities() = caps
    override suspend fun listTmuxSessions() = tmux
    override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface {
        watchListeners += listener
        return UiWatch()
    }
    override suspend fun focusHerdrPane(session: String?, paneId: String) = Unit
    override suspend fun stopMoshServer(pid: UInt) = Unit
    override suspend fun scrollTarget(target: TerminalTarget, paneId: String?, scroll: TargetScroll) = Unit
}

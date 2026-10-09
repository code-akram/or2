package io.github.code_akram.or2.demo

import android.os.Handler
import android.os.Looper
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.connection.HostPort
import io.github.code_akram.or2.ffi.AgentIdentity
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrIntegration
import io.github.code_akram.or2.ffi.HerdrIntegrationState
import io.github.code_akram.or2.ffi.HerdrListener
import io.github.code_akram.or2.ffi.HerdrPane
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWatchInterface
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.KeyInput
import io.github.code_akram.or2.ffi.PermissionAnswer
import io.github.code_akram.or2.ffi.PermissionPrompt
import io.github.code_akram.or2.ffi.ReplyRoute
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TargetNav
import io.github.code_akram.or2.ffi.TargetScroll
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ffi.ViewportScroll
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.util.concurrent.CopyOnWriteArrayList

/**
 * The README demo's made-up world: two hosts with herdr agents, tmux and shells, all scripted on the main thread. Nothing
 * here touches a network, a key or a real host; every name, path and address is invented (addresses are reserved
 * documentation ones). The app sees it through [connector], exactly where the native connection would be.
 */
internal class DemoWorld {
    val lock = Any()
    private val main = Handler(Looper.getMainLooper())
    private val token = Any()
    private val sessions = CopyOnWriteArrayList<DemoSession>()

    private fun screen() = DemoScreen(lock, ::changed)

    /** Every screen change redraws every open terminal: they are few, and frames are only built when taken. */
    private fun changed() = sessions.forEach { it.changed() }

    fun at(delayMs: Long, block: () -> Unit) {
        main.postAtTime(block, token, android.os.SystemClock.uptimeMillis() + delayMs)
    }

    fun stop() = main.removeCallbacksAndMessages(token)

    // --- the hosts -----------------------------------------------------------------------------------------

    val claude = DemoAgent("w1:p1", "w1:t1", "w1", "Claude Code", "claude", AgentStatus.BLOCKED, "~/code/or2", "Toolbar key heights", screen())
    val codex = DemoAgent("w1:p2", "w1:t2", "w1", "Codex", "codex", AgentStatus.WORKING, "~/code/or2", "Review frame decoder", screen())
    val pi = DemoAgent("w2:p1", "w2:t1", "w2", "pi", "pi", AgentStatus.DONE, "~/code/docs", "README pass", screen())
    private val personal = DemoHerdr(
        "personal",
        listOf(HerdrWorkspace("w1", 1u, "or2"), HerdrWorkspace("w2", 2u, "docs")),
        listOf(HerdrTab("w1:t1", "w1", 1u, "ui"), HerdrTab("w1:t2", "w1", 2u, "core"), HerdrTab("w2:t1", "w2", 1u, "readme")),
        listOf(claude, codex, pi), focused = claude.paneId,
    )

    val api = DemoAgent("w1:p1", "w1:t1", "w1", "Claude Code", "claude", AgentStatus.WORKING, "~/src/api", "Migrate auth tests", screen())
    val amp = DemoAgent("w1:p2", "w1:t2", "w1", "Amp", "amp", AgentStatus.IDLE, "~/src/web", null, screen())
    private val work = DemoHerdr(
        "work",
        listOf(HerdrWorkspace("w1", 1u, "api")),
        listOf(HerdrTab("w1:t1", "w1", 1u, "tests"), HerdrTab("w1:t2", "w1", 2u, "web")),
        listOf(api, amp), focused = api.paneId,
    )

    val workstation = DemoHost(
        "atlas", "atlas.local", personal, mapOf("main" to screen(), "build" to screen()),
        listOf(TmuxSession("main", 3u, 1u), TmuxSession("build", 1u, 0u)),
        listOf("/home/dev/code/or2", "/home/dev/code/docs", "/home/dev/code/herdr"), screen(),
    )
    val buildBox = DemoHost(
        "build-box", "198.51.100.7", work, mapOf("main" to screen(), "deploy" to screen()),
        listOf(TmuxSession("main", 2u, 0u), TmuxSession("deploy", 1u, 0u), TmuxSession("logs", 4u, 0u)),
        listOf("/home/dev/src/api", "/home/dev/src/web", "/home/dev/infra/terraform", "/home/dev/src/api/migrations"), screen(),
    )
    private val hosts = listOf(workstation, buildBox)

    /** What the app calls instead of `connect_host`: the host is found by its address, and connects at once. */
    val connector = HostConnector { request, listener ->
        val host = hosts.first { host -> request.addresses.any { it.host == host.address } }
        DemoPort(host, listener).also { port -> at(150) { listener.onHostStateChanged(port.state()) } }
    }

    // --- the scripts ----------------------------------------------------------------------------------------

    init {
        Content.claudeBlocked(claude.screen)
        Content.codexWorking(codex.screen)
        Content.piDone(pi.screen)
        Content.apiWorking(api.screen)
        Content.ampIdle(amp.screen)
        Content.tmuxBuild(workstation.tmux.getValue("main"))
        Content.tmuxBuild(workstation.tmux.getValue("build"))
        Content.deploy(buildBox.tmux.getValue("main"))
        Content.deploy(buildBox.tmux.getValue("deploy"))
        Content.shell(workstation.shell, "atlas", "~")
        Content.shell(buildBox.shell, "build-box", "~")
    }

    /** The background life of the hosts: the tmux test watcher on the workstation, the agent on the build box. */
    fun startAmbient() {
        var test = 0
        fun watcher() {
            Content.testTick(workstation.tmux.getValue("main"), test++)
            at(170) { watcher() }
        }
        watcher()
        var step = 0
        fun builder() {
            Content.apiTick(api.screen, step++)
            at(1_100) { builder() }
        }
        at(600) { builder() }
    }

    /** Codex finishes its review: a few lines, then Done (the inbox row moves groups). */
    fun codexFinishes() {
        Content.codexFinish(codex.screen, this) { personal.status(codex, AgentStatus.DONE) }
    }

    private fun submitted(herdr: DemoHerdr, text: String) {
        val agent = herdr.agents.first { it.paneId == herdr.focused }
        if (agent === claude && claude.status == AgentStatus.BLOCKED) {
            personal.status(claude, AgentStatus.WORKING)
            Content.claudeAnswered(claude.screen, text, this) { personal.status(claude, AgentStatus.DONE) }
        }
    }

    // --- the fakes ------------------------------------------------------------------------------------------

    inner class DemoAgent(
        val paneId: String, val tabId: String, val workspaceId: String, val name: String, val kind: String,
        var status: AgentStatus, val cwd: String, val title: String?, val screen: DemoScreen,
    ) {
        var seq = 1uL
        fun ffi() = HerdrAgent(
            paneId, tabId, workspaceId, name, kind, name, status, cwd, seq, "term_$paneId",
            AgentIdentity("term_$paneId", kind, name, null), title,
        )
    }

    inner class DemoHerdr(
        val name: String, private val workspaces: List<HerdrWorkspace>, private val tabs: List<HerdrTab>,
        val agents: List<DemoAgent>, focused: String,
    ) {
        @Volatile var focused = focused
            private set
        private var version = 1uL
        val listeners = CopyOnWriteArrayList<HerdrListener>()
        val screen get() = agents.first { it.paneId == focused }.screen

        fun view(): HerdrView = synchronized(lock) {
            HerdrView(
                version, focused, workspaces, tabs, agents.map { HerdrPane(it.paneId, it.kind, it.cwd) }, agents.map { it.ffi() },
                agents.first { it.paneId == focused }.tabId,
            )
        }

        private fun publish() {
            synchronized(lock) { version++ }
            val view = view()
            listeners.forEach { it.onHerdrStateChanged(HerdrState.Live(view)) }
        }

        fun focus(paneId: String) {
            if (agents.none { it.paneId == paneId }) throw HostException.PaneNotFound()
            focused = paneId
            publish()
            changed()
        }

        fun focusTab(tabId: String) = focus(agents.first { it.tabId == tabId }.paneId)

        fun status(agent: DemoAgent, status: AgentStatus) {
            synchronized(lock) {
                agent.status = status
                agent.seq++
            }
            publish()
        }
    }

    inner class DemoHost(
        val label: String, val address: String, val herdr: DemoHerdr, val tmux: Map<String, DemoScreen>,
        val tmuxSessions: List<TmuxSession>, val directories: List<String>, val shell: DemoScreen,
    ) {
        val capabilities = HostCapabilities("/usr/bin/tmux", "/home/dev/.local/bin/herdr", "/usr/bin/mosh-server",
            listOf(HerdrSessionInfo(herdr.name, true, true)))
    }

    private inner class DemoPort(private val host: DemoHost, private val listener: HostListener) : HostPort {
        @Volatile private var closed = false

        override fun state(): HostState = if (closed) HostState.Closed(CloseReason.Disconnected) else HostState.Connected(0u)
        override fun approveHostKey(fingerprint: String) = Unit
        override fun rejectHostKey() = Unit
        override fun disconnect() {
            closed = true
            listener.onHostStateChanged(state())
        }
        override fun close() = Unit

        override fun openTerminal(
            target: TerminalTarget, transport: TerminalTransport, columns: UShort, rows: UShort, moshBudgetMs: UInt?,
            listener: SessionListener,
        ): SessionInterface {
            val herdr = host.herdr
            val session = when (target) {
                is TerminalTarget.Herdr -> DemoSession({ herdr.screen }, transport, columns, rows) { submitted(herdr, it) }
                is TerminalTarget.Tmux -> DemoSession({ host.tmux.getValue(target.sessionName) }, transport, columns, rows) {}
                else -> DemoSession({ host.shell }, transport, columns, rows) {}
            }
            session.listener = listener
            sessions += session
            // SSH is there at once; mosh's bootstrap takes a moment, then the app swaps the terminal over to it.
            at(if (transport == TerminalTransport.MOSH) 700 else 40) { session.connect() }
            return session
        }

        override suspend fun capabilities() = host.capabilities
        override suspend fun moshServer() = host.capabilities.moshServer
        override suspend fun recentDirectories() = host.directories
        override suspend fun listTmuxSessions() = host.tmuxSessions

        override fun watchHerdr(session: String?, listener: HerdrListener): HerdrWatchInterface {
            val herdr = host.herdr
            herdr.listeners += listener
            at(80) { listener.onHerdrStateChanged(HerdrState.Live(herdr.view())) }
            return object : HerdrWatchInterface {
                override fun state(): HerdrState = HerdrState.Live(herdr.view())
                override fun stop() { herdr.listeners -= listener }
                fun close() { herdr.listeners -= listener }
            }
        }

        override suspend fun focusHerdrPane(session: String?, paneId: String) {
            delay(90)
            withContext(Dispatchers.Main) { host.herdr.focus(paneId) }
        }

        override suspend fun focusHerdrTab(session: String?, tabId: String) {
            delay(90)
            withContext(Dispatchers.Main) { host.herdr.focusTab(tabId) }
        }

        override suspend fun stopMoshServer(pid: UInt) = Unit
        override suspend fun scrollTarget(target: TerminalTarget, paneId: String?, scroll: TargetScroll, clientId: String?) = Unit
        override suspend fun navigate(target: TerminalTarget, paneId: String?, nav: TargetNav, clientId: String?) = Unit
        override suspend fun replyToPane(session: String?, paneId: String, agent: AgentIdentity, text: String) = ReplyRoute.PROMPTED
        override suspend fun permissionPrompt(session: String?, paneId: String, agent: AgentIdentity): PermissionPrompt? = null
        override suspend fun answerPermission(
            session: String?, paneId: String, agent: AgentIdentity, seq: ULong, answer: PermissionAnswer,
        ) = Unit
        override suspend fun uploadImage(bytes: ByteArray, extension: String) = "/home/dev/.cache/or2/images/or2-1.$extension"
        override suspend fun installHerdrIntegration(id: String) = Unit
        override suspend fun herdrIntegrations() =
            listOf("claude", "codex", "pi", "amp").map { HerdrIntegration(it, HerdrIntegrationState.CURRENT) }
    }

    /** One terminal: whatever its [source] screen shows, as full frames at the size the view asked for. */
    private inner class DemoSession(
        private val source: () -> DemoScreen, private val transport: TerminalTransport, columns: UShort, rows: UShort,
        private val submit: (String) -> Unit,
    ) : SessionInterface, AutoCloseable {
        var listener: SessionListener? = null
        @Volatile private var state: SessionState = SessionState.Connecting
        private var columns = columns.toInt()
        private var rows = rows.toInt()
        private var sequence = 0uL
        private var dirty = true
        private var closed = false

        fun connect() {
            if (state != SessionState.Connecting) return
            state = SessionState.Connected
            listener?.onStateChanged(state)
            changed()
        }

        fun changed() {
            synchronized(this) { dirty = true }
            if (state == SessionState.Connected) listener?.onFrameReady()
        }

        override fun takeFrame(): TerminalFrame? = synchronized(this) {
            if (!dirty || closed) return null
            dirty = false
            sequence++
            source().frame(columns, rows, sequence)
        }

        override fun requestFullFrame() = changed()

        override fun resize(columns: UShort, rows: UShort) {
            synchronized(this) {
                this.columns = columns.toInt()
                this.rows = rows.toInt()
            }
            changed()
        }

        override fun submitText(text: String) = at(0) { submit(text) }
        override fun sendText(text: String) = Unit
        override fun pasteText(text: String) = Unit
        override fun sendKey(input: KeyInput) = Unit
        override fun scroll(scroll: ViewportScroll) = Unit
        override fun mouseClick(column: UShort, row: UShort) = Unit

        override fun disconnect() {
            state = SessionState.Closed(CloseReason.Disconnected)
            listener?.onStateChanged(state)
        }

        override fun close() {
            synchronized(this) { closed = true }
            sessions -= this
        }

        override fun state() = state
        override fun transport() = transport
        override fun serverPid(): UInt? = null
        override fun clientId(): String? = null
        override fun roam() = Unit
    }
}

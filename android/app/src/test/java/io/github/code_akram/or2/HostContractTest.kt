package io.github.code_akram.or2

import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrListener
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostAddress
import io.github.code_akram.or2.ffi.HostConnectException
import io.github.code_akram.or2.ffi.HostConnectRequest
import io.github.code_akram.or2.ffi.HostConnection
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostListener
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.NavDirection
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TargetNav
import io.github.code_akram.or2.ffi.TerminalFrame
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import io.github.code_akram.or2.ffi.connectHost
import io.github.code_akram.or2.ffi.contractProbeHost
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.networkChanged
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * Records callbacks on whichever thread Rust calls from. JUnit assertion errors thrown inside a
 * callback would not reach the test, so callbacks only record.
 */
open class CallbackRecorder<T : Any> {
    val items = LinkedBlockingQueue<T>()
    val callbackThreads: MutableSet<Thread> = ConcurrentHashMap.newKeySet()
    private val active = AtomicInteger()

    @Volatile
    var overlapped = false
        private set

    /** Optional log shared across recorders: "tag:ItemName", appended as each item arrives. */
    @Volatile
    var timeline: MutableList<String>? = null

    @Volatile
    var timelineTag = ""

    protected fun record(item: T) {
        if (active.incrementAndGet() != 1) overlapped = true
        callbackThreads.add(Thread.currentThread())
        timeline?.add("$timelineTag:${item::class.simpleName}")
        items.add(item)
        active.decrementAndGet()
    }

    /** The next callback, which must be a [R]: this checks delivery order. */
    inline fun <reified R : T> await(): R {
        val item = items.poll(RecordingListener.TIMEOUT_SECONDS, TimeUnit.SECONDS)
        assertNotNull("timed out waiting for ${R::class.simpleName}", item)
        if (item !is R) fail("expected ${R::class.simpleName}, got $item")
        return item as R
    }

    fun assertQuiet() {
        assertNull(items.poll(RecordingListener.QUIET_MILLIS, TimeUnit.MILLISECONDS))
    }
}

class HostRecorder : CallbackRecorder<HostState>(), HostListener {
    override fun onHostStateChanged(state: HostState) = record(state)
}

class HerdrRecorder : CallbackRecorder<HerdrState>(), HerdrListener {
    override fun onHerdrStateChanged(state: HerdrState) = record(state)
}

/**
 * The host connection contract (FFI API 6) across the real FFI, driven by `contractProbeHost`:
 * a scripted host with fixed answers and no network.
 */
class HostContractTest {
    private val clientKey = generateEd25519Key("probe")

    private fun request(
        trusted: List<String> = emptyList(),
        addresses: List<HostAddress> = listOf(HostAddress("probe.invalid", 22u)),
        username: String = "akram",
        privateKey: ByteArray = clientKey.privateKey.copyOf(),
    ) = HostConnectRequest(addresses, username, privateKey, trusted)

    private fun TerminalFrame.rowText(index: Int) =
        changedRows.single { it.index.toInt() == index }.cells.joinToString("") { it.text }.trimEnd()

    /** A first-use probe host, approved and connected. */
    private fun connectedHost(recorder: HostRecorder = HostRecorder()): HostConnection {
        val host = contractProbeHost(request(), recorder)
        val prompt = recorder.await<HostState.AwaitingHostKeyDecision>()
        host.approveHostKey(prompt.presented.fingerprint)
        recorder.await<HostState.Authenticating>()
        assertEquals(HostState.Connected(0u), recorder.await<HostState.Connected>())
        return host
    }

    private fun openShell(
        host: HostConnection,
        target: TerminalTarget = TerminalTarget.Shell,
        columns: UShort = 60u,
    ): Pair<Session, RecordingListener> {
        val listener = RecordingListener()
        return host.openTerminal(target, TerminalTransport.SSH, columns, 5u, null, listener) to listener
    }

    @Test
    fun invalidRequestsAreRejectedSynchronouslyWithTypedErrors() {
        val listener = HostRecorder()
        val ok = HostAddress("h", 22u)
        assertThrows(HostConnectException.NoAddresses::class.java) {
            contractProbeHost(request(addresses = emptyList()), listener)
        }
        assertThrows(HostConnectException.TooManyAddresses::class.java) {
            contractProbeHost(request(addresses = List(9) { ok }), listener)
        }
        contractProbeHost(request(addresses = List(8) { ok }), HostRecorder()).use { it.disconnect() }
        val badHost = assertThrows(HostConnectException.InvalidAddress::class.java) {
            contractProbeHost(request(addresses = listOf(ok, HostAddress("a b", 22u))), listener)
        }
        assertEquals(1u, badHost.index)
        val badPort = assertThrows(HostConnectException.InvalidAddress::class.java) {
            contractProbeHost(request(addresses = listOf(HostAddress("h", 0u))), listener)
        }
        assertEquals(0u, badPort.index)
        assertThrows(HostConnectException.InvalidUsername::class.java) {
            contractProbeHost(request(username = ""), listener)
        }
        assertThrows(HostConnectException.InvalidPrivateKey::class.java) {
            contractProbeHost(request(privateKey = "junk".encodeToByteArray()), listener)
        }
        val badKey = assertThrows(HostConnectException.InvalidTrustedHostKey::class.java) {
            contractProbeHost(request(trusted = listOf(clientKey.publicKey.openssh, "not a key")), listener)
        }
        assertEquals(1u, badKey.index)
        // The production export validates the same way.
        assertThrows(HostConnectException.NoAddresses::class.java) {
            connectHost(request(addresses = emptyList()), listener)
        }
        listener.assertQuiet()
    }

    @Test
    fun connectHostReallyConnectsAndReportsEveryUnreachableAddressFromARustThread() {
        val listener = HostRecorder()
        val testThread = Thread.currentThread()
        // Loopback ports nothing listens on: the real driver races both and both are refused.
        val dead = List(2) { ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort } }
        val addresses = dead.map { HostAddress("127.0.0.1", it.toUShort()) }
        connectHost(request(addresses = addresses), listener).use { host ->
            val closed = listener.await<HostState.Closed>()
            val failure = (closed.reason as CloseReason.Failed).failure
            assertTrue("expected Unreachable, got $failure", failure is SessionFailure.Unreachable)
            val message = (failure as SessionFailure.Unreachable).message
            assertTrue(message, "address 0" in message && "address 1" in message)
            assertEquals(closed, host.state())
            // A closed host refuses everything, quietly.
            assertThrows(HostException.Closed::class.java) {
                host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 24u, null, RecordingListener())
            }
            assertThrows(HostException.Closed::class.java) { runBlocking { host.capabilities() } }
            assertThrows(HostException.Closed::class.java) { runBlocking { host.moshServer() } }
        }
        listener.assertQuiet()
        assertFalse(testThread in listener.callbackThreads)
        assertFalse(listener.overlapped)
    }

    @Test
    fun firstUseHostKeyThenQueriesTerminalsAndWatchesRoundTrip() {
        val recorder = HostRecorder()
        val testThread = Thread.currentThread()
        contractProbeHost(request(), recorder).use { host ->
            val prompt = recorder.await<HostState.AwaitingHostKeyDecision>()
            assertTrue(prompt.previouslyTrusted.isEmpty())
            assertEquals("ssh-ed25519", prompt.presented.algorithm)
            assertTrue(prompt.presented.fingerprint.startsWith("SHA256:"))
            assertEquals(prompt, host.state())

            // Before Connected everything but the host-key decision is refused.
            assertThrows(HostException.NotConnected::class.java) { runBlocking { host.capabilities() } }
            assertThrows(HostException.NotConnected::class.java) { runBlocking { host.moshServer() } }
            assertThrows(HostException.NotConnected::class.java) { runBlocking { host.listTmuxSessions() } }
            assertThrows(HostException.NotConnected::class.java) {
                host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 24u, null, RecordingListener())
            }
            assertThrows(HostException.NotConnected::class.java) { host.watchHerdr(null, HerdrRecorder()) }
            assertThrows(HostException.NotConnected::class.java) { runBlocking { host.focusHerdrPane(null, "w1:p1") } }
            assertThrows(HostException.HostKeyMismatch::class.java) {
                host.approveHostKey(clientKey.publicKey.fingerprint)
            }

            host.approveHostKey(prompt.presented.fingerprint)
            recorder.await<HostState.Authenticating>()
            assertEquals(HostState.Connected(0u), recorder.await<HostState.Connected>())
            assertEquals(HostState.Connected(0u), host.state())
            assertThrows(HostException.NoHostKeyPrompt::class.java) { host.rejectHostKey() }

            val capabilities = runBlocking { host.capabilities() }
            assertEquals("/usr/bin/tmux", capabilities.tmux)
            assertEquals("/home/probe/.local/bin/herdr", capabilities.herdr)
            assertEquals("/usr/bin/mosh-server", capabilities.moshServer)
            // API 14: the program probe's answer alone.
            assertEquals("/usr/bin/mosh-server", runBlocking { host.moshServer() })
            assertEquals("C.UTF-8", capabilities.utf8Locale)
            assertEquals(listOf("default", "or2-probe"), capabilities.herdrSessions.map { it.name })
            assertEquals(listOf(true, false), capabilities.herdrSessions.map { it.running })
            assertEquals(listOf(true, false), capabilities.herdrSessions.map { it.isDefault })
            // The default session is opened with a null name, never its listed name.
            val default = capabilities.herdrSessions.single { it.isDefault }
            val defaultWatch = HerdrRecorder()
            host.watchHerdr(null, defaultWatch).use {
                assertEquals(default.name, defaultWatch.await<HerdrState.Live>().view.workspaces[0].label)
            }

            val tmux = runBlocking { host.listTmuxSessions() }
            assertEquals(listOf("main", "build"), tmux.map { it.name })
            assertEquals(3u, tmux[0].windows)
            assertEquals(1u, tmux[0].attachedClients)
            assertEquals(1_700_000_000L, tmux[0].createdUnix)
            assertTrue(tmux[0].activityUnix > tmux[1].activityUnix)

            host.disconnect()
            assertEquals(CloseReason.Disconnected, recorder.await<HostState.Closed>().reason)
            recorder.assertQuiet()
        }
        assertFalse("callbacks never overlap", recorder.overlapped)
        assertFalse("callbacks never run on the caller's thread", testThread in recorder.callbackThreads)
    }

    @Test
    fun terminalsOpenAtConnectedWithoutHostKeyStatesAndEchoInput() {
        val host = connectedHost()
        val (shell, shellListener) = openShell(host, columns = 30u)
        // A channel session goes straight from Connecting to Connected.
        shellListener.awaitState<SessionState.Connected>()
        val first = shellListener.awaitFrame(shell)
        assertTrue(first.full)
        assertEquals(30.toUShort(), first.columns)
        assertEquals(5.toUShort(), first.rows)
        assertEquals("or2 contract probe shell", first.rowText(0))

        shell.sendText("é\n")
        assertEquals("text c3 a9 0d", shellListener.awaitFrame(shell).rowText(2))
        shell.resize(20u, 4u)
        val resized = shellListener.awaitFrame(shell)
        assertTrue(resized.full)
        assertEquals(4, resized.changedRows.size)

        // Several sessions share one host and are independent.
        val (tmux, tmuxListener) = openShell(host, TerminalTarget.Tmux("my work"))
        tmuxListener.awaitState<SessionState.Connected>()
        assertEquals("or2 contract probe tmux my work", tmuxListener.awaitFrame(tmux).rowText(0))
        val (herdr, herdrListener) = openShell(host, TerminalTarget.Herdr("work", "w1:p2"))
        herdrListener.awaitState<SessionState.Connected>()
        assertEquals("or2 contract probe herdr work w1:p2", herdrListener.awaitFrame(herdr).rowText(0))

        tmux.disconnect()
        assertEquals(CloseReason.Disconnected, tmuxListener.awaitState<SessionState.Closed>().reason)
        assertThrows(SessionException.Closed::class.java) { tmux.sendText("late") }
        shell.sendText("x")
        assertEquals("text 78", shellListener.awaitFrame(shell).rowText(2))
        assertEquals(HostState.Connected(0u), host.state())

        host.disconnect()
        assertFalse(shellListener.overlapped)
        host.close()
    }

    @Test
    fun moshProbeTerminalsReportHealthAndCountRoamsInTheEchoRow() {
        val host = connectedHost()
        val listener = RecordingListener()
        val mosh = host.openTerminal(TerminalTarget.Tmux("work"), TerminalTransport.MOSH, 60u, 5u, null, listener)
        val ssh = openShell(host)
        assertEquals(TerminalTransport.MOSH, mosh.transport())
        assertEquals(TerminalTransport.SSH, ssh.first.transport())
        listener.awaitState<SessionState.Connected>()
        assertEquals("or2 contract probe tmux work", listener.awaitFrame(mosh).rowText(0))
        // The SSH terminal's own first frame is signalled too; take it now, or the wait for its echo
        // below would find that old signal, take whatever has been published so far (on a loaded
        // machine, still only the first frame) and compare the wrong row.
        ssh.second.awaitState<SessionState.Connected>()
        assertEquals("or2 contract probe shell", ssh.second.awaitFrame(ssh.first).rowText(0))
        // The fixed sequence: healthy, stale (past the 5 s grey-out), recovered.
        val sequence = List(3) { listener.awaitHealth() }
        assertEquals(listOf(300uL, 6000uL, 400uL), sequence.map { it.sinceHeardMs })
        assertEquals(listOf(300uL, 9000uL, 400uL), sequence.map { it.sinceAckMs })

        mosh.roam()
        assertEquals("roams 1", listener.awaitFrame(mosh).rowText(2))
        mosh.sendText("x")
        assertEquals("text 78 | roams 1", listener.awaitFrame(mosh).rowText(2))
        // network_changed() roams every live mosh session and leaves SSH terminals alone.
        networkChanged()
        assertEquals("text 78 | roams 2", listener.awaitFrame(mosh).rowText(2))
        ssh.first.roam()
        ssh.first.sendText("y")
        assertEquals("text 79", ssh.second.awaitFrame(ssh.first).rowText(2))
        assertTrue(ssh.second.healths.isEmpty())

        host.disconnect()
        assertEquals(CloseReason.Disconnected, listener.awaitState<SessionState.Closed>().reason)
        // A closed session ignores roam and network_changed.
        mosh.roam()
        networkChanged()
        listener.assertNoMoreStates()
        host.close()
    }

    @Test
    fun invalidNamesAndDimensionsAreRejectedBeforeAnythingOpens() {
        val host = connectedHost()
        val listener = RecordingListener()
        fun open(target: TerminalTarget) = host.openTerminal(target, TerminalTransport.SSH, 80u, 24u, null, listener)
        for (name in listOf("", "a:b", "a.b", "a\\b", "a\nb", "x".repeat(129))) {
            assertThrows(name, HostException.InvalidName::class.java) { open(TerminalTarget.Tmux(name)) }
        }
        for (session in listOf("", "a b", "a:b", "é", "x".repeat(65))) {
            assertThrows(session, HostException.InvalidName::class.java) { open(TerminalTarget.Herdr(session, null)) }
            assertThrows(session, HostException.InvalidName::class.java) { host.watchHerdr(session, HerdrRecorder()) }
        }
        for (pane in listOf("", "a b", "a.b", "a/b", "x".repeat(129))) {
            assertThrows(pane, HostException.InvalidName::class.java) { open(TerminalTarget.Herdr(null, pane)) }
        }
        assertThrows(HostException.EmptyDimension::class.java) {
            host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 0u, 24u, null, listener)
        }
        assertThrows(HostException.EmptyDimension::class.java) {
            host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 0u, null, listener)
        }
        // Edge-valid names open.
        host.openTerminal(TerminalTarget.Tmux("x".repeat(128)), TerminalTransport.SSH, 80u, 24u, null, listener).disconnect()
        host.openTerminal(TerminalTarget.Herdr("a_b-1".repeat(12), "w1:p_2-3".repeat(16)), TerminalTransport.SSH, 80u, 24u, null, listener).disconnect()
        listener.awaitState<SessionState.Connected>()
        host.disconnect()
        host.close()
    }

    @Test
    fun herdrWatchGoesLiveUpdatesOnceAndClosesOnStop() {
        val host = connectedHost()
        val recorder = HerdrRecorder()
        val watch = host.watchHerdr("work", recorder)
        val first = recorder.await<HerdrState.Live>().view
        val update = recorder.await<HerdrState.Live>().view
        assertEquals(1uL, first.version)
        assertEquals(2uL, update.version)
        assertEquals(22u, first.protocol)
        assertEquals("work", first.workspaces[0].label)
        assertEquals("w1:p1", first.focusedPaneId)

        fun HerdrView.statuses() = agents.map { it.status }
        assertEquals(listOf(AgentStatus.BLOCKED, AgentStatus.WORKING, AgentStatus.IDLE), first.statuses())
        assertEquals(listOf(AgentStatus.WORKING, AgentStatus.WORKING, AgentStatus.IDLE), update.statuses())
        assertTrue(update.agents[0].stateChangeSeq > first.agents[0].stateChangeSeq)
        assertEquals(listOf("w1:p1", "w1:p2", "w2:p1"), first.agents.map { it.paneId })
        assertEquals(first.agents.map { it.paneId }, first.panes.map { it.paneId })
        assertEquals(AgentStatus.BLOCKED, first.workspaces[0].agentStatus)
        assertEquals(AgentStatus.WORKING, update.workspaces[0].agentStatus)
        assertEquals(update, (watch.state() as HerdrState.Live).view)

        watch.stop()
        assertEquals(HerdrState.Closed, recorder.await<HerdrState.Closed>())
        assertEquals(HerdrState.Closed, watch.state())
        watch.stop() // Idempotent.
        recorder.assertQuiet()
        assertFalse(recorder.overlapped)
        assertFalse(Thread.currentThread() in recorder.callbackThreads)

        // The default session when none is named; and a closed watch does not end the host.
        val defaultRecorder = HerdrRecorder()
        val defaultWatch = host.watchHerdr(null, defaultRecorder)
        assertEquals("default", defaultRecorder.await<HerdrState.Live>().view.workspaces[0].label)
        defaultWatch.close() // Dropping the object stops the watch.
        defaultRecorder.await<HerdrState.Live>()
        defaultRecorder.await<HerdrState.Closed>()
        assertEquals(HostState.Connected(0u), host.state())
        host.disconnect()
        host.close()
    }

    @Test
    fun focusingAHerdrPaneIsAwaitedReportsAVanishedPaneAndIsVisibleToLaterWatches() {
        val host = connectedHost()
        // A reused agent terminal is refocused first: success is awaited before the terminal is shown.
        runBlocking { host.focusHerdrPane("work", "w1:p2") }
        // A pane that has gone is its own error, not a generic command failure.
        assertThrows(HostException.PaneNotFound::class.java) { runBlocking { host.focusHerdrPane(null, "w9:p9") } }
        // Names are validated like terminal targets, before anything runs.
        for (session in listOf("", "a b", "a.b", "x".repeat(65))) {
            assertThrows(session, HostException.InvalidName::class.java) { runBlocking { host.focusHerdrPane(session, "w1:p1") } }
        }
        for (pane in listOf("", "w1 p1", "w1.p1", "p".repeat(129))) {
            assertThrows(pane, HostException.InvalidName::class.java) { runBlocking { host.focusHerdrPane(null, pane) } }
        }
        // The probe host keeps the focus it was given: a watch started afterwards sees it.
        val recorder = HerdrRecorder()
        val watch = host.watchHerdr(null, recorder)
        val view = recorder.await<HerdrState.Live>().view
        assertEquals("w1:p2", view.focusedPaneId)
        assertEquals(listOf(false, true, false), view.agents.map { it.focused })
        watch.stop()
        recorder.await<HerdrState.Live>()
        recorder.await<HerdrState.Closed>()
        // The last successful focus stands after a refused one; the host stays usable.
        runBlocking { host.focusHerdrPane(null, "w2:p1") }
        assertEquals(HostState.Connected(0u), host.state())
        host.disconnect()
        host.close()
    }

    @Test
    fun navigationCrossesTheFfiValidatesNamesAndAShellDoesNothing() {
        val host = connectedHost()
        val tmux = TerminalTarget.Tmux("main")
        runBlocking {
            for (nav in listOf(
                TargetNav.NextWindow, TargetNav.PreviousWindow, TargetNav.NextSession, TargetNav.PreviousSession,
                TargetNav.Pane(NavDirection.LEFT), TargetNav.Pane(NavDirection.RIGHT),
                TargetNav.Pane(NavDirection.UP), TargetNav.Pane(NavDirection.DOWN),
            )) {
                host.navigate(tmux, null, nav)
                host.navigate(TerminalTarget.Herdr("work", null), "w1:p2", nav)
            }
            // A shell has nothing to move: answered at once, whatever the move.
            host.navigate(TerminalTarget.Shell, null, TargetNav.NextSession)
        }
        // A herdr pane that has gone, and names validated like terminal targets.
        assertThrows(HostException.PaneNotFound::class.java) {
            runBlocking { host.navigate(TerminalTarget.Herdr(null, null), "w9:p9", TargetNav.Pane(NavDirection.LEFT)) }
        }
        assertThrows(HostException.InvalidName::class.java) {
            runBlocking { host.navigate(TerminalTarget.Tmux("a:b"), null, TargetNav.NextWindow) }
        }
        assertThrows(HostException.InvalidName::class.java) {
            runBlocking { host.navigate(TerminalTarget.Herdr(null, null), "w1 p1", TargetNav.NextWindow) }
        }
        assertEquals(HostState.Connected(0u), host.state())
        host.disconnect()
        host.close()
    }

    @Test
    fun disconnectingTheHostClosesItsTerminalsAndWatchesFirst() {
        // One timeline for all three sources, so the order itself is asserted.
        val timeline: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())
        val recorder = HostRecorder().apply { this.timeline = timeline; timelineTag = "host" }
        val host = connectedHost(recorder)
        val (session, sessionListener) = openShell(host)
        sessionListener.timeline = timeline
        sessionListener.awaitState<SessionState.Connected>()
        val herdrRecorder = HerdrRecorder().apply { this.timeline = timeline; timelineTag = "herdr" }
        val watch = host.watchHerdr(null, herdrRecorder)
        herdrRecorder.await<HerdrState.Live>()
        herdrRecorder.await<HerdrState.Live>()

        host.disconnect()
        assertEquals(CloseReason.Disconnected, sessionListener.awaitState<SessionState.Closed>().reason)
        assertEquals(HerdrState.Closed, herdrRecorder.await<HerdrState.Closed>())
        assertEquals(CloseReason.Disconnected, recorder.await<HostState.Closed>().reason)
        val closes = synchronized(timeline) { timeline.filter { it.endsWith(":Closed") } }
        assertEquals("host:Closed", closes.last())
        assertEquals(setOf("session:Closed", "herdr:Closed"), closes.dropLast(1).toSet())
        assertEquals(3, closes.size)
        assertEquals(HostState.Closed(CloseReason.Disconnected), host.state())
        assertEquals(HerdrState.Closed, watch.state())
        assertThrows(SessionException.Closed::class.java) { session.sendText("late") }

        assertThrows(HostException.Closed::class.java) { runBlocking { host.capabilities() } }
        assertThrows(HostException.Closed::class.java) { runBlocking { host.listTmuxSessions() } }
        assertThrows(HostException.Closed::class.java) {
            host.openTerminal(TerminalTarget.Shell, TerminalTransport.SSH, 80u, 24u, null, RecordingListener())
        }
        assertThrows(HostException.Closed::class.java) { host.watchHerdr(null, HerdrRecorder()) }
        assertThrows(HostException.Closed::class.java) { runBlocking { host.focusHerdrPane(null, "w1:p1") } }
        assertThrows(HostException.Closed::class.java) { host.rejectHostKey() }
        host.disconnect() // Idempotent.
        recorder.assertQuiet()
        sessionListener.assertNoMoreStates()
        host.close()
    }

    @Test
    fun trustedHostKeySkipsThePromptAndAChangedKeyCanBeRejected() {
        val learned = HostRecorder()
        val firstUse = contractProbeHost(request(), learned)
        val presented = learned.await<HostState.AwaitingHostKeyDecision>().presented
        firstUse.disconnect()
        learned.await<HostState.Closed>()

        val trusted = HostRecorder()
        contractProbeHost(request(trusted = listOf(clientKey.publicKey.openssh, presented.openssh)), trusted).use { host ->
            trusted.await<HostState.Authenticating>()
            assertEquals(HostState.Connected(0u), trusted.await<HostState.Connected>())
            host.disconnect()
            trusted.await<HostState.Closed>()
        }

        val previous = generateEd25519Key("old host key")
        val changed = HostRecorder()
        contractProbeHost(request(trusted = listOf(previous.publicKey.openssh)), changed).use { host ->
            val prompt = changed.await<HostState.AwaitingHostKeyDecision>()
            assertEquals(listOf(previous.publicKey.fingerprint), prompt.previouslyTrusted.map { it.fingerprint })
            host.rejectHostKey()
            val closed = changed.await<HostState.Closed>()
            assertEquals(CloseReason.Failed(SessionFailure.HostKeyRejected), closed.reason)
            assertThrows(HostException.Closed::class.java) { host.approveHostKey(prompt.presented.fingerprint) }
        }
    }

    @Test
    fun closingTheHostObjectDisconnects() {
        val recorder = HostRecorder()
        val host = contractProbeHost(request(), recorder)
        recorder.await<HostState.AwaitingHostKeyDecision>()
        host.close()
        assertEquals(CloseReason.Disconnected, recorder.await<HostState.Closed>().reason)
    }
}

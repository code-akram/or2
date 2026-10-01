package io.github.code_akram.or2.gallery

import android.graphics.Color as AndroidColor
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.about.AboutRoute
import io.github.code_akram.or2.about.LicensesRoute
import io.github.code_akram.or2.app.AppScaffold
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HerdrTab
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HerdrWorkspace
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.LinkHealth
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.generateEd25519Key
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.pair.AddHostSheet
import io.github.code_akram.or2.pair.CameraAccess
import io.github.code_akram.or2.pair.KeyChoice
import io.github.code_akram.or2.pair.PairInstallKeyScreen
import io.github.code_akram.or2.pair.PairProgressScreen
import io.github.code_akram.or2.pair.PairReview
import io.github.code_akram.or2.pair.PairReviewScreen
import io.github.code_akram.or2.pair.PairScanScreen
import io.github.code_akram.or2.home.HomeScreen
import io.github.code_akram.or2.home.HomeSession
import io.github.code_akram.or2.home.HostCard
import io.github.code_akram.or2.home.hostCardStatus
import io.github.code_akram.or2.host.HostScreen
import io.github.code_akram.or2.host.HostTerminalItem
import io.github.code_akram.or2.host.PickerTab
import io.github.code_akram.or2.host.SessionPickerSheet
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.hosts.HostFormScreen
import io.github.code_akram.or2.inbox.InboxHostRow
import io.github.code_akram.or2.inbox.InboxScreen
import io.github.code_akram.or2.inbox.InboxSource
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.buildInbox
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.session.TerminalCard
import io.github.code_akram.or2.terminal.TerminalChromeState
import io.github.code_akram.or2.terminal.TerminalGrid
import io.github.code_akram.or2.terminal.TerminalGridPreview
import io.github.code_akram.or2.terminal.TerminalScreen
import io.github.code_akram.or2.terminal.TerminalView
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Type
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.receiveAsFlow

/**
 * Debug-only UI gallery: every screen and key state rendered with fake data, no network, no
 * biometrics, no database. `am start -n io.github.code_akram.or2/.gallery.UiGalleryActivity
 * --es screen <name>` opens one directly (see [screens]); without an extra it lists them.
 */
class UiGalleryActivity : ComponentActivity() {
    private var probe: Session? = null
    private val probeState = MutableStateFlow<SessionState>(SessionState.Connecting)
    private val probeFrames = Channel<Unit>(Channel.CONFLATED)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(AndroidColor.TRANSPARENT),
        )
        val initial = intent.getStringExtra("screen")
        setContent {
            var screen by remember { mutableStateOf(initial) }
            BackHandler(enabled = screen != null && initial == null) { screen = null }
            val current = screen
            AppScaffold(fullScreen = current != null && current.startsWith("terminal")) {
                if (current == null) Menu { screen = it } else Screen(current)
            }
        }
    }

    @Composable
    private fun Menu(open: (String) -> Unit) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp).testTag("gallery-menu")) {
            Text("UI gallery", style = Or2Type.ScreenTitle, color = Or2Colors.Text)
            screens.forEach { name ->
                Text(name, style = Or2Type.RowLabel, color = Or2Colors.Accent,
                    modifier = Modifier.clickable { open(name) }.padding(vertical = 12.dp).testTag("gallery:$name"))
            }
        }
    }

    @Composable
    private fun Screen(name: String) {
        when (name) {
            "home" -> Home(HomeVariant.Sessions)
            "home-empty" -> Home(HomeVariant.Empty)
            "host-cards" -> Home(HomeVariant.CardStates)
            "inbox" -> Inbox(empty = false)
            "inbox-empty" -> Inbox(empty = true)
            "picker-herdr" -> Picker(PickerTab.HERDR)
            "picker-tmux" -> Picker(PickerTab.TMUX)
            "picker-recent" -> Picker(PickerTab.RECENT)
            "host-form" -> HostFormScreen(null, listOf(key1, key2), false, {}, {})
            "host-form-edit" -> HostFormScreen(multiHost, listOf(key1, key2), false, {}, {})
            "add-host" -> AddHostSheet(easyPair = {}, manual = {}, dismiss = {})
            "pair-scan" -> PairScanScreen(null, CameraAccess(granted = false, denied = false) {}, {}, back = {})
            "pair-scan-denied" -> PairScanScreen("That is not an or2 pairing code. Run or2-pair on the host and scan the code it prints.",
                CameraAccess(granted = false, denied = true) {}, {}, back = {})
            "pair-review" -> pairReview(listOf(key1, key2), KeyChoice.Existing("k1"), error = null)
            "pair-review-new" -> pairReview(emptyList(), KeyChoice.New, error = "The host declined the key, so nothing was changed. Run or2-pair again to retry.")
            "pair-progress" -> PairProgressScreen("dev", key1.fingerprint, cancel = {})
            "pair-install" -> PairInstallKeyScreen("workstation", key1.openssh, key1.fingerprint, done = {})
            "keys" -> KeysScreen(listOf(key1, key2), false, { _, _ -> }, { _, _, _ -> }, {})
            "keys-empty" -> KeysScreen(emptyList(), false, { _, _ -> }, { _, _, _ -> }, {})
            "about" -> AboutRoute(back = {}, openLicenses = {})
            "licenses" -> LicensesRoute(back = {})
            "hostkey-first" -> HostKey(changed = false)
            "hostkey-changed" -> HostKey(changed = true)
            "terminal" -> Terminal(pad = false, composer = false)
            "terminal-arrowpad" -> Terminal(pad = true, composer = false)
            "terminal-composer" -> Terminal(pad = false, composer = true)
            else -> Text("Unknown screen: $name", color = Or2Colors.Danger)
        }
    }

    // --- data ----------------------------------------------------------------------------

    /** A review from a made-up pairing code (the real parser, so the fingerprint is the key's own). */
    @Composable
    private fun pairReview(keys: List<KeyRecord>, choice: KeyChoice, error: String?) {
        val offer = remember {
            parsePairPayload(
                "or2-pair:1?name=workstation&user=dev&port=22&a=100.101.102.103&a=192.168.1.20&a=workstation.local" +
                    "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7" +
                    "&pair=192.168.1.20:41234&otp=AAAQEAYEAUDAOCAJBIFQYDIOB4",
            )
        }
        PairReviewScreen(PairReview(offer, offer.name, offer.username, choice, error), keys, edit = { _, _, _ -> }, submit = {}, back = {})
    }

    private val key1 = KeyRecord("k1", "Phone key", "ssh-ed25519", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPhoneKeyExampleExampleExampleExample0123 phone",
        "SHA256:7vK2mQ9xRpL3aTn0sWZ4cEdHfY8uJbNqXoGiVtB1MkA", "phone", byteArrayOf(), byteArrayOf())
    private val key2 = KeyRecord("k2", "Work laptop", "ssh-ed25519", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIWorkKeyExampleExampleExampleExample4567 work",
        "SHA256:Q1xW8eZyT5nUo2Hd9bFgKj3RmVaC6lSiPtYvN0DcEh4", "work", byteArrayOf(), byteArrayOf())

    private fun host(id: Long, label: String, user: String = "dev", address: String = "$label.invalid", port: Int = 22, key: String? = "k1") =
        Host(HostRecord(id, label, user, key, true), listOf(HostEndpoint(address, port)))

    private val multiHost = Host(
        HostRecord(3, "workstation", "dev", "k1", true),
        listOf(HostEndpoint("workstation.invalid", 22), HostEndpoint("198.51.100.7", 2222)),
    )

    private fun agent(pane: String, status: AgentStatus, name: String, cwd: String, tab: String = "w1:t1", workspace: String = "w1") =
        HerdrAgent(pane, tab, workspace, name, "claude", name, status, cwd, null, false, 1uL)

    private fun view(vararg agents: HerdrAgent) = HerdrView(
        1uL, 22u, null,
        listOf(HerdrWorkspace("w1", 1u, "or2", true, AgentStatus.BLOCKED), HerdrWorkspace("w2", 2u, "docs", false, AgentStatus.WORKING)),
        listOf(HerdrTab("w1:t1", "w1", 1u, "ui-polish", true, AgentStatus.BLOCKED), HerdrTab("w2:t1", "w2", 1u, "readme", false, AgentStatus.WORKING)),
        emptyList(), agents.toList(),
    )

    private val caps = HostCapabilities("/usr/bin/tmux", "/home/dev/.local/bin/herdr", null, "C.UTF-8", listOf(
        HerdrSessionInfo("personal", true, true), HerdrSessionInfo("work", true, false), HerdrSessionInfo("archive", false, false)))
    private val tmux = TmuxList.Loaded(listOf(
        TmuxSession("main", 3u, 1u, 0L, 30L), TmuxSession("build", 1u, 0u, 0L, 20L), TmuxSession("scratch", 2u, 0u, 0L, 10L)))

    private fun demoGrid(columns: Int = 55, rows: Int = 60) = TerminalGrid().also { check(it.apply(terminalDemoFrame(columns, rows))) }

    // --- screens -------------------------------------------------------------------------

    private enum class HomeVariant { Sessions, Empty, CardStates }

    @Composable
    private fun Home(variant: HomeVariant) {
        val grid = remember { demoGrid() }
        val sessions = if (variant != HomeVariant.Sessions) emptyList() else listOf(
            HomeSession(1, "workstation", "tmux main", "~/code/or2", Transport.SSH) { m -> TerminalGridPreview(grid, 0, m) },
            HomeSession(2, "build-box", "herdr personal", "~/code/herdr", Transport.MOSH) { m -> TerminalGridPreview(grid, 0, m) },
        )
        val hosts = when (variant) {
            HomeVariant.Empty -> emptyList()
            HomeVariant.Sessions -> listOf(
                card(host(1, "workstation"), HostState.Connected(0u), blocked = 1),
                card(host(2, "build-box", address = "198.51.100.7"), HostState.Connected(0u)),
                card(host(3, "nas"), null),
            )
            HomeVariant.CardStates -> listOf(
                card(host(1, "workstation"), HostState.Connected(0u), blocked = 1),
                card(host(2, "build-box"), HostState.Connected(0u)),
                card(host(3, "unlocking"), null, unlocking = true),
                card(host(4, "checking"), HostState.Connecting),
                card(host(5, "authenticating"), HostState.Authenticating),
                card(host(6, "lab"), HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))),
                card(host(7, "nas"), null),
            )
        }
        HomeScreen(
            sessions, hosts, keyCount = if (variant == HomeVariant.Empty) 0 else 2,
            blocked = if (variant == HomeVariant.Empty) 0 else 1, working = if (variant == HomeVariant.Empty) 0 else 2,
            canConnectAll = false, busy = false,
            openSession = {}, openHost = {}, addHost = {}, editHost = {}, connectHost = {}, disconnectHost = {}, deleteHost = {},
            openInbox = {}, openKeys = {}, connectAll = {},
        )
    }

    private fun card(host: Host, state: HostState?, blocked: Int = 0, unlocking: Boolean = false) =
        HostCard(host, hostCardStatus(state, unlocking, blocked), linkStatus(state))

    @Composable
    private fun Inbox(empty: Boolean) {
        val one = host(1, "workstation")
        val two = host(2, "build-box", address = "198.51.100.7")
        val state = if (empty) {
            InboxState(listOf(InboxHostRow(one, LinkStatus.CONNECTED, "Connected", "No running herdr sessions", 0)), emptyList())
        } else {
            val groups = buildInbox(listOf(
                InboxSource(1, "workstation", null, "personal", view(
                    agent("w1:p1", AgentStatus.BLOCKED, "Claude Code", "~/code/or2"),
                    agent("w1:p2", AgentStatus.WORKING, "Codex", "~/code/herdr"),
                    agent("w2:p1", AgentStatus.DONE, "Claude Code", "~/code/docs", tab = "w2:t1", workspace = "w2"),
                    agent("w2:p2", AgentStatus.WORKING, "Amp", "~/code/docs", tab = "w2:t1", workspace = "w2"),
                )),
                InboxSource(2, "build-box", "work", "work", view(agent("w1:p9", AgentStatus.IDLE, "Claude Code", "~/src/build"))),
            ))
            InboxState(listOf(
                InboxHostRow(one, LinkStatus.CONNECTED, "Connected", null, 4),
                InboxHostRow(two, LinkStatus.CONNECTED, "Connected", "herdr is unavailable: the session's socket cannot be opened (is it owned by another user?)", 1),
                InboxHostRow(host(3, "nas"), LinkStatus.FAILED, "Authentication rejected. Check the username and public-key authorization.", null, 0),
            ), groups)
        }
        InboxScreen(state, busy = false, connectAll = {}, connect = {}, openHost = {}, openAgent = {})
    }

    @Composable
    private fun Picker(tab: PickerTab) {
        Box(Modifier.fillMaxSize()) {
            HostScreen(host(1, "workstation"), HostState.Connected(0u), caps, null, tmux, false, {}, {}, {}, {}, {}, {}, {}, {},
                pickerOffered = true)
            SessionPickerSheet(
                caps, null, tmux,
                recent = listOf(HostTerminalItem(1, "tmux main", false), HostTerminalItem(2, "herdr personal", false), HostTerminalItem(3, "shell", true)),
                openShell = {}, openTmux = {}, openHerdr = {}, resume = {}, refresh = {}, dismiss = {}, initialTab = tab,
            )
        }
    }

    @Composable
    private fun HostKey(changed: Boolean) {
        val presented = PublicKeyInfo("ssh-ed25519", "k", "SHA256:3Fq8vXk0mT2yLdN7pRzA1sWbHcE9uJgOiVnY5tB4QeM", "")
        val old = listOf(PublicKeyInfo("ssh-ed25519", "o", "SHA256:Zc9bN1xQ4mWuT7yLdKp2sRfHaE8vJgOiVnY3tB0QeMA", ""),
            PublicKeyInfo("ssh-rsa", "r", "SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f", ""))
        HostScreen(host(1, "workstation"), HostState.AwaitingHostKeyDecision(presented, if (changed) old else emptyList()), null, null,
            TmuxList.Loading, false, {}, {}, {}, {}, {}, {}, {}, {})
    }

    @Composable
    private fun Terminal(pad: Boolean, composer: Boolean) {
        val session = remember { startProbe() }
        val state by probeState.collectAsStateWithLifecycle()
        // The composer opens with a message typed, so the caret, the focus ring and the lit send button show.
        val chrome = remember { TerminalChromeState(padOpen = pad, composerOpen = composer, composerText = if (composer) "yes, go ahead" else "") }
        LaunchedEffect(Unit) {
            // Show the demo frame once the terminal view has its geometry (the probe's first frame
            // arrives first), and again whenever a resize (the keyboard) makes the probe redraw its own.
            var shown: Pair<Int, Int>? = null
            while (true) {
                delay(100)
                val view = findTerminal(window.decorView) ?: continue
                if (view.grid.hasGrid && view.width > 0 && state == SessionState.Connected) {
                    val size = view.grid.columns to view.grid.rows.size
                    val probeText = view.grid.rows.firstOrNull()?.cells?.joinToString("") { it.text }.orEmpty()
                    if (size != shown || probeText.startsWith("or2 contract probe")) {
                        view.clearSelection()
                        view.grid.apply(terminalDemoFrame(size.first, size.second, view.grid.sequence + 1u))
                        view.invalidate()
                        shown = size
                    }
                }
            }
        }
        TerminalCard("workstation: tmux main", Transport.SSH, SessionState.Connected, minimise = {}, openSwitcher = {}, endSession = {}) {
            TerminalScreen(session, probeState, probeFrames.receiveAsFlow(), Modifier.weight(1f),
                composerHint = "Message workstation…", chrome = chrome)
        }
    }

    private fun startProbe(): Session {
        probe?.let { return it }
        val key = generateEd25519Key("gallery")
        try {
            return contractProbeSession(
                ConnectRequest("probe.invalid", 22u, "probe", key.privateKey, emptyList(), 40u, 12u),
                object : SessionListener {
                    override fun onStateChanged(state: SessionState) {
                        probeState.value = state
                        if (state is SessionState.AwaitingHostKeyDecision) runOnUiThread { probe?.approveHostKey(state.presented.fingerprint) }
                    }
                    override fun onFrameReady() { probeFrames.trySend(Unit) }
                    override fun onLinkHealth(health: LinkHealth) = Unit
                },
            ).also { probe = it }
        } finally {
            key.privateKey.fill(0)
        }
    }

    private fun findTerminal(view: View): TerminalView? {
        if (view is TerminalView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) findTerminal(view.getChildAt(index))?.let { return it }
        return null
    }

    override fun onDestroy() {
        probe?.let {
            it.disconnect()
            it.close()
        }
        probeFrames.close()
        super.onDestroy()
    }

    companion object {
        val screens = listOf(
            "home", "home-empty", "host-cards", "inbox", "inbox-empty", "picker-herdr", "picker-tmux", "picker-recent",
            "host-form", "host-form-edit", "keys", "keys-empty", "about", "licenses", "hostkey-first", "hostkey-changed",
            "add-host", "pair-scan", "pair-scan-denied", "pair-review", "pair-review-new", "pair-progress", "pair-install",
            "terminal", "terminal-arrowpad", "terminal-composer",
        )
    }
}

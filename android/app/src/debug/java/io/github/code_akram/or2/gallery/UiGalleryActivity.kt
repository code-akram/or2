package io.github.code_akram.or2.gallery

import android.graphics.Color as AndroidColor
import android.os.Bundle
import android.view.View
import android.view.MotionEvent
import android.os.SystemClock
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
import androidx.compose.runtime.rememberCoroutineScope
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
import io.github.code_akram.or2.ffi.HerdrAgent
import io.github.code_akram.or2.ffi.HerdrIntegrationState
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
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TmuxSession
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.pair.AddHostSheet
import io.github.code_akram.or2.pair.CameraAccess
import io.github.code_akram.or2.pair.KeepAliveScreen
import io.github.code_akram.or2.pair.KeyChoice
import io.github.code_akram.or2.pair.PairInstallKeyScreen
import io.github.code_akram.or2.pair.PairProgressScreen
import io.github.code_akram.or2.pair.PairReview
import io.github.code_akram.or2.pair.PairReviewScreen
import io.github.code_akram.or2.pair.PairScanScreen
import io.github.code_akram.or2.paste.ImagePaste
import io.github.code_akram.or2.paste.ShareTarget
import io.github.code_akram.or2.paste.SharePickerSheet
import io.github.code_akram.or2.paste.UploadState
import io.github.code_akram.or2.paste.uploadNotice
import io.github.code_akram.or2.home.HomeScreen
import io.github.code_akram.or2.home.HomeSession
import io.github.code_akram.or2.home.HostCard
import io.github.code_akram.or2.home.hostCardStatus
import io.github.code_akram.or2.host.GateAction
import io.github.code_akram.or2.host.OpenSessions
import io.github.code_akram.or2.host.PickerGate
import io.github.code_akram.or2.host.PickerTab
import io.github.code_akram.or2.host.SessionPickerSheet
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.hosts.HostFormScreen
import io.github.code_akram.or2.inbox.InboxHostRow
import io.github.code_akram.or2.inbox.EnableReplyDialog
import io.github.code_akram.or2.inbox.InboxScreen
import io.github.code_akram.or2.notify.EnableReplyRequest
import io.github.code_akram.or2.inbox.InboxSource
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.buildInbox
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.session.CloseShellDialog
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.TerminalCard
import io.github.code_akram.or2.session.TerminalItem
import io.github.code_akram.or2.session.TerminalsSheet
import io.github.code_akram.or2.session.SpacesSheet
import io.github.code_akram.or2.terminal.TargetScroller
import io.github.code_akram.or2.terminal.TerminalChromeState
import io.github.code_akram.or2.terminal.TerminalGrid
import io.github.code_akram.or2.terminal.TerminalGridPreview
import io.github.code_akram.or2.terminal.TerminalScreen
import io.github.code_akram.or2.terminal.TerminalView
import io.github.code_akram.or2.terminal.Transport
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Type
import androidx.compose.ui.platform.LocalContext
import io.github.code_akram.or2.about.APP_LICENSE
import io.github.code_akram.or2.about.LicenseData
import io.github.code_akram.or2.about.LicenseText
import io.github.code_akram.or2.about.LicenseTextPage
import io.github.code_akram.or2.about.readAsset
import io.github.code_akram.or2.app.SettingsScreen
import io.github.code_akram.or2.home.HostOptionsSheet
import io.github.code_akram.or2.keys.PublicKeySheet
import io.github.code_akram.or2.terminal.ShortcutsSheet
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.receiveAsFlow

/** A made-up pairing code: the contract's own example, whose check character is valid. */
private const val GALLERY_PAIR_CODE = "7KQ4-M2XD-9PTM"

/**
 * Debug-only UI gallery: every screen, sheet and key state rendered with fake data, no network, no
 * biometrics, no database. `am start -n io.github.code_akram.or2/.gallery.UiGalleryActivity
 * --es screen <name>` opens one directly (see [screens]); without an extra it lists them. Any name
 * with the [SCROLLED] suffix shows that screen with its content dragged up under the top bar.
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
            AppScaffold(fullScreen = current != null && (current.startsWith("terminal") || current == "spaces")) {
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
        if (name.endsWith(SCROLLED)) {
            // Any screen, its content scrolled up under the fixed top bar by a real drag (the scroll edge shows).
            Screen(name.removeSuffix(SCROLLED))
            LaunchedEffect(name) {
                delay(1_000)
                dragUp(window.decorView)
            }
            return
        }
        when (name) {
            "home" -> Home(HomeVariant.Sessions)
            "home-empty" -> Home(HomeVariant.Empty)
            "home-notices" -> Home(HomeVariant.Notices)
            "host-cards" -> Home(HomeVariant.CardStates)
            "home-picker" -> HomePicker(HomeVariant.Sessions, gate = null)
            "home-picker-connecting" -> HomePicker(HomeVariant.Connecting, PickerGate.Connecting("nas", "Checking server…", spinning = true))
            "home-picker-failed" -> HomePicker(HomeVariant.Failed, PickerGate.Stopped("nas",
                "Authentication rejected. Check the username and public-key authorization.", failed = true, detail = null,
                action = GateAction.RETRY, enabled = true))
            "inbox" -> Inbox(empty = false)
            "inbox-empty" -> Inbox(empty = true)
            "inbox-enable-reply" -> Box(Modifier.fillMaxSize()) {
                Inbox(empty = false)
                EnableReplyDialog(EnableReplyRequest(2, "build-box", "pi", "pi"), enable = {}, dismiss = {})
            }
            "picker-herdr" -> HomePicker(HomeVariant.Sessions, gate = null, tab = PickerTab.HERDR)
            "picker-tmux" -> HomePicker(HomeVariant.Sessions, gate = null, tab = PickerTab.TMUX)
            "picker-udp" -> HomePicker(HomeVariant.Sessions, gate = null, tab = PickerTab.TMUX, udpBlocked = true)
            "picker-many" -> HomePicker(HomeVariant.Sessions, gate = null, tab = PickerTab.HERDR, many = true)
            "home-picker-many" -> HomePicker(HomeVariant.Sessions, gate = null, many = true)
            "home-options" -> Box(Modifier.fillMaxSize()) {
                Home(HomeVariant.Sessions)
                HostOptionsSheet(card(host(1, "workstation"), HostState.Connected(0u)), busy = false, {}, {}, {}, {}, {})
            }
            "home-close-shell" -> Box(Modifier.fillMaxSize()) {
                Home(HomeVariant.Sessions)
                CloseShellDialog(close = {}, dismiss = {})
            }
            "spaces" -> Box(Modifier.fillMaxSize()) {
                Terminal(host = "build-box", target = "herdr work", transport = Transport.MOSH)
                SpacesSheet("work", spacesView, focus = {}, dismiss = {})
            }
            "terminals" -> Box(Modifier.fillMaxSize()) {
                Terminal(target = "tmux main", transport = Transport.MOSH)
                TerminalsSheet(
                    listOf(
                        TerminalItem(1, 1, "workstation", "tmux main", closed = false),
                        TerminalItem(2, 1, "workstation", "shell", closed = true),
                        TerminalItem(3, 2, "build-box", "herdr personal", closed = false),
                    ),
                    currentId = 1, select = {}, close = {}, copyScreen = {}, shortcuts = {}, dismiss = {},
                )
            }
            "settings" -> SettingsScreen(agentAlerts = true, setAgentAlerts = {}, copyFromHost = true, setCopyFromHost = {}, back = {})
            "shortcuts" -> ShortcutsSheet(dismiss = {})
            "host-form" -> HostFormScreen(null, listOf(key1, key2), false, {}, {}, createKey = { _, _ -> key1 }, deviceLabel = "Pixel")
            "host-form-new-key" -> HostFormScreen(null, emptyList(), false, {}, {}, createKey = { _, _ -> key1 }, deviceLabel = "Pixel")
            "host-form-edit" -> HostFormScreen(multiHost, listOf(key1, key2), false, {}, {}, createKey = { _, _ -> key1 }, deviceLabel = "Pixel",
                delete = {})
            "add-host" -> AddHostSheet(easyPair = {}, manual = {}, dismiss = {})
            "pair-scan" -> PairScanScreen(GALLERY_PAIR_CODE, null, CameraAccess(granted = false, denied = false) {}, {}, back = {})
            "pair-scan-denied" -> PairScanScreen(GALLERY_PAIR_CODE, "That is not an or2 pairing code. Run or2-pair on the host and scan the code it prints.",
                CameraAccess(granted = false, denied = true) {}, {}, back = {})
            "pair-review" -> pairReview(listOf(key1, key2), KeyChoice.Existing("k1"), error = null)
            "pair-review-new" -> pairReview(emptyList(), KeyChoice.New, error = "Couldn't reach workstation on port 22. Pairing uses the same SSH port as connecting: the phone must reach it (same network, ZeroTier or Tailscale, or a public address).")
            "pair-progress" -> PairProgressScreen("workstation", cancel = {})
            "pair-install" -> PairInstallKeyScreen("workstation", key1.openssh, key1.fingerprint, done = {})
            "keepalive" -> KeepAliveScreen(waiting = false, allow = {}, notNow = {})
            "keepalive-waiting" -> KeepAliveScreen(waiting = true, allow = {}, notNow = {})
            "keys" -> KeysScreen(listOf(key1, key2), false, { _, _ -> }, { _, _, _ -> }, {})
            "keys-empty" -> KeysScreen(emptyList(), false, { _, _ -> }, { _, _, _ -> }, {})
            "key-sheet" -> Box(Modifier.fillMaxSize()) {
                KeysScreen(listOf(key1, key2), false, { _, _ -> }, { _, _, _ -> }, {})
                PublicKeySheet(key1, busy = false, delete = {}, dismiss = {})
            }
            "about" -> AboutRoute(back = {}, openLicenses = {})
            "licenses" -> LicensesRoute(back = {})
            "license-text" -> {
                val context = LocalContext.current
                val gpl = remember { readAsset(context, LicenseData.COPYING) }
                LicenseTextPage(APP_LICENSE, "The licence of or2 itself", null, null, listOf(LicenseText("COPYING", gpl)), back = {})
            }
            "hostkey-first" -> HostKey(changed = false)
            "hostkey-changed" -> HostKey(changed = true)
            "hostkey-changed-many" -> HostKey(changed = true, many = true)
            "terminal" -> Terminal()
            "terminal-tmux" -> Terminal(target = "tmux main", transport = Transport.MOSH)
            "terminal-long" -> Terminal(host = "build-box-staging-eu-west", target = "herdr personal w1:p2", transport = Transport.MOSH)
            "terminal-stale" -> Terminal(target = "tmux main", transport = Transport.MOSH, health = LinkHealth(12_300uL))
            "terminal-closed" -> Terminal(target = "tmux main", cardState = SessionState.Closed(CloseReason.Disconnected))
            "terminal-arrowpad" -> Terminal(pad = true)
            "terminal-arrowpad-text" -> Terminal(pad = true, dense = true)
            "terminal-herdr-wheel" -> Terminal(target = "herdr personal w1:p2", herdrWheelAway = true)
            "terminal-composer" -> Terminal(composer = true)
            "terminal-attach" -> Terminal(composer = true, images = true)
            "terminal-uploading" -> Terminal(target = "tmux main", images = true, upload = UploadState.Uploading())
            "terminal-upload-failed" -> Terminal(target = "tmux main", images = true,
                upload = UploadState.Failed("SFTP is not available on this host"))
            "share-picker" -> Box(Modifier.fillMaxSize()) {
                Home(HomeVariant.Sessions)
                SharePickerSheet(
                    listOf(ShareTarget(1, "workstation", "tmux main"), ShareTarget(2, "build-box", "herdr personal"),
                        ShareTarget(3, "workstation", "shell")),
                    pick = {}, dismiss = {},
                )
            }
            else -> Text("Unknown screen: $name", color = Or2Colors.Danger)
        }
    }

    // --- data ----------------------------------------------------------------------------

    /** A review from a made-up pairing code (the real parser, so the fingerprint is the key's own). */
    @Composable
    private fun pairReview(keys: List<KeyRecord>, choice: KeyChoice, error: String?) {
        val offer = remember {
            parsePairPayload(
                "or2-pair:2?name=workstation&user=dev&port=22&a=100.101.102.103&a=192.168.1.20&a=workstation.local" +
                    "&hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7" +
                    "&id=abcdefghijklm",
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

    private fun agent(
        pane: String, status: AgentStatus, name: String, cwd: String, tab: String = "w1:t1", workspace: String = "w1", title: String? = null,
    ) = HerdrAgent(pane, tab, workspace, name, "claude", name, status, cwd, 1uL, "term_$pane", null, title)

    private fun view(vararg agents: HerdrAgent) = HerdrView(
        1uL, null,
        listOf(HerdrWorkspace("w1", 1u, "or2"), HerdrWorkspace("w2", 2u, "docs")),
        listOf(HerdrTab("w1:t1", "w1", 1u, "ui-polish"), HerdrTab("w2:t1", "w2", 1u, "readme")),
        emptyList(), agents.toList(),
    )

    /**
     * A herdr session as the Spaces sheet shows it: `~` with one plain tab, `or2` with two tabs (the focused `ui` holding
     * a blocked and the focused, working agent; `2` one done agent, no title), `docs` with no tabs (left out).
     */
    private val spacesView = HerdrView(
        1uL, "w2:p2",
        listOf(HerdrWorkspace("w1", 1u, "~"), HerdrWorkspace("w2", 2u, "or2"), HerdrWorkspace("w3", 3u, "docs")),
        listOf(HerdrTab("w1:t1", "w1", 1u, "1"), HerdrTab("w2:t1", "w2", 1u, "ui"), HerdrTab("w2:t2", "w2", 2u, "2")),
        emptyList(),
        listOf(
            agent("w2:p1", AgentStatus.BLOCKED, "Claude Code", "~/code/or2", tab = "w2:t1", workspace = "w2", title = "Repository context gathering"),
            agent("w2:p2", AgentStatus.WORKING, "reviewer", "~/code/or2", tab = "w2:t1", workspace = "w2", title = "Review v013 brief | or2"),
            agent("w2:p3", AgentStatus.DONE, "Codex", "~/code/or2", tab = "w2:t2", workspace = "w2"),
        ),
        "w2:t1",
    )

    private val caps = HostCapabilities("/usr/bin/tmux", "/home/dev/.local/bin/herdr", null, listOf(
        HerdrSessionInfo("personal", true, true), HerdrSessionInfo("work", true, false), HerdrSessionInfo("archive", false, false)))
    /**
     * The live herdr views of [caps]' sessions: the default one (`personal`) runs four agents in two workspaces, `work`
     * none; `archive` is not running (one row).
     */
    private val pickerViews = mapOf<String?, HerdrView>(
        null to view(
            agent("w1:p1", AgentStatus.BLOCKED, "Claude Code", "~/code/or2", title = "Repository context gathering"),
            agent("w1:p2", AgentStatus.WORKING, "Codex", "~/code/or2/android/app/src/main/java/io/github/code_akram/or2", title = "Review v013 brief | or2"),
            agent("w2:p1", AgentStatus.DONE, "Claude Code", "~/code/docs", tab = "w2:t1", workspace = "w2"),
            agent("w2:p2", AgentStatus.IDLE, "Amp", "~/code/docs", tab = "w2:t1", workspace = "w2"),
        ),
        "work" to view(),
    )

    /** A host with more herdr sessions than fit: the picker at full height, its list scrolling inside the sheet. */
    private val manyCaps = caps.copy(herdrSessions = (1..30).map { HerdrSessionInfo("session-$it", it % 3 != 0, it == 1) })
    private val tmux = TmuxList.Loaded(listOf(
        TmuxSession("main", 3u, 1u), TmuxSession("build", 1u, 0u), TmuxSession("scratch", 2u, 0u)))

    private fun demoGrid(columns: Int = 55, rows: Int = 60) = TerminalGrid().also { check(it.apply(terminalDemoFrame(columns, rows))) }

    // --- screens -------------------------------------------------------------------------

    private enum class HomeVariant { Sessions, Empty, CardStates, Notices, Connecting, Failed }

    @Composable
    private fun Home(variant: HomeVariant) {
        val grid = remember { demoGrid() }
        val live = variant == HomeVariant.Sessions
        // Each host's open terminals sit in its card: a herdr session showing its focused agent, a tmux session, and a
        // shell that has closed (marked, its final frame dimmed).
        val workstation = if (!live) emptyList() else listOf(
            HomeSession(1, "herdr", "Claude Code", Transport.MOSH) { m -> TerminalGridPreview(grid, 0, m) },
            HomeSession(2, "tmux main", "", Transport.MOSH) { m -> TerminalGridPreview(grid, 0, m) },
            HomeSession(3, "shell", "", Transport.SSH, closed = true) { m -> TerminalGridPreview(grid, 0, m) },
        )
        val buildBox = if (!live) emptyList() else listOf(
            HomeSession(4, "shell", "", Transport.SSH, closeAsks = true) { m -> TerminalGridPreview(grid, 0, m) },
        )
        val hosts = when (variant) {
            HomeVariant.Empty -> emptyList()
            HomeVariant.Sessions, HomeVariant.Notices, HomeVariant.Connecting, HomeVariant.Failed -> listOf(
                card(host(1, "workstation", address = "workstation.local"), HostState.Connected(0u), blocked = 1, terminals = workstation),
                card(host(2, "build-box", address = "198.51.100.7"), HostState.Connected(0u), terminals = buildBox),
                // The host the connecting and failed pickers are for: its card says the same as the sheet.
                card(host(3, "nas"), when (variant) {
                    HomeVariant.Connecting -> HostState.Connecting
                    HomeVariant.Failed -> HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected))
                    else -> null
                }),
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
            hosts, keyCount = if (variant == HomeVariant.Empty) 0 else 2,
            blocked = if (variant == HomeVariant.Empty) 0 else 1,
            // Two hosts can connect on the card-states page: `Connect all` shows at the header's end there.
            canConnectAll = variant == HomeVariant.CardStates, busy = false,
            openPicker = {}, openSession = {}, closeSession = {}, addHost = {}, easyPair = {}, manualHost = {}, editHost = {}, connectHost = {},
            disconnectHost = {}, deleteHost = {}, openInbox = {}, openKeys = {}, connectAll = {},
            // Both one-line offers: the battery exemption was declined, and the connection notification is not allowed.
            batteryCard = variant == HomeVariant.Notices, notificationCard = variant == HomeVariant.Notices,
        )
    }

    private fun card(host: Host, state: HostState?, blocked: Int = 0, unlocking: Boolean = false, terminals: List<HomeSession> = emptyList()) =
        HostCard(host, hostCardStatus(state, unlocking, blocked), linkStatus(state), terminals = terminals)

    @Composable
    private fun Inbox(empty: Boolean) {
        val one = host(1, "workstation")
        val two = host(2, "build-box", address = "198.51.100.7")
        val state = if (empty) {
            InboxState(listOf(InboxHostRow(one, LinkStatus.CONNECTED, "Connected", "No running herdr sessions", 0)), emptyList())
        } else {
            val groups = buildInbox(listOf(
                InboxSource(1, "workstation", null, "personal", view(
                    agent("w1:p1", AgentStatus.BLOCKED, "Claude Code", "~/code/or2", title = "Repository context gathering"),
                    agent("w1:p2", AgentStatus.WORKING, "Codex", "~/code/herdr", title = "Review v013 brief | or2"),
                    agent("w2:p1", AgentStatus.DONE, "Claude Code", "~/code/docs", tab = "w2:t1", workspace = "w2"),
                    agent("w2:p2", AgentStatus.WORKING, "Amp", "~/code/docs", tab = "w2:t1", workspace = "w2"),
                )),
                // pi reports no session and its integration is missing: its row offers Enable Reply.
                InboxSource(2, "build-box", "work", "work", view(
                    agent("w1:p9", AgentStatus.IDLE, "Claude Code", "~/src/build"),
                    HerdrAgent("w1:p8", "w1:t1", "w1", null, "pi", "pi", AgentStatus.BLOCKED, "~/src/build", 1uL, "term_w1:p8"),
                ), mapOf("claude" to HerdrIntegrationState.CURRENT, "pi" to HerdrIntegrationState.NOT_INSTALLED)),
            ))
            InboxState(listOf(
                InboxHostRow(one, LinkStatus.CONNECTED, "Connected", null, 4),
                InboxHostRow(two, LinkStatus.CONNECTED, "Connected", "herdr is unavailable: the session's socket cannot be opened (is it owned by another user?)", 1),
                InboxHostRow(host(3, "nas"), LinkStatus.FAILED, "Authentication rejected. Check the username and public-key authorization.", null, 0),
            ), groups)
        }
        InboxScreen(state, busy = false, connectAll = {}, connect = {}, openAgent = {})
    }

    /**
     * The session picker over Home, from a card's header: the lists for a connected host ([gate] null; the herdr session
     * `personal` and the tmux session `main` are open in or2, so marked `Open`), or the sheet before its host has
     * connected. [udpBlocked] adds the muted line on mosh's UDP under the tabs. The herdr tab shows [pickerViews]' agents
     * (not with [many], whose sessions are not watched: one row each).
     */
    @Composable
    private fun HomePicker(variant: HomeVariant, gate: PickerGate?, many: Boolean = false, tab: PickerTab? = null, udpBlocked: Boolean = false) {
        Box(Modifier.fillMaxSize()) {
            Home(variant)
            SessionPickerSheet(
                if (many) manyCaps else caps, null, tmux, OpenSessions(herdr = setOf("personal"), tmux = setOf("main")),
                openShell = {}, openTmux = {}, openHerdr = {}, refresh = {}, dismiss = {}, initialTab = tab, gate = gate, title = "workstation",
                udpBlocked = udpBlocked, herdrViews = if (many) emptyMap() else pickerViews,
            )
        }
    }

    @Composable
    private fun HostKey(changed: Boolean, many: Boolean = false) {
        val presented = PublicKeyInfo("ssh-ed25519", "k", "SHA256:3Fq8vXk0mT2yLdN7pRzA1sWbHcE9uJgOiVnY5tB4QeM", "")
        val two = listOf(PublicKeyInfo("ssh-ed25519", "o", "SHA256:Zc9bN1xQ4mWuT7yLdKp2sRfHaE8vJgOiVnY3tB0QeMA", ""),
            PublicKeyInfo("ssh-rsa", "r", "SHA256:Ab3dE5fG7hJ9kL1mN3pQ5rS7tU9vW1xY3zA5bC7dE9f", ""))
        // Many previously trusted keys: the tallest dialog, which must still keep clear of the status bar.
        val old = if (many) (1..6).flatMap { two } else two
        // The dialog shows over whatever is on screen: Home here.
        Box(Modifier.fillMaxSize()) {
            Home(HomeVariant.Connecting)
            HostTrustDialog(HostState.AwaitingHostKeyDecision(presented, if (changed) old else emptyList()), false, {}, {}, hostLabel = "nas")
        }
    }

    /**
     * The terminal screen over the probe's session; the card's own [cardState] and [health] are the
     * gallery's (the probe stays connected underneath, so the demo frame still shows). [dense] fills
     * every cell with text (the arrow pad over text); [herdrWheelAway] is a herdr target that tracks
     * the mouse after a swipe up went to herdr as wheel events (route 1): the scroll-to-bottom button
     * shows, and tapping it sends nothing anywhere here.
     */
    @Composable
    private fun Terminal(
        host: String = "workstation", target: String = "shell", transport: Transport = Transport.SSH,
        cardState: SessionState = SessionState.Connected, health: LinkHealth? = null, pad: Boolean = false, composer: Boolean = false,
        dense: Boolean = false, herdrWheelAway: Boolean = false, images: Boolean = false, upload: UploadState = UploadState.Idle,
    ) {
        val session = remember { startProbe() }
        val scope = rememberCoroutineScope()
        val herdr = remember { TerminalTarget.Herdr("personal", "w1:p2") }
        val scroller = remember(herdrWheelAway) {
            if (herdrWheelAway) TargetScroller(scope, { _ -> }).apply { wheeled(-6) } else null
        }
        val state by probeState.collectAsStateWithLifecycle()
        // Uploads that never leave the phone: the attach button shows, and a pick says where it would have gone.
        val paste = remember(images) { if (images) ImagePaste(scope) { _, extension -> "/home/dev/.cache/or2/images/or2-gallery.$extension" } else null }
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
                        view.grid.apply(terminalDemoFrame(size.first, size.second, view.grid.sequence + 1u, dense, mouseTracking = herdrWheelAway))
                        view.invalidate()
                        shown = size
                    }
                }
            }
        }
        // A herdr terminal has the third, blue disc (Spaces); shell and tmux headers keep two.
        TerminalCard(host, target, transport, cardState, minimise = {}, openSwitcher = {}, endSession = {}, linkHealth = health,
            upload = uploadNotice(upload), openSpaces = if (target.startsWith("herdr")) ({}) else null) {
            TerminalScreen(session, probeState, probeFrames.receiveAsFlow(), Modifier.weight(1f),
                composerHint = "Message $host…", chrome = chrome,
                target = if (scroller != null) herdr else TerminalTarget.Shell, targetScroller = scroller, imagePaste = paste)
        }
    }

    private fun startProbe(): Session {
        probe?.let { return it }
        return contractProbeSession(
            40u, 12u,
            object : SessionListener {
                override fun onStateChanged(state: SessionState) {
                    probeState.value = state
                }
                override fun onFrameReady() { probeFrames.trySend(Unit) }
                override fun onLinkHealth(health: LinkHealth) = Unit
                override fun onClipboardWrite(text: String) = Unit
                override fun onServerPid(pid: UInt) = Unit
            },
        ).also { probe = it }
    }

    /**
     * A slow drag up the middle of the screen, ending still (so nothing flings): content scrolls up under the top bar
     * exactly as a finger would move it.
     */
    private fun dragUp(root: View) {
        val x = root.width / 2f
        val from = root.height * 0.75f
        val to = root.height * 0.35f
        val start = SystemClock.uptimeMillis()
        fun event(action: Int, y: Float, at: Long) = MotionEvent.obtain(start, start + at, action, x, y, 0).also {
            root.dispatchTouchEvent(it)
            it.recycle()
        }
        event(MotionEvent.ACTION_DOWN, from, 0)
        val steps = 20
        for (step in 1..steps) event(MotionEvent.ACTION_MOVE, from + (to - from) * step / steps, step * 16L)
        event(MotionEvent.ACTION_MOVE, to, steps * 16L + 200)
        event(MotionEvent.ACTION_UP, to, steps * 16L + 216)
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
        /** A name with this suffix is that screen with its content dragged up under the top bar (e.g. `licenses-scrolled`). */
        const val SCROLLED = "-scrolled"

        val screens = listOf(
            "home", "home-scrolled", "home-empty", "home-notices", "host-cards", "host-cards-scrolled", "home-options", "home-close-shell",
            "home-picker", "home-picker-many", "home-picker-connecting", "home-picker-failed",
            "inbox", "inbox-scrolled", "inbox-empty", "inbox-enable-reply", "picker-herdr", "picker-many", "picker-tmux", "picker-udp",
            "host-form", "host-form-scrolled", "host-form-new-key", "host-form-edit", "keys", "keys-scrolled", "keys-empty", "key-sheet",
            "settings", "about", "about-scrolled", "licenses", "licenses-scrolled", "license-text", "license-text-scrolled",
            "hostkey-first", "hostkey-changed", "hostkey-changed-many", "shortcuts",
            "add-host", "pair-scan", "pair-scan-scrolled", "pair-scan-denied", "pair-review", "pair-review-scrolled", "pair-review-new",
            "pair-progress", "pair-install", "keepalive", "keepalive-waiting",
            "terminal", "terminal-tmux", "terminal-long", "terminal-stale", "terminal-closed",
            "terminal-arrowpad", "terminal-arrowpad-text", "terminal-herdr-wheel", "terminal-composer",
            "terminal-attach", "terminal-uploading", "terminal-upload-failed", "share-picker", "terminals", "spaces",
        )
    }
}

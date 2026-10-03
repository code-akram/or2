package io.github.code_akram.or2.app

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.about.AboutRoute
import io.github.code_akram.or2.about.LicensesRoute
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.hosts.HostFormScreen
import io.github.code_akram.or2.inbox.InboxScreen
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.hostStates
import io.github.code_akram.or2.inbox.inbox
import io.github.code_akram.or2.inbox.pendingHostKeys
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.notify.AgentAlertSettings
import io.github.code_akram.or2.notify.AgentOpenRequests
import io.github.code_akram.or2.notify.OnScreen
import io.github.code_akram.or2.pair.AddHostSheet
import io.github.code_akram.or2.pair.KeepAliveScreen
import io.github.code_akram.or2.pair.PairDestination
import io.github.code_akram.or2.pair.PairFlow
import io.github.code_akram.or2.pair.PairState
import io.github.code_akram.or2.pair.ShownCode
import io.github.code_akram.or2.paste.ImageShares
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.SessionScreen
import io.github.code_akram.or2.ui.Or2BottomInsets
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Theme
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.or2Background

/** What the screens can ask for; the activity implements them (biometrics, storage, ...). */
class AppActions(
    val saveHost: (Host, Host?) -> Unit,
    val deleteHost: (Host) -> Unit,
    val generateKey: (label: String, comment: String) -> Unit,
    val importKey: (Uri, String, String?) -> Unit,
    val deleteKey: (String) -> Unit,
    /** Unlocks (one prompt per key record) and connects the hosts that are not live. */
    val connect: (List<Host>) -> Unit,
    val approve: (ActiveHost, HostState.AwaitingHostKeyDecision) -> Unit,
    val reject: (ActiveHost) -> Unit,
    /** Shows a message, or clears it when null. */
    val message: (String?) -> Unit,
    /** The last focused terminal, for reattach after the app returns to the foreground. */
    val reattach: ReattachMemory = ReattachMemory(MemoryPrefStore()),
    /**
     * The battery-optimisation exemption, asked once as the last step of adding a host ([Destination.KeepAlive],
     * [BatteryPrompt.step], answered through [answerKeepAlive]: "Allow" opens the system's own request) and the
     * non-blocking Home card that remains when it was declined ([requestBatteryExemption] opens the request again).
     * Never asked on connect.
     */
    val battery: BatteryPrompt = BatteryPrompt(MemoryPrefStore()),
    val requestBatteryExemption: () -> Unit = {},
    val answerKeepAlive: (allow: Boolean) -> Unit = {},
    /**
     * The in-context offer of the connection notification (Home's card while a host is connected); never asked on
     * connect. [allowNotifications] asks for `POST_NOTIFICATIONS` (or opens the app's notification settings once
     * Android no longer asks); it is the one entry point later uses (agent alerts) call from their own offer.
     */
    val notifications: NotificationOffer = NotificationPermission(MemoryPrefStore()) { true }.offer(NotificationUse.CONNECTION),
    val allowNotifications: () -> Unit = {},
    /** True once per process when the previous one died with sessions open ([SessionMarker]): the launcher resumes. */
    val takeColdResume: () -> Boolean = { false },
    /** Easy pair: the flow (its state outlives the activity), and the phone's name for the host and for new keys. */
    val pair: PairFlow? = null,
    val deviceLabel: String = "phone",
    /** **New key** on the pairing review and the host form: makes and stores an Ed25519 key (one biometric prompt). */
    val createKey: suspend (label: String, comment: String) -> KeyRecord = { _, _ -> error("key creation is not available") },
    /** The `Agent notifications` switch (Settings), and what turning it changes ([setAgentAlerts]). */
    val agentAlerts: AgentAlertSettings = AgentAlertSettings(MemoryPrefStore()),
    val setAgentAlerts: (Boolean) -> Unit = {},
    /** An agent notification's tap: the pane to open, its host connected first when it is not. */
    val agentOpens: AgentOpenRequests = AgentOpenRequests(),
    /** The terminal on screen while the app is resumed, else null: its herdr pane gets no notification. */
    val onScreen: (OnScreen?) -> Unit = {},
    /** Images shared from another app: the user picks the open terminal each goes to. */
    val imageShares: ImageShares = ImageShares(),
)

/**
 * The whole UI: Home is the start destination (the one place for hosts and their terminals), the agents inbox its
 * sibling (an icon button switches); keys, settings, the host form and terminals push on top. Home's own assembly is
 * [HomeRoute], the reattach and resume state [rememberResume], shared images [ImageShareRoute] and the floating
 * cards [Notices].
 */
@Composable
fun Or2App(
    hosts: List<Host>, keys: List<KeyRecord>, message: String?, busy: Boolean,
    connections: HostConnections, actions: AppActions,
    /** False until the stored hosts and keys have been read: screens that edit one wait for it. */
    loaded: Boolean = true,
) {
    var saved by rememberSaveable { mutableStateOf(NavStack().encode()) }
    val nav = remember(saved) { NavStack.decode(saved) }
    val activations = connections.activations
    val focusing by activations.pending.collectAsStateWithLifecycle()
    // Any navigation of the user's own ends a wait for a pane focus; its terminal is not shown after all.
    fun navigate(next: NavStack) {
        activations.cancel()
        // Leaving the pairing screens ends the pairing (and wipes its code).
        if (nav.current is Destination.EasyPair && next.current !is Destination.EasyPair) actions.pair?.cancel()
        saved = next.encode()
    }
    fun pop() { nav.backOrHome()?.let(::navigate) }
    val terminals by connections.terminals.collectAsStateWithLifecycle()
    DisposableEffect(activations) { onDispose { activations.cancel() } }
    val current = nav.current
    val currentTerminal = (current as? Destination.Terminal)?.let { destination -> terminals.find { it.id == destination.terminalId } }
    // Back leaves the app only from Home; from the Inbox (a sibling top-level screen) and from a terminal it goes Home.
    BackHandler(enabled = nav.backOrHome() != null) { nav.backOrHome()?.let(::navigate) }

    val hostsNow = rememberUpdatedState(hosts)
    val inbox by remember(connections) { connections.inbox(snapshotFlow { hostsNow.value }) }
        .collectAsStateWithLifecycle(InboxState(emptyList(), emptyList()))
    val states by remember(connections) { connections.hostStates() }.collectAsStateWithLifecycle(emptyMap())
    val pending by remember(connections) { connections.pendingHostKeys() }.collectAsStateWithLifecycle(emptyList())
    val keepAliveStep by actions.battery.step.collectAsStateWithLifecycle()

    // "Add host" (the FAB) opens the add-host chooser in a sheet; the empty Home and inbox show the same chooser
    // inline. Its two cards lead to Easy pair (scan) or the manual form.
    var addSheet by rememberSaveable { mutableStateOf(false) }
    val pairFlow = actions.pair
    val pairState by (pairFlow?.state ?: remember { kotlinx.coroutines.flow.MutableStateFlow<PairState>(PairState.Scanning(ShownCode.None)) })
        .collectAsStateWithLifecycle()
    fun easyPair() {
        addSheet = false
        pairFlow?.start()
        navigate(nav.push(Destination.EasyPair))
    }
    fun manualHost() {
        addSheet = false
        navigate(nav.push(Destination.HostForm(0)))
    }

    // Hosts between the tap and the key being unlocked: their card says "Unlocking key...".
    var unlocking by remember { mutableStateOf(emptySet<Long>()) }
    LaunchedEffect(busy) { if (!busy) unlocking = emptySet() }
    fun connect(list: List<Host>) {
        unlocking = list.map { it.id }.toSet()
        actions.connect(list)
    }

    // The session picker over Home, from a host card's header: the host it is for, or null. It belongs to Home: anything
    // that leaves Home (a terminal opening, a notification's tap) closes it.
    var homePicker by rememberSaveable { mutableStateOf<Long?>(null) }
    LaunchedEffect(current) { if (current != Destination.Home) homePicker = null }

    // An activation finished: show the terminal (reading the stack now: the screen may have moved on), or
    // explain why not and stay where we are.
    fun enter(activation: Activation, replace: Boolean) {
        when (activation) {
            is Activation.Ready -> {
                actions.message(null)
                val stack = NavStack.decode(saved)
                val destination = Destination.Terminal(activation.terminal.id)
                saved = (if (replace) stack.replaceTop(destination) else stack.push(destination)).encode()
            }
            is Activation.Failed -> actions.message(activation.message)
        }
    }
    // An open terminal shown again (a Home thumbnail, the Terminals sheet, a share): as it is, nothing to wait for.
    fun show(id: Long, replace: Boolean = false) {
        actions.message(null)
        val destination = Destination.Terminal(id)
        navigate(if (replace) nav.replaceTop(destination) else nav.push(destination))
    }

    // Opens at once: under AUTO a tmux or herdr terminal starts on SSH and moves to mosh behind the
    // scenes once UDP is known to work (see HostConnections.openTerminal).
    fun openTerminal(active: ActiveHost, target: TerminalTarget) {
        actions.message(null)
        activations.launchOpen(active, target) { enter(it, replace = false) }
    }

    val resume = rememberResume(
        hosts, terminals, states, busy, loaded, currentTerminal, connections, actions,
        AppNavigation(stack = { NavStack.decode(saved) }, navigate = ::navigate, enter = ::enter), ::connect,
    )

    // The visible terminal of a resumed app is "on screen": its herdr pane gets no agent notification, and loses one it had.
    val lifecycleState by LocalLifecycleOwner.current.lifecycle.currentStateFlow.collectAsState()
    val onScreen = currentTerminal?.takeIf { lifecycleState.isAtLeast(Lifecycle.State.RESUMED) }?.let { OnScreen(it.host.id, it.target) }
    LaunchedEffect(onScreen) { actions.onScreen(onScreen) }
    DisposableEffect(actions) { onDispose { actions.onScreen(null) } }

    AppScaffold(fullScreen = current is Destination.Terminal) {
        Box(Modifier.fillMaxSize()) {
            if (!loaded && current is Destination.HostForm) {
                // A restored form must not render (or save) before its host is read.
                Column { TopBar(back = ::pop) }
            } else when (current) {
                Destination.Home -> HomeRoute(
                    hosts, keys.size, terminals, states, inbox, connections, busy, unlocking, actions, resume,
                    picker = homePicker, setPicker = { homePicker = it },
                    connect = ::connect, openTerminal = ::openTerminal, show = { show(it) },
                    push = { navigate(nav.push(it)) }, addHost = { addSheet = true }, easyPair = ::easyPair, manualHost = ::manualHost,
                )
                Destination.Inbox -> InboxScreen(
                    inbox, busy,
                    connectAll = { connect(inbox.hosts.map { it.host }) },
                    connect = { connect(listOf(it)) },
                    // An explicit agent request: its pane is focused first, then its session's terminal is shown.
                    openAgent = { item ->
                        actions.message(null)
                        activations.launchOpenAgent(item.hostId, item.hostLabel, item.session, item.paneId) { enter(it, replace = false) }
                    },
                    openHome = { navigate(nav.top(Destination.Home)) },
                    openKeys = { navigate(nav.push(Destination.Keys)) },
                    easyPair = ::easyPair, manualHost = ::manualHost,
                )
                Destination.About -> AboutRoute(back = ::pop, openLicenses = { navigate(nav.push(Destination.Licenses)) })
                Destination.Licenses -> LicensesRoute(back = ::pop)
                Destination.Settings -> {
                    val agentAlerts by actions.agentAlerts.enabled.collectAsStateWithLifecycle()
                    SettingsRoute(agentAlerts, actions.setAgentAlerts, back = ::pop)
                }
                Destination.Keys -> KeysScreen(keys, busy, actions.generateKey, actions.importKey, actions.deleteKey, back = ::pop)
                Destination.EasyPair -> if (pairFlow == null) Column { TopBar(back = ::pop) } else PairDestination(
                    pairState, keys, pairFlow, actions.deviceLabel, actions.createKey,
                    close = ::pop,
                    // A code made with --manual ends on the key to install: Done returns Home with the host saved (through
                    // the battery step, the last step of adding a host, when it is still to be asked).
                    done = { pairFlow.consume(); navigate(NavStack.afterKeyToInstall(actions.battery.shouldOffer())) },
                )
                is Destination.HostForm -> {
                    val previous = hosts.find { it.id == current.hostId }
                    HostFormScreen(previous, keys, busy, save = { host -> actions.saveHost(host, previous) }, close = ::pop,
                        createKey = actions.createKey, deviceLabel = actions.deviceLabel,
                        // A new host ends on the battery step (after the key line, with New key); an edit just closes.
                        saved = { navigate(nav.afterHostFormSaved(previous == null && actions.battery.shouldOffer())) },
                        // Deleted: back to Home, where its card was.
                        delete = { host -> actions.deleteHost(host); navigate(NavStack()) })
                }
                is Destination.KeepAlive -> KeepAliveScreen(
                    waiting = keepAliveStep == KeepAliveStep.WAIT,
                    allow = { actions.answerKeepAlive(true) }, notNow = { actions.answerKeepAlive(false) },
                )
                is Destination.Terminal -> SessionScreen(
                    connections, currentTerminal, terminals,
                    minimise = { navigate(nav.top(Destination.Home)) },
                    select = { show(it.id, replace = true) },
                )
            }
            // One overlay, one order: at the top of a full-screen terminal (it has no notice area), else above the
            // bottom (and above Home's FAB).
            Notices(
                message, focusing, resume.chip, busy,
                dismissMessage = { actions.message(null) }, reconnect = resume.acceptChip, dismissChip = resume.dismissChip,
                modifier = if (current is Destination.Terminal) Modifier.align(Alignment.TopCenter).padding(top = Or2Dimens.Gutter)
                else Modifier.align(Alignment.BottomCenter).windowInsetsPadding(Or2BottomInsets)
                    .padding(bottom = if (current == Destination.Home) Or2Dimens.Fab + 24.dp else 8.dp),
            )
        }
    }
    if (addSheet) {
        AddHostSheet(easyPair = ::easyPair, manual = ::manualHost, dismiss = { addSheet = false })
    }
    ImageShareRoute(actions, connections, terminals, show = { id -> show(id, replace = NavStack.decode(saved).current is Destination.Terminal) })
    // Paired: the host and its trusted key are saved. Once the stored list shows it, land on Home with its picker open
    // and connect (the unlock is the usual one; the host key is already trusted, so no first-use prompt). When the
    // battery step is still to be asked, it comes first, as the last step of adding the host: it then does the same.
    val paired = (pairState as? PairState.Paired)?.host
    LaunchedEffect(paired, hosts, busy) {
        val host = paired ?: return@LaunchedEffect
        if (hosts.none { it.id == host.id } || busy) return@LaunchedEffect
        pairFlow?.consume()
        val keepAlive = actions.battery.shouldOffer()
        navigate(NavStack.afterPaired(host.id, keepAlive))
        if (!keepAlive) {
            homePicker = host.id
            connect(listOf(host))
        }
    }
    // The battery step is answered (or there is nothing left to ask: exempt meanwhile, or answered before a restore):
    // on to Home with the paired host's picker open, connecting it, or back to where the host was added from. Never
    // during a connect: a paired host connects only after the step.
    val keepAliveHost = (current as? Destination.KeepAlive)?.hostId
    LaunchedEffect(keepAliveHost, keepAliveStep, loaded, busy) {
        val hostId = keepAliveHost ?: return@LaunchedEffect
        if (keepAliveStep != KeepAliveStep.DONE || !loaded || busy) return@LaunchedEffect
        navigate(NavStack.decode(saved).afterKeepAlive())
        val host = hostsNow.value.find { it.id == hostId } ?: return@LaunchedEffect
        homePicker = host.id
        connect(listOf(host))
    }
    // A host-key prompt needs an answer wherever the user is, one dialog at a time (over the picker too).
    pending.firstOrNull()?.let { (active, prompt) ->
        HostTrustDialog(prompt, busy, { actions.approve(active, prompt) }, { actions.reject(active) },
            hostLabel = hosts.find { it.id == active.host.id }?.label ?: active.host.label)
    }
}

/**
 * Shared chrome: the dark theme and the soft-glow background, edge to edge. Only the side and top
 * insets are applied here: scrolling screens run under the navigation bar and reserve it at the end
 * of their content ([io.github.code_akram.or2.ui.BottomInsetSpacer]); a terminal ([fullScreen])
 * owns the bottom (IME and navigation bar) itself, everything else lifts above the keyboard.
 */
@Composable
fun AppScaffold(fullScreen: Boolean, content: @Composable () -> Unit) {
    Or2Theme {
        Box(Modifier.fillMaxSize().or2Background()) {
            val insets = WindowInsets.systemBars.union(WindowInsets.displayCutout)
            // The shell: every screen lives inside the safe area and is clipped to it, so nothing a screen
            // draws (a title, an icon, a row moved by a scroll, an overscroll stretch, a drag) can ever reach
            // the status bar or the cutout. Only the background above runs under the system bars.
            Box(
                Modifier.fillMaxSize()
                    .windowInsetsPadding(insets.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top))
                    .clipToBounds()
                    .then(if (fullScreen) Modifier else Modifier.imePadding()),
            ) { content() }
        }
    }
}

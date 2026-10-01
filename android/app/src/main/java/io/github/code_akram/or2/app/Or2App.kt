package io.github.code_akram.or2.app

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.activity.compose.LocalActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.reconnectOffer
import io.github.code_akram.or2.connection.ReconnectOffer
import io.github.code_akram.or2.connection.terminalClosedStates
import io.github.code_akram.or2.connection.transports
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.home.HomeResume
import io.github.code_akram.or2.home.HomeScreen
import io.github.code_akram.or2.home.HomeSession
import io.github.code_akram.or2.home.HostCard
import io.github.code_akram.or2.home.hostCardStatus
import io.github.code_akram.or2.host.HostScreen
import io.github.code_akram.or2.host.HostTerminalItem
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.hosts.HostFormScreen
import io.github.code_akram.or2.inbox.InboxScreen
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.dialogForOtherHost
import io.github.code_akram.or2.inbox.hostStates
import io.github.code_akram.or2.inbox.inbox
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.inbox.pendingHostKeys
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.SessionScreen
import io.github.code_akram.or2.session.hostErrorMessage
import io.github.code_akram.or2.terminal.TerminalThumbnail
import io.github.code_akram.or2.terminal.display
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.Or2BottomInsets
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Theme
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.or2Background
import kotlinx.coroutines.CancellationException

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
    /** The one-time battery-optimisation explanation; [requestBatteryExemption] opens the system dialog. */
    val battery: BatteryPrompt = BatteryPrompt(MemoryPrefStore()),
    val requestBatteryExemption: () -> Unit = {},
)

/**
 * The whole UI: Home is the start destination, the agents inbox its sibling (an icon button
 * switches); keys, a host, the host form and terminals push on top.
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
    fun navigate(next: NavStack) { activations.cancel(); saved = next.encode() }
    fun pop() { nav.backOrHome()?.let(::navigate) }
    val terminals by connections.terminals.collectAsStateWithLifecycle()
    DisposableEffect(activations) { onDispose { activations.cancel() } }
    val current = nav.current
    val currentTerminal = (current as? Destination.Terminal)?.let { destination -> terminals.find { it.id == destination.terminalId } }
    // Back leaves the app only from Home; from the Inbox (a sibling top-level screen) it goes Home.
    BackHandler(enabled = nav.backOrHome() != null) { nav.backOrHome()?.let(::navigate) }

    val hostsNow = rememberUpdatedState(hosts)
    val inbox by remember(connections) { connections.inbox(snapshotFlow { hostsNow.value }) }
        .collectAsStateWithLifecycle(InboxState(emptyList(), emptyList()))
    val states by remember(connections) { connections.hostStates() }.collectAsStateWithLifecycle(emptyMap())
    val pending by remember(connections) { connections.pendingHostKeys() }.collectAsStateWithLifecycle(emptyList())

    val transports by remember(connections) { connections.transports() }.collectAsStateWithLifecycle(emptyMap())
    val closedStates by remember(connections) { connections.terminalClosedStates() }.collectAsStateWithLifecycle(emptyMap())
    val last by actions.reattach.last.collectAsStateWithLifecycle()

    // Hosts between the tap and the key being unlocked: their card says "Unlocking key...".
    var unlocking by remember { mutableStateOf(emptySet<Long>()) }
    LaunchedEffect(busy) { if (!busy) unlocking = emptySet() }
    fun connect(list: List<Host>) {
        unlocking = list.map { it.id }.toSet()
        actions.connect(list)
    }

    // Hosts whose session picker was already offered for the connection they have now, so Back from
    // a terminal returns to the host page without covering it with the picker again.
    var offered by rememberSaveable(stateSaver = listSaver<Set<Long>, Long>(save = { it.toList() }, restore = { it.toSet() })) {
        mutableStateOf(emptySet<Long>())
    }
    fun openHostPage(hostId: Long) {
        offered = offered - hostId // A fresh visit offers the picker again.
        navigate(nav.push(Destination.HostPage(hostId)))
    }

    // The focus finished: show the terminal (reading the stack now: the screen may have moved on), or
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
    // Home thumbnails, the host screen's recent list and the switcher: an open terminal is activated again.
    fun resumeTerminal(id: Long, replace: Boolean = false) {
        val terminal = connections.terminal(id)
        if (terminal == null) navigate(if (replace) nav.replaceTop(Destination.Terminal(id)) else nav.push(Destination.Terminal(id)))
        else activations.launchReuse(terminal) { enter(it, replace) }
    }

    // Under AUTO a tap right after connecting waits (briefly) for the capability probe, so the
    // terminal does not silently open over SSH on a host that has mosh-server.
    fun openTerminal(active: ActiveHost, target: TerminalTarget) {
        actions.message(null)
        activations.launchOpen(active, target) { enter(it, replace = false) }
    }

    // --- reattach: the last focused terminal, and what to do when the app returns -----------------
    val connectedHosts = states.filterValues { it is HostState.Connected }.keys
    val reattach = decideReattach(
        last, terminals.map { OpenSession(it.id, it.host.id, it.target, closedStates[it.id] != true) },
        connectedHosts, hosts.map { it.id }.toSet(),
    )
    var pendingResume by remember { mutableStateOf<LastTerminal?>(null) }
    val resumeCard = when {
        pendingResume != null -> null
        reattach is Reattach.Reopen -> resumeCardOf(reattach.last, hosts)
        reattach is Reattach.Resume -> resumeCardOf(reattach.last, hosts)
        else -> null
    }
    fun hostLabel(id: Long) = hosts.find { it.id == id }?.label ?: "the host"
    // A terminal reopened or resumed replaces a terminal screen instead of stacking on it.
    fun enterReattached(activation: Activation) {
        enter(activation, replace = NavStack.decode(saved).current is Destination.Terminal)
    }
    fun reopen(target: LastTerminal) = activations.launchReopen(target, hostLabel(target.hostId), ::enterReattached)
    fun resumeLast() {
        val target = last ?: return
        val host = hosts.find { it.id == target.hostId } ?: return
        if (target.hostId in connectedHosts) {
            reopen(target)
        } else {
            pendingResume = target
            if (!busy) connect(listOf(host))
        }
    }
    val resumeState = pendingResume?.let { states[it.hostId] }
    LaunchedEffect(pendingResume, resumeState, busy) {
        val target = pendingResume ?: return@LaunchedEffect
        when (resumeStep(resumeState, busy)) {
            ResumeStep.WAIT -> Unit
            ResumeStep.ABORT -> pendingResume = null
            ResumeStep.OPEN -> { pendingResume = null; reopen(target) }
        }
    }

    // The chip was tapped: the usual grouped unlock (one biometric per distinct key) and connect. Coming
    // back from a terminal whose connection was lost, its terminal is reopened (or reused, if a mosh
    // session kept it alive) once the host is connected again.
    // The hosts whose lost connection the app offered to restore when it returned: shown as a chip
    // (not a dialog: nothing waits for an answer), for as long as they are still lost.
    var offeredIds by remember { mutableStateOf(emptySet<Long>()) }
    var stoppedOnTerminal by rememberSaveable { mutableStateOf(false) }
    fun acceptReconnect(offer: ReconnectOffer) {
        offeredIds = emptySet()
        last?.takeIf { stoppedOnTerminal && offer.hosts.any { host -> host.id == it.hostId } }?.let { pendingResume = it }
        connect(offer.hosts)
    }

    // What the user last had in front of them: remembered while the terminal is connected now (and not being
    // closed: `rememberShown` checks that when it runs). Its state is an input of the effect, and a terminal
    // that closed (the user's disconnect, a remote exit) never becomes the target again when the screen is
    // recreated or visited once more, whatever it once did.
    val shownState = currentTerminal?.state?.collectAsStateWithLifecycle()?.value
    val shownTransport = currentTerminal?.transport?.collectAsStateWithLifecycle()?.value
    LaunchedEffect(currentTerminal, shownState, shownTransport) {
        if (currentTerminal != null && shownState == SessionState.Connected && shownTransport != null) {
            actions.reattach.rememberShown(currentTerminal, shownTransport)
        }
    }
    // A terminal id saved by a previous process means nothing now: the process died while the user was
    // on that terminal (the system killed it, and brought the user back through the recents list).
    // Start from Home and resume at once: one grouped unlock (the fingerprint), then the host connects
    // and the remembered target reopens, with no further tap. A cold start from the launcher has no
    // saved destination and shows Home's Resume card instead.
    var recovered by remember { mutableStateOf(false) }
    LaunchedEffect(loaded) {
        if (!loaded || recovered) return@LaunchedEffect
        recovered = true
        val top = NavStack.decode(saved).current
        if (top is Destination.Terminal && connections.terminal(top.terminalId) == null) {
            navigate(NavStack())
            if (shouldAutoResume(actions.reattach.last.value, hostsNow.value, connectedHosts)) resumeLast()
        }
    }

    // Leaving and returning: see the contract's reattach, battery and reconnect rules. The flags
    // are saved state: the foreground service keeps the process alive, so the system can destroy
    // and recreate the activity while it is in the background (memory pressure, "don't keep
    // activities", a long time away), exactly when the connection is most likely to have died. The
    // work itself waits for the stored hosts to be read, which a recreated activity has not yet done.
    val activity = LocalActivity.current
    var returning by rememberSaveable { mutableStateOf(false) }
    var returned by remember { mutableStateOf(false) }
    var batteryExplanation by rememberSaveable { mutableStateOf(false) }
    val chipOffer = remember(offeredIds, hosts, states) {
        if (offeredIds.isEmpty()) null
        else reconnectOffer(hosts.filter { it.id in offeredIds }, connections.hosts.value)
    }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) {
        if (activity?.isChangingConfigurations == true) return@LifecycleEventEffect
        returning = true
        stoppedOnTerminal = NavStack.decode(saved).current is Destination.Terminal
        actions.battery.onBackgrounded(connections.hasOpenSession())
    }
    LifecycleEventEffect(Lifecycle.Event.ON_START) {
        if (!returning) return@LifecycleEventEffect
        returning = false
        returned = true
    }
    LaunchedEffect(returned, loaded) {
        if (!returned || !loaded) return@LaunchedEffect
        returned = false
        val stored = hostsNow.value
        if (actions.battery.takeIfDue()) batteryExplanation = true
        offeredIds = reconnectOffer(stored, connections.hosts.value)?.hosts?.map { it.id }?.toSet().orEmpty()
        val liveHosts = connections.hosts.value.filterValues { it.state.value is HostState.Connected }.keys
        val decision = decideReattach(
            actions.reattach.last.value,
            connections.terminals.value.map { OpenSession(it.id, it.host.id, it.target, it.state.value !is SessionState.Closed) },
            liveHosts, stored.map { it.id }.toSet(),
        )
        when {
            !stoppedOnTerminal -> Unit
            decision is Reattach.Show -> connections.terminal(decision.terminalId)?.let { terminal ->
                activations.launchReuse(terminal) { activation ->
                    enterReattached(activation)
                    (activation as? Activation.Ready)?.terminal?.handle?.value?.let { handle -> runCatching { handle.requestFullFrame() } }
                }
            }
            decision is Reattach.Reopen -> reopen(decision.last)
            else -> {
                // A terminal screen for a terminal that is gone (dismissed, or lost with a process that
                // died) gives way to Home. A terminal that closed stays: its reason and final frame are
                // the user's to read and dismiss, and Home's Resume card is there when it can be resumed.
                val top = NavStack.decode(saved).current
                if (top is Destination.Terminal && connections.terminal(top.terminalId) == null) navigate(NavStack())
            }
        }
    }

    AppScaffold(fullScreen = current is Destination.Terminal) {
        Box(Modifier.fillMaxSize()) {
            if (!loaded && (current is Destination.HostForm || current is Destination.HostPage)) {
                // A restored form or host page must not render (or save) before its host is read.
                Column { TopBar(back = ::pop) }
            } else when (current) {
                Destination.Home -> {
                    val blockedByHost = inbox.groups.filter { it.status == AgentStatus.BLOCKED }.flatMap { it.items }.groupingBy { it.hostId }.eachCount()
                    val sessions = remember(terminals, inbox, connections, transports) {
                        terminals.map { terminal ->
                            HomeSession(
                                terminal.id, terminal.host.label, terminal.title,
                                inbox.cwdOf(terminal) ?: (terminal.host.username + "@" + terminal.host.addresses.first().hostname),
                                (transports[terminal.id] ?: terminal.transport.value).display(),
                            ) { thumbnail -> TerminalThumbnail(terminal, connections, thumbnail) }
                        }
                    }
                    val cards = hosts.map { host ->
                        val state = states[host.id]
                        HostCard(host, hostCardStatus(state, host.id in unlocking, blockedByHost[host.id] ?: 0, host.sleeps), linkStatus(state, host.sleeps))
                    }
                    val connectable = cards.filter { it.host.keyId != null && it.link.canConnect }
                    HomeScreen(
                        sessions = sessions,
                        hosts = cards, keyCount = keys.size,
                        blocked = inbox.groups.filter { it.status == AgentStatus.BLOCKED }.sumOf { it.items.size },
                        working = inbox.groups.filter { it.status == AgentStatus.WORKING }.sumOf { it.items.size },
                        canConnectAll = connectable.size > 1, busy = busy,
                        openSession = { resumeTerminal(it.id) },
                        openHost = { host ->
                            val link = linkStatus(states[host.id], host.sleeps)
                            // Tapping a host that is not connected unlocks and connects it; the host screen
                            // is where its host-key prompts and the session picker live.
                            if (!busy && host.keyId != null && link.canConnect) connect(listOf(host))
                            openHostPage(host.id)
                        },
                        addHost = { navigate(nav.push(Destination.HostForm(0))) },
                        editHost = { navigate(nav.push(Destination.HostForm(it.id))) },
                        connectHost = { host ->
                            connect(listOf(host))
                            openHostPage(host.id)
                        },
                        disconnectHost = { connections.disconnect(it.id) },
                        deleteHost = actions.deleteHost,
                        openInbox = { navigate(nav.push(Destination.Inbox)) },
                        openKeys = { navigate(nav.push(Destination.Keys)) },
                        connectAll = { connect(connectable.map { it.host }) },
                        resume = resumeCard, onResume = { resumeLast() },
                    )
                }
                Destination.Inbox -> InboxScreen(
                    inbox, busy,
                    connectAll = { connect(inbox.hosts.map { it.host }) },
                    connect = { connect(listOf(it)) },
                    openHost = { openHostPage(it.id) },
                    // Focus the agent's pane first; tapping the same agent again returns to its open terminal.
                    openAgent = { item ->
                        actions.message(null)
                        activations.launchOpenAgent(item.hostId, item.hostLabel, item.session, item.paneId) { enter(it, replace = false) }
                    },
                    openHome = { navigate(nav.top(Destination.Home)) },
                    openKeys = { navigate(nav.push(Destination.Keys)) },
                    addHost = { navigate(nav.push(Destination.HostForm(0))) },
                )
                Destination.Keys -> KeysScreen(keys, busy, actions.generateKey, actions.importKey, actions.deleteKey, back = ::pop)
                is Destination.HostForm -> {
                    val previous = hosts.find { it.id == current.hostId }
                    HostFormScreen(previous, keys, busy, save = { host -> actions.saveHost(host, previous); pop() }, close = ::pop,
                        openKeys = { navigate(nav.push(Destination.Keys)) })
                }
                is Destination.HostPage -> HostPage(
                    current.hostId, hosts, terminals, connections, busy, actions, ::openTerminal, back = ::pop,
                    pickerOffered = current.hostId in offered,
                    setPickerOffered = { value -> offered = if (value) offered + current.hostId else offered - current.hostId },
                    edit = { navigate(nav.push(Destination.HostForm(current.hostId))) },
                    resume = { resumeTerminal(it) },
                )
                is Destination.Terminal -> SessionScreen(
                    connections, currentTerminal, terminals,
                    minimise = { navigate(nav.top(Destination.Home)) },
                    select = { resumeTerminal(it.id, replace = true) },
                )
            }
            if (current is Destination.Terminal) {
                // A full-screen terminal has no notice area: progress and failures float at the top.
                Column(Modifier.align(Alignment.TopCenter).fillMaxWidth().padding(Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    FocusNotice(focusing)
                    if (message != null) MessageCard(message, dismiss = { actions.message(null) })
                    chipOffer?.let { ReconnectChip(it, reconnect = { acceptReconnect(it) }, dismiss = { offeredIds = emptySet() }) }
                }
            } else {
                Notices(message, busy, focusing, dismiss = { actions.message(null) },
                    chip = chipOffer?.let { offer -> { ReconnectChip(offer, reconnect = { acceptReconnect(offer) }, dismiss = { offeredIds = emptySet() }) } },
                    modifier = Modifier.align(Alignment.BottomCenter).windowInsetsPadding(Or2BottomInsets)
                        .padding(bottom = if (current == Destination.Home) Or2Dimens.Fab + 24.dp else 8.dp))
            }
        }
    }
    // A prompt for a host whose screen is not showing still needs an answer, one dialog at a time:
    // the shown host's own screen already has its dialog.
    pending.dialogForOtherHost((current as? Destination.HostPage)?.hostId)?.let { (active, prompt) ->
        HostTrustDialog(prompt, busy, { actions.approve(active, prompt) }, { actions.reject(active) },
            hostLabel = hosts.find { it.id == active.host.id }?.label ?: active.host.label)
    }
    if (batteryExplanation) {
        Or2Dialog(
            onDismiss = { batteryExplanation = false }, title = "Keep sessions connected",
            confirm = {
                TextAction("Allow", { batteryExplanation = false; actions.requestBatteryExemption() }, modifier = Modifier.testTag("battery-allow"))
            },
            dismiss = { TextAction("Not now", { batteryExplanation = false }, color = Or2Colors.Text, modifier = Modifier.testTag("battery-dismiss")) },
            modifier = Modifier.testTag("battery-dialog"),
        ) {
            Text(
                "Android may stop or slow or2 while it is in the background and drop your sessions. " +
                    "Allow or2 to ignore battery optimisation so they stay connected. This is asked only once.",
            )
        }
    }
}

/** The Home card for the last terminal: what it was and how it was reached. */
private fun resumeCardOf(last: LastTerminal, hosts: List<Host>): HomeResume? {
    val host = hosts.find { it.id == last.hostId } ?: return null
    val what = when (val target = last.target) {
        TerminalTarget.Shell -> "shell"
        is TerminalTarget.Tmux -> "tmux ${target.sessionName}"
        is TerminalTarget.Herdr -> "herdr" + (target.paneId?.let { " $it" } ?: "")
    }
    return HomeResume("${host.label}: $what", last.transport.name.lowercase().replaceFirstChar { it.uppercase() })
}

/** The cwd herdr reports for the pane this terminal is attached to, if the inbox knows it. */
private fun InboxState.cwdOf(terminal: ActiveTerminal): String? {
    val target = terminal.target as? TerminalTarget.Herdr ?: return null
    val pane = target.paneId ?: return null
    return groups.flatMap { it.items }.find { it.hostId == terminal.host.id && it.session == target.session && it.paneId == pane }?.cwd
}

/** A message the user can dismiss. Live regions: assistive services announce a new one. */
@Composable
private fun MessageCard(message: String, dismiss: () -> Unit) {
    Or2Card(Modifier.testTag("message-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
        Row(Modifier.padding(start = Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Text(message, style = Or2Type.Secondary, color = Or2Colors.Text, modifier = Modifier.weight(1f).padding(vertical = 8.dp))
            IconAction(Or2Icons.Close, "Dismiss message", dismiss, Modifier.testTag("message-dismiss"), tint = Or2Colors.TextMuted)
        }
    }
}

/** In-place progress while an agent's pane is being focused: a card with a spinner, never a dialog. */
@Composable
private fun FocusNotice(focusing: String?) {
    if (focusing == null) return
    Or2Card(Modifier.fillMaxWidth().testTag("focus-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
        Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
            Spinner()
            Text(focusing + "…", style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 12.dp))
        }
    }
}

/** Messages and waiting state as floating cards above the content, never modal. */
@Composable
private fun Notices(
    message: String?, busy: Boolean, focusing: String?, dismiss: () -> Unit, modifier: Modifier = Modifier,
    chip: (@Composable () -> Unit)? = null,
) {
    Column(modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (message != null) MessageCard(message, dismiss)
        FocusNotice(focusing)
        chip?.invoke()
        if (busy) {
            Or2Card(Modifier.testTag("busy-banner").semantics { liveRegion = LiveRegionMode.Polite }, color = Or2Colors.SurfaceRaised) {
                Row(Modifier.padding(Or2Dimens.Gutter), verticalAlignment = Alignment.CenterVertically) {
                    Spinner()
                    Text("Waiting for authentication or operation…", style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                        modifier = Modifier.padding(start = 12.dp))
                }
            }
        }
    }
}

@Composable
private fun HostPage(
    hostId: Long, hosts: List<Host>, terminals: List<ActiveTerminal>, connections: HostConnections, busy: Boolean, actions: AppActions,
    openTerminal: (ActiveHost, TerminalTarget) -> Unit, back: () -> Unit, edit: () -> Unit, resume: (Long) -> Unit,
    pickerOffered: Boolean, setPickerOffered: (Boolean) -> Unit,
) {
    val host = hosts.find { it.id == hostId }
    if (host == null) {
        Column {
            TopBar(back = back)
            Text("This host no longer exists.", style = Or2Type.Body, color = Or2Colors.TextMuted, modifier = Modifier.padding(Or2Dimens.Gutter))
        }
        return
    }
    val active = connections.hosts.collectAsStateWithLifecycle().value[hostId]
    key(active) {
        val state = active?.state?.collectAsStateWithLifecycle()?.value
        val caps = active?.capabilities?.collectAsStateWithLifecycle()?.value
        val capsError = active?.capabilitiesError?.collectAsStateWithLifecycle()?.value
        var refreshes by remember { mutableIntStateOf(0) }
        var tmux by remember { mutableStateOf<TmuxList>(TmuxList.Loading) }
        val connected = state is HostState.Connected
        LaunchedEffect(active, connected, refreshes) {
            if (active == null || !connected) return@LaunchedEffect
            tmux = try {
                TmuxList.Loaded(connections.listTmuxSessions(active))
            } catch (error: CancellationException) {
                throw error
            } catch (error: HostException) {
                TmuxList.Failed(hostErrorMessage(error))
            }
        }
        val items = hostTerminalItems(terminals.filter { it.host.id == hostId })
        HostScreen(
            host, state, caps, capsError, tmux, busy,
            connect = { actions.connect(listOf(host)) },
            disconnect = { connections.disconnect(hostId) },
            approve = { prompt -> active?.let { actions.approve(it, prompt) } },
            reject = { active?.let(actions.reject) },
            openShell = { active?.let { openTerminal(it, TerminalTarget.Shell) } },
            openTmux = { name -> active?.let { openTerminal(it, TerminalTarget.Tmux(name)) } },
            openHerdr = { session -> active?.let { openTerminal(it, TerminalTarget.Herdr(session, null)) } },
            refresh = { refreshes++ },
            terminals = items, resume = resume, back = back, edit = edit,
            pickerOffered = pickerOffered, setPickerOffered = setPickerOffered,
        )
        // Refreshing re-probes capabilities (new herdr sessions) as well as the tmux list.
        LaunchedEffect(active, refreshes) {
            if (active != null && refreshes > 0) connections.refresh(active)
        }
    }
}

@Composable
private fun hostTerminalItems(terminals: List<ActiveTerminal>): List<HostTerminalItem> = terminals.map { terminal ->
    key(terminal.id) {
        val state by terminal.state.collectAsStateWithLifecycle()
        HostTerminalItem(terminal.id, terminal.title, state is SessionState.Closed)
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
            Box(
                Modifier.fillMaxSize()
                    .windowInsetsPadding(insets.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top))
                    .then(if (fullScreen) Modifier else Modifier.imePadding()),
            ) { content() }
        }
    }
}

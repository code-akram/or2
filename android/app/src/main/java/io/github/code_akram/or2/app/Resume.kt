package io.github.code_akram.or2.app

import androidx.activity.compose.LocalActivity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.ReconnectOffer
import io.github.code_akram.or2.connection.reconnectOffer
import io.github.code_akram.or2.connection.targetTitle
import io.github.code_akram.or2.connection.terminalClosedStates
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.home.HomeResume
import io.github.code_akram.or2.notify.AgentOpenStart
import io.github.code_akram.or2.notify.AgentPaneKey
import io.github.code_akram.or2.notify.agentOpenStart
import io.github.code_akram.or2.terminal.display

/** How the resume state reads and moves [Or2App]'s navigation. */
internal class AppNavigation(
    /** The stack as it is now (the screen may have moved on since the caller composed). */
    val stack: () -> NavStack,
    val navigate: (NavStack) -> Unit,
    /** A finished activation: show its terminal ([replace]: in place of the terminal on screen), or say why not. */
    val enter: (Activation, replace: Boolean) -> Unit,
)

/** What Home and the notices show of the resume state, and what their taps do. */
internal class ResumeUi(
    /** Home's Resume card: null while nothing can be resumed, or a resume is already on its way. */
    val card: HomeResume?,
    val resume: () -> Unit,
    /** The reconnect chip: the app came back and found these hosts' connections lost. */
    val chip: ReconnectOffer?,
    val acceptChip: (ReconnectOffer) -> Unit,
    val dismissChip: () -> Unit,
)

/** The pending open of [Or2App], kept across recreation (see [PendingOpen.Saver]). */
@Composable
fun rememberPendingOpen(): MutableState<PendingOpen?> =
    rememberSaveable(stateSaver = PendingOpen.Saver) { mutableStateOf<PendingOpen?>(null) }

/**
 * The reattach and resume state of [Or2App]: the last terminal and its Resume card, the one [PendingOpen] (a Resume or
 * an agent notification's tap waiting for its host to connect), the launch's recovery, the terminal remembered while it
 * is shown ([currentTerminal]), and what happens when the app returns (show or reopen the terminal it was left on, the
 * reconnect chip). See contracts.md: reattach, battery and reconnect rules.
 */
@Composable
internal fun rememberResume(
    hosts: List<Host>, terminals: List<ActiveTerminal>, states: Map<Long, HostState>, busy: Boolean, loaded: Boolean,
    currentTerminal: ActiveTerminal?, connections: HostConnections, actions: AppActions, navigation: AppNavigation,
    connect: (List<Host>) -> Unit,
): ResumeUi {
    val activations = connections.activations
    val hostsNow = rememberUpdatedState(hosts)
    val last by actions.reattach.last.collectAsStateWithLifecycle()
    val closedStates by remember(connections) { connections.terminalClosedStates() }.collectAsStateWithLifecycle(emptyMap())
    val connectedHosts = states.filterValues { it is HostState.Connected }.keys
    val reattach = decideReattach(
        last, terminals.map { OpenSession(it.id, it.host.id, it.target, closedStates[it.id] != true) },
        connectedHosts, hosts.map { it.id }.toSet(),
    )
    // Saved state: a rotation (or a restored process) while the connect is pending must not lose the target.
    var pending by rememberPendingOpen()
    val card = when {
        pending is PendingOpen.Resume -> null
        reattach is Reattach.Reopen -> resumeCardOf(reattach.last, hosts)
        reattach is Reattach.Resume -> resumeCardOf(reattach.last, hosts)
        else -> null
    }

    fun hostLabel(id: Long) = hostsNow.value.find { it.id == id }?.label ?: "the host"
    // A terminal reopened or resumed replaces a terminal screen instead of stacking on it.
    fun enterReattached(activation: Activation) = navigation.enter(activation, navigation.stack().current is Destination.Terminal)
    fun reopen(target: LastTerminal, span: String? = null, connectedInThisTap: Boolean = false) =
        activations.launchReopen(target, hostLabel(target.hostId), span, connectedInThisTap, ::enterReattached)
    fun openAgent(pane: AgentPaneKey) {
        actions.message(null)
        activations.launchOpenAgent(pane.hostId, hostLabel(pane.hostId), pane.session, pane.paneId) { navigation.enter(it, false) }
    }
    fun resumeLast() {
        val target = actions.reattach.last.value ?: return
        if (hostsNow.value.none { it.id == target.hostId }) return
        val span = "resume host=${target.hostId}"
        connections.timing.begin(span)
        if (connections.host(target.hostId)?.state?.value is HostState.Connected) reopen(target, span) else pending = PendingOpen.Resume(target)
    }

    // The one pending open: its host connects (its own unlock, once no other runs), then the terminal opens.
    val pendingState = pending?.let { states[it.hostId] }
    LaunchedEffect(pending, pendingState, busy) {
        val open = pending ?: return@LaunchedEffect
        when (pendingStep(pendingState, busy, open.started)) {
            PendingStep.WAIT -> Unit
            PendingStep.ABORT -> pending = null
            PendingStep.CONNECT -> {
                val host = hostsNow.value.find { it.id == open.hostId }
                pending = host?.let { open.started() }
                host?.let { connect(listOf(it)) }
            }
            PendingStep.OPEN -> {
                pending = null
                when (open) {
                    is PendingOpen.Resume -> {
                        val span = "resume host=${open.hostId}"
                        connections.timing.mark(span, "host-connected")
                        // This tap connected the host: AUTO may await `mosh_server()` (one round trip) for the shell.
                        reopen(open.last, span.takeIf(connections.timing::isRunning), connectedInThisTap = true)
                    }
                    is PendingOpen.Agent -> openAgent(open.pane)
                }
            }
        }
    }

    // What the user last had in front of them: remembered while the terminal is connected now (and not being
    // closed: `rememberShown` checks that when it runs). Its state is an input of the effect, and a terminal
    // that closed (the user's close, a remote exit) never becomes the target again when the screen is
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
    // saved destination (OxygenOS drops a killed app from recents, so this is the usual way back): it
    // resumes the same way when the previous process died with sessions open (`SessionMarker`, one-shot
    // per process), and Home's Resume card is what remains when the fingerprint is cancelled. With "Reopen the last
    // terminal on launch" off, neither path resumes: the app starts on Home, Resume card and all.
    var recovered by remember { mutableStateOf(false) }
    LaunchedEffect(loaded) {
        if (!loaded || recovered) return@LaunchedEffect
        recovered = true
        val coldStart = actions.takeColdResume()
        // An agent notification's tap started the app: the user asked for that pane, not the last terminal (the tap is
        // handled once this has run, so a dead terminal screen is left first).
        val tapped = actions.agentOpens.request.value != null
        val top = navigation.stack().current
        val deadTerminalScreen = top is Destination.Terminal && connections.terminal(top.terminalId) == null
        if (deadTerminalScreen) navigation.navigate(NavStack())
        val resumes = resumesOnLaunch(
            actions.reopenLastTerminal(), tapped, deadTerminalScreen, coldStart, actions.reattach.last.value, hostsNow.value,
            connectedHosts,
        )
        if (resumes) resumeLast()
    }

    // An agent notification's tap opens the pane the way an inbox tap does; a host that is not connected connects first
    // (the pending open). After the launch's own recovery (which may leave a dead terminal screen and must not cancel
    // this open).
    val agentOpen by actions.agentOpens.request.collectAsStateWithLifecycle()
    LaunchedEffect(agentOpen, loaded, recovered) {
        if (agentOpen == null || !loaded || !recovered) return@LaunchedEffect
        val pane = actions.agentOpens.take() ?: return@LaunchedEffect
        if (hostsNow.value.none { it.id == pane.hostId }) {
            actions.message("That host no longer exists.")
            return@LaunchedEffect
        }
        if (agentOpenStart(connections.host(pane.hostId)?.state?.value) == AgentOpenStart.OPEN) openAgent(pane)
        else pending = PendingOpen.Agent(pane)
    }

    // The hosts whose lost connection the app offered to restore when it returned: shown as a chip
    // (not a dialog: nothing waits for an answer), for as long as they are still lost. The chip's tap is the usual
    // grouped unlock (one biometric per distinct key) and connect; coming back from a terminal whose connection was
    // lost, its terminal is reopened (or reused, if a mosh session kept it alive) once the host is connected again.
    var offeredIds by remember { mutableStateOf(emptySet<Long>()) }
    var stoppedOnTerminal by rememberSaveable { mutableStateOf(false) }
    fun acceptReconnect(offer: ReconnectOffer) {
        offeredIds = emptySet()
        actions.reattach.last.value?.takeIf { stoppedOnTerminal && offer.hosts.any { host -> host.id == it.hostId } }
            ?.let { pending = PendingOpen.Resume(it, started = true) }
        connect(offer.hosts)
    }
    val chip = remember(offeredIds, hosts, states) {
        if (offeredIds.isEmpty()) null else reconnectOffer(hosts.filter { it.id in offeredIds }, connections.hosts.value)
    }

    // Leaving and returning. The flags are saved state: the foreground service keeps the process alive, so the system
    // can destroy and recreate the activity while it is in the background (memory pressure, "don't keep activities",
    // a long time away), exactly when the connection is most likely to have died. The work itself waits for the stored
    // hosts to be read, which a recreated activity has not yet done.
    val activity = LocalActivity.current
    var returning by rememberSaveable { mutableStateOf(false) }
    var returned by remember { mutableStateOf(false) }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) {
        if (activity?.isChangingConfigurations == true) return@LifecycleEventEffect
        returning = true
        stoppedOnTerminal = navigation.stack().current is Destination.Terminal
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
        offeredIds = reconnectOffer(stored, connections.hosts.value)?.hosts?.map { it.id }?.toSet().orEmpty()
        val liveHosts = connections.hosts.value.filterValues { it.state.value is HostState.Connected }.keys
        val decision = decideReattach(
            actions.reattach.last.value,
            connections.terminals.value.map { OpenSession(it.id, it.host.id, it.target, it.state.value !is SessionState.Closed) },
            liveHosts, stored.map { it.id }.toSet(),
        )
        when {
            !stoppedOnTerminal -> Unit
            // Back through an agent notification: its pane is being opened, not the terminal the app was left on.
            actions.agentOpens.request.value != null || pending is PendingOpen.Agent || activations.pending.value != null -> Unit
            // Shown as it is (nothing is focused), with a full frame first.
            decision is Reattach.Show -> connections.terminal(decision.terminalId)?.let { terminal ->
                activations.cancel()
                enterReattached(Activation.Ready(terminal))
                terminal.handle.value?.let { handle -> runCatching { handle.requestFullFrame() } }
            }
            decision is Reattach.Reopen -> reopen(decision.last)
            else -> {
                // A terminal screen for a terminal that is gone (dismissed, or lost with a process that
                // died) gives way to Home. A terminal that closed stays: its reason and final frame are
                // the user's to read and dismiss, and Home's Resume card is there when it can be resumed.
                val top = navigation.stack().current
                if (top is Destination.Terminal && connections.terminal(top.terminalId) == null) navigation.navigate(NavStack())
            }
        }
    }

    return ResumeUi(card, ::resumeLast, chip, ::acceptReconnect, dismissChip = { offeredIds = emptySet() })
}

/** The Home card for the last terminal: what it was ([targetTitle]) and how it was reached (`SSH` or `Mosh`). */
private fun resumeCardOf(last: LastTerminal, hosts: List<Host>): HomeResume? {
    val host = hosts.find { it.id == last.hostId } ?: return null
    return HomeResume("${host.label}: ${targetTitle(last.target)}", last.transport.display().label)
}

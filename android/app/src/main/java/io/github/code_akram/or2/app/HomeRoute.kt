package io.github.code_akram.or2.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.terminalClosedStates
import io.github.code_akram.or2.connection.transports
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.home.HomeScreen
import io.github.code_akram.or2.home.HomeSession
import io.github.code_akram.or2.home.HostCard
import io.github.code_akram.or2.home.hostAddressLine
import io.github.code_akram.or2.home.hostCardStatus
import io.github.code_akram.or2.home.sessionDetail
import io.github.code_akram.or2.home.tapConnects
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.herdrViews
import io.github.code_akram.or2.inbox.linkStatus
import io.github.code_akram.or2.terminal.TerminalThumbnail
import io.github.code_akram.or2.terminal.display

/**
 * Home, assembled: each stored host's card with its status and its open terminals (live thumbnails), the notices and
 * the Resume card ([resume]), and the session picker over Home for the host in [picker] (a card header's tap sets it;
 * [setPicker] null closes it). A header tap on a host that is not connected starts its connection (the usual unlock),
 * and the sheet shows the progress, then the lists.
 */
@Composable
internal fun HomeRoute(
    hosts: List<Host>, keyCount: Int, terminals: List<ActiveTerminal>, states: Map<Long, HostState>, inbox: InboxState,
    connections: HostConnections, busy: Boolean, unlocking: Set<Long>, actions: AppActions, resume: ResumeUi,
    picker: Long?, setPicker: (Long?) -> Unit,
    connect: (List<Host>) -> Unit, openTerminal: (ActiveHost, TerminalTarget) -> Unit, openAgent: OpenAgent, show: (Long) -> Unit,
    push: (Destination) -> Unit, addHost: () -> Unit, easyPair: () -> Unit, manualHost: () -> Unit,
) {
    val transports by remember(connections) { connections.transports() }.collectAsStateWithLifecycle(emptyMap())
    val herdrViews by remember(connections) { connections.herdrViews() }.collectAsStateWithLifecycle(emptyMap())
    val closedStates by remember(connections) { connections.terminalClosedStates() }.collectAsStateWithLifecycle(emptyMap())
    val batteryCard by actions.battery.card.collectAsStateWithLifecycle()
    val notificationOffer by actions.notifications.visible.collectAsStateWithLifecycle()
    val wakes by actions.wakeStatus.collectAsStateWithLifecycle()
    val blocked = inbox.groups.filter { it.status == AgentStatus.BLOCKED }.flatMap { it.items }
    val blockedByHost = blocked.groupingBy { it.hostId }.eachCount()
    val sessions = remember(terminals, herdrViews, connections, transports, closedStates) {
        terminals.groupBy { it.host.id }.mapValues { (_, list) ->
            list.map { terminal ->
                val herdr = terminal.target as? TerminalTarget.Herdr
                val closed = closedStates[terminal.id] == true
                HomeSession(
                    terminal.id, terminal.title,
                    sessionDetail(herdr?.let { herdrViews[terminal.host.id to it.session] }),
                    (transports[terminal.id] ?: terminal.transport.value).display(),
                    closed = closed, closeAsks = closeAsks(terminal.target, closed),
                ) { thumbnail -> TerminalThumbnail(terminal, connections, thumbnail) }
            }
        }
    }
    val cards = hosts.map { host ->
        val state = states[host.id]
        HostCard(
            host, hostCardStatus(state, host.id in unlocking, blockedByHost[host.id] ?: 0, host.sleeps, host.addresses, wakes[host.id]),
            linkStatus(state, host.sleeps), hostAddressLine(host, state), sessions[host.id].orEmpty(),
        )
    }
    val connectable = cards.filter { it.host.keyId != null && it.link.canConnect }
    val connectedHosts = states.filterValues { it is HostState.Connected }.keys
    HomeScreen(
        hosts = cards, keyCount = keyCount, blocked = blocked.size,
        canConnectAll = connectable.size > 1, busy = busy,
        openPicker = { host ->
            // The picker over Home at once, connecting the host first when it is not (the usual unlock); the sheet
            // shows the progress, then the lists.
            if (tapConnects(host, linkStatus(states[host.id], host.sleeps), busy)) connect(listOf(host))
            setPicker(host.id)
        },
        openSession = { show(it.id) },
        closeSession = { session -> connections.terminal(session.id)?.let(connections.activations::close) },
        addHost = addHost, easyPair = easyPair, manualHost = manualHost,
        editHost = { push(Destination.HostForm(it.id)) },
        connectHost = { connect(listOf(it)) },
        disconnectHost = { connections.disconnect(it.id) },
        deleteHost = actions.deleteHost,
        openInbox = { push(Destination.Inbox) },
        openKeys = { push(Destination.Keys) },
        openAbout = { push(Destination.About) },
        openSettings = { push(Destination.Settings) },
        connectAll = { connect(connectable.map { it.host }) },
        resume = resume.card, onResume = resume.resume,
        batteryCard = batteryCard, allowBattery = actions.requestBatteryExemption, dismissBattery = actions.battery::dismissCard,
        // In context: offered only while a host is connected, which is when the notification would show.
        notificationCard = notificationOffer && connectedHosts.isNotEmpty(),
        allowNotifications = actions.allowNotifications, dismissNotifications = actions.notifications::dismiss,
        wakeHost = actions.wake,
    )
    val pickerHost = picker?.let { id -> hosts.find { it.id == id } }
    if (pickerHost != null) {
        HomePickerSheet(
            pickerHost, terminals, connections, unlocking = pickerHost.id in unlocking, busy = busy,
            openTerminal = openTerminal, openAgent = openAgent,
            connect = { connect(listOf(pickerHost)) },
            edit = { push(Destination.HostForm(pickerHost.id)) },
            dismiss = { setPicker(null) },
        )
    }
}

package io.github.code_akram.or2.app

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.host.HostScreen
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.hosts.HostsScreen
import io.github.code_akram.or2.inbox.InboxScreen
import io.github.code_akram.or2.inbox.InboxState
import io.github.code_akram.or2.inbox.LinkStatus
import io.github.code_akram.or2.inbox.dialogForOtherHost
import io.github.code_akram.or2.inbox.inbox
import io.github.code_akram.or2.inbox.linkStatuses
import io.github.code_akram.or2.inbox.pendingHostKeys
import io.github.code_akram.or2.keys.KeysScreen
import io.github.code_akram.or2.session.HostTrustDialog
import io.github.code_akram.or2.session.OpenTerminalsRow
import io.github.code_akram.or2.session.SessionScreen
import io.github.code_akram.or2.session.hostErrorMessage
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
)

/** The whole UI: the inbox is the start destination; hosts, keys, a host and terminals push on it. */
@Composable
fun Or2App(
    hosts: List<Host>, keys: List<KeyRecord>, message: String?, busy: Boolean,
    connections: HostConnections, actions: AppActions,
) {
    var saved by rememberSaveable { mutableStateOf(NavStack().encode()) }
    val nav = remember(saved) { NavStack.decode(saved) }
    fun navigate(next: NavStack) { saved = next.encode() }
    val terminals by connections.terminals.collectAsStateWithLifecycle()
    val current = nav.current
    val currentTerminal = (current as? Destination.Terminal)?.let { destination -> terminals.find { it.id == destination.terminalId } }
    BackHandler(enabled = nav.back() != null) { nav.back()?.let(::navigate) }

    val hostsNow = rememberUpdatedState(hosts)
    val inbox by remember(connections) { connections.inbox(snapshotFlow { hostsNow.value }) }
        .collectAsStateWithLifecycle(InboxState(emptyList(), emptyList()))
    val links by remember(connections) { connections.linkStatuses() }.collectAsStateWithLifecycle(emptyMap())
    val pending by remember(connections) { connections.pendingHostKeys() }.collectAsStateWithLifecycle(emptyList())

    fun openTerminal(active: ActiveHost, target: TerminalTarget) {
        try {
            val terminal = connections.openTerminal(active, target)
            actions.message(null)
            navigate(nav.push(Destination.Terminal(terminal.id)))
        } catch (error: CancellationException) {
            throw error
        } catch (error: HostException) {
            actions.message(hostErrorMessage(error))
        }
    }

    val terminalConnected = key(currentTerminal) { currentTerminal?.hasConnected?.collectAsStateWithLifecycle()?.value == true }
    AppScaffold(nav.tab, terminalVisible = currentTerminal != null && terminalConnected,
        fullScreen = current is Destination.Terminal, selectTab = { navigate(nav.top(it)) }) {
        message?.let {
            Text(it, color = MaterialTheme.colorScheme.error)
            TextButton(onClick = { actions.message(null) }) { Text("Dismiss") }
        }
        if (busy) Text("Waiting for authentication or operation…")
        val openTerminalsRow: @Composable () -> Unit = {
            OpenTerminalsRow(terminals) { navigate(nav.push(Destination.Terminal(it.id))) }
        }
        when (current) {
            Destination.Inbox -> InboxScreen(
                inbox, busy,
                connectAll = { actions.connect(inbox.hosts.map { it.host }) },
                connect = { actions.connect(listOf(it)) },
                openHost = { navigate(nav.push(Destination.HostPage(it.id))) },
                openAgent = { item ->
                    val active = connections.host(item.hostId)
                    val target = TerminalTarget.Herdr(item.session, item.paneId)
                    // Tapping the same agent again returns to its open terminal.
                    val open = connections.findOpenTerminal(item.hostId, target)
                    if (open != null) navigate(nav.push(Destination.Terminal(open.id)))
                    else if (active == null) actions.message("${item.hostLabel} is no longer connected.")
                    else openTerminal(active, target)
                },
                openTerminals = openTerminalsRow,
            )
            Destination.Hosts -> HostsScreen(
                hosts, keys, busy,
                save = actions.saveHost, delete = actions.deleteHost,
                connect = { host ->
                    actions.connect(listOf(host))
                    // Host-key prompts live on the host screen.
                    navigate(nav.push(Destination.HostPage(host.id)))
                },
                open = { navigate(nav.push(Destination.HostPage(it.id))) },
                link = { links[it.id] ?: LinkStatus.NOT_CONNECTED },
            )
            Destination.Keys -> KeysScreen(keys, busy, actions.generateKey, actions.importKey, actions.deleteKey)
            is Destination.HostPage -> HostPage(current.hostId, hosts, connections, busy, actions, ::openTerminal, openTerminalsRow)
            is Destination.Terminal -> SessionScreen(
                connections, currentTerminal, terminals,
                back = { nav.back()?.let(::navigate) ?: navigate(nav.top(Destination.Inbox)) },
                select = { navigate(nav.replaceTop(Destination.Terminal(it.id))) },
            )
        }
    }
    // A prompt for a host whose screen is not showing still needs an answer, one dialog at a time:
    // the shown host's own screen already has its dialog.
    pending.dialogForOtherHost((current as? Destination.HostPage)?.hostId)?.let { (active, prompt) ->
        HostTrustDialog(prompt, busy, { actions.approve(active, prompt) }, { actions.reject(active) },
            hostLabel = hosts.find { it.id == active.host.id }?.label ?: active.host.label)
    }
}

@Composable
private fun HostPage(
    hostId: Long, hosts: List<Host>, connections: HostConnections, busy: Boolean, actions: AppActions,
    openTerminal: (ActiveHost, TerminalTarget) -> Unit, openTerminals: @Composable () -> Unit,
) {
    val host = hosts.find { it.id == hostId }
    if (host == null) {
        Text("This host no longer exists.")
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
            openTerminals = openTerminals,
        )
        // Refreshing re-probes capabilities (new herdr sessions) as well as the tmux list.
        LaunchedEffect(active, refreshes) {
            if (active != null && refreshes > 0) connections.refresh(active)
        }
    }
}

/** Shared navigation chrome; a connected terminal uses only system-bar/cutout insets, not form padding. */
@Composable
fun AppScaffold(
    tab: Destination, terminalVisible: Boolean, fullScreen: Boolean, selectTab: (Destination) -> Unit,
    content: @Composable () -> Unit,
) {
    val activity = LocalActivity.current
    val view = LocalView.current
    SideEffect {
        activity?.let {
            val controller = WindowCompat.getInsetsController(it.window, view)
            controller.isAppearanceLightStatusBars = !terminalVisible
            controller.isAppearanceLightNavigationBars = !terminalVisible
        }
    }
    MaterialTheme(colorScheme = if (terminalVisible) darkColorScheme(background = Color.Black, surface = Color(0xff101010)) else lightColorScheme()) {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(
                modifier = Modifier.fillMaxSize()
                    .windowInsetsPadding(WindowInsets.systemBars.union(WindowInsets.displayCutout))
                    .then(if (fullScreen) Modifier else Modifier.imePadding())
                    .then(if (terminalVisible) Modifier else Modifier.padding(16.dp)),
                verticalArrangement = Arrangement.spacedBy(if (terminalVisible) 0.dp else 12.dp),
            ) {
                if (!terminalVisible) {
                    Text("or2", style = MaterialTheme.typography.headlineMedium)
                    Row {
                        listOf(Destination.Inbox to "Inbox", Destination.Hosts to "Hosts", Destination.Keys to "Keys").forEach { (destination, name) ->
                            TextButton(onClick = { selectTab(destination) }) { Text(if (tab == destination) "• $name" else name) }
                        }
                    }
                }
                content()
            }
        }
    }
}

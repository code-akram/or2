package io.github.code_akram.or2.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.UdpVerdict
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.host.GateAction
import io.github.code_akram.or2.host.OpenSessions
import io.github.code_akram.or2.host.SessionPickerSheet
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.host.pickerGate
import io.github.code_akram.or2.session.hostErrorMessage
import kotlinx.coroutines.CancellationException

/** What a host's session picker shows, read from its live connection, and where its choices go. */
internal class PickerSource(
    val state: HostState?,
    val caps: HostCapabilities?,
    val capsError: String?,
    val tmux: TmuxList,
    val udpBlocked: Boolean,
    /** Re-probes capabilities (new herdr sessions) and lists tmux again. */
    val refresh: () -> Unit,
    /** Opens [TerminalTarget] on the connection (the app's `openTerminal`: `TerminalActivations.launchOpen`). */
    val open: (TerminalTarget) -> Unit,
)

/**
 * The [PickerSource] of [active] (null: no connection yet). The tmux list is read once the host is connected and on
 * each refresh; a new connection starts from `Loading`.
 */
@Composable
internal fun pickerSource(
    active: ActiveHost?, connections: HostConnections, openTerminal: (ActiveHost, TerminalTarget) -> Unit,
): PickerSource {
    val state = active?.state?.collectAsStateWithLifecycle()?.value
    val caps = active?.capabilities?.collectAsStateWithLifecycle()?.value
    val capsError = active?.capabilitiesError?.collectAsStateWithLifecycle()?.value
    val verdict = active?.udpVerdict?.collectAsStateWithLifecycle()?.value
    val moshServer = active?.moshServer?.collectAsStateWithLifecycle()?.value
    var refreshes by remember(active) { mutableIntStateOf(0) }
    var tmux by remember(active) { mutableStateOf<TmuxList>(TmuxList.Loading) }
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
    // Refreshing re-probes capabilities (new herdr sessions) as well as the tmux list.
    LaunchedEffect(active, refreshes) {
        if (active != null && refreshes > 0) connections.refresh(active)
    }
    return PickerSource(
        state, caps, capsError, tmux,
        // A host without mosh-server is not a UDP problem: only say so when mosh is there and blocked.
        udpBlocked = verdict == UdpVerdict.BLOCKED && (moshServer == null || moshServer.path != null),
        refresh = { refreshes++ },
        open = { target -> active?.let { openTerminal(it, target) } },
    )
}

/**
 * The session picker over Home, from a host card's header. It shows at once: while [host] is not connected it shows
 * the host's progress ([unlocking]: waiting on the biometric unlock), or why it is not connected with Retry or Connect
 * ([connect]) or, for a host without a key, "Select a key" ([edit]); the lists follow once the host is connected.
 * Choosing a target closes it ([dismiss]) and opens the terminal, or switches to the one already open on that session
 * (its row is marked `Open`).
 */
@Composable
internal fun HomePickerSheet(
    host: Host, terminals: List<ActiveTerminal>, connections: HostConnections, unlocking: Boolean, busy: Boolean,
    openTerminal: (ActiveHost, TerminalTarget) -> Unit, connect: () -> Unit, edit: () -> Unit, dismiss: () -> Unit,
) {
    // No key(active) here: a connection that starts while the sheet is up must not close and reopen the sheet.
    val active = connections.hosts.collectAsStateWithLifecycle().value[host.id]
    val source = pickerSource(active, connections, openTerminal)
    SessionPickerSheet(
        source.caps, source.capsError, source.tmux, openSessionsOf(terminals.filter { it.host.id == host.id }),
        openShell = { dismiss(); source.open(TerminalTarget.Shell) },
        openTmux = { name -> dismiss(); source.open(TerminalTarget.Tmux(name)) },
        openHerdr = { session -> dismiss(); source.open(TerminalTarget.Herdr(session, null)) },
        refresh = source.refresh, dismiss = dismiss,
        gate = pickerGate(host, source.state, unlocking, busy),
        title = host.label,
        udpBlocked = source.udpBlocked,
        gateAction = { action ->
            when (action) {
                GateAction.SELECT_KEY -> { dismiss(); edit() }
                GateAction.RETRY, GateAction.CONNECT -> if (!busy) connect()
            }
        },
    )
}

/** The sessions of one host's [terminals] that are open now (a closed terminal marks nothing): the picker's `Open` rows. */
@Composable
private fun openSessionsOf(terminals: List<ActiveTerminal>): OpenSessions {
    val open = terminals.filter { terminal ->
        key(terminal.id) { terminal.state.collectAsStateWithLifecycle().value !is SessionState.Closed }
    }
    return OpenSessions.of(open.map { it.target })
}

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
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.host.DirectoryList
import io.github.code_akram.or2.host.GateAction
import io.github.code_akram.or2.host.OpenSessions
import io.github.code_akram.or2.host.SessionPickerSheet
import io.github.code_akram.or2.host.TmuxList
import io.github.code_akram.or2.host.pickerGate
import io.github.code_akram.or2.inbox.herdrViews
import io.github.code_akram.or2.session.hostErrorMessage

/**
 * An explicit agent request (an Inbox row, an agent in the picker): its pane is focused, then its session's terminal is
 * shown (`TerminalActivations.launchOpenAgent`, with the progress notice).
 */
internal typealias OpenAgent = (hostId: Long, hostLabel: String, session: String?, paneId: String) -> Unit

/** What a host's session picker shows, read from its live connection, and where its choices go. */
internal class PickerSource(
    val state: HostState?,
    val caps: HostCapabilities?,
    val capsError: String?,
    val tmux: TmuxList,
    /** A tmux re-read (after the first answer) or a Refresh's re-probe is running. */
    val refreshing: Boolean,
    val udpBlocked: Boolean,
    /** The host's live herdr views by session (null: the default session), the Inbox's source. */
    val herdrViews: Map<String?, HerdrView>,
    /** Re-probes capabilities (new herdr sessions) and lists tmux again. */
    val refresh: () -> Unit,
    /** The tmux tab is shown: its list is read again, unless a read is running already. */
    val tmuxShown: () -> Unit,
    /** Opens [TerminalTarget] on the connection (the app's `openTerminal`: `TerminalActivations.launchOpen`). */
    val open: (TerminalTarget) -> Unit,
    val directories: DirectoryList,
    val readingDirectories: Boolean,
    val refreshDirectories: () -> Unit,
)

/**
 * The [PickerSource] of [active] (null: no connection yet). The tmux list is read once the host is connected (each
 * time the picker opens: the source lives as long as the sheet), again whenever the tmux tab is shown, and on each
 * refresh; until its first answer it is `Loading`, and a later read keeps the old list while it runs.
 */
@Composable
internal fun pickerSource(
    active: ActiveHost?, connections: HostConnections, openTerminal: (ActiveHost, TerminalTarget) -> Unit,
): PickerSource {
    val state = active?.state?.collectAsStateWithLifecycle()?.value
    val directories = active?.directories?.collectAsStateWithLifecycle()?.value ?: DirectoryList.Loading
    val readingDirectories = active?.readingDirectories?.collectAsStateWithLifecycle()?.value ?: false
    var directoryReads by remember(active) { mutableIntStateOf(0) }
    LaunchedEffect(active, directoryReads) {
        if (active != null && directoryReads > 0) connections.refreshDirectories(active)
    }
    val caps = active?.capabilities?.collectAsStateWithLifecycle()?.value
    val capsError = active?.capabilitiesError?.collectAsStateWithLifecycle()?.value
    val verdict = active?.udpVerdict?.collectAsStateWithLifecycle()?.value
    val moshServer = active?.moshServer?.collectAsStateWithLifecycle()?.value
    val views by remember(connections) { connections.herdrViews() }.collectAsStateWithLifecycle(emptyMap())
    var refreshes by remember(active) { mutableIntStateOf(0) }
    var reads by remember(active) { mutableIntStateOf(0) }
    var tmux by remember(active) { mutableStateOf<TmuxList>(TmuxList.Loading) }
    // The running read and probe, each by its own token: a cancelled one's cleanup never clears its successor's.
    var reading by remember(active) { mutableStateOf<Any?>(null) }
    var probing by remember(active) { mutableStateOf<Any?>(null) }
    val connected = state is HostState.Connected
    LaunchedEffect(active, connected, reads) {
        if (active == null || !connected) return@LaunchedEffect
        val token = Any()
        reading = token
        try {
            tmux = try {
                TmuxList.Loaded(connections.listTmuxSessions(active))
            } catch (error: HostException) {
                TmuxList.Failed(hostErrorMessage(error))
            }
        } finally {
            if (reading === token) reading = null
        }
    }
    // Refreshing re-probes capabilities (new herdr sessions) as well as the tmux list.
    LaunchedEffect(active, refreshes) {
        if (active == null || refreshes == 0) return@LaunchedEffect
        val token = Any()
        probing = token
        try {
            connections.refresh(active)
        } finally {
            if (probing === token) probing = null
        }
    }
    val hostId = active?.host?.id
    return PickerSource(
        state, caps, capsError, tmux,
        // The first read shows in place of the list instead.
        refreshing = probing != null || (reading != null && tmux !is TmuxList.Loading),
        // A host without mosh-server is not a UDP problem: only say so when mosh is there and blocked.
        udpBlocked = verdict == UdpVerdict.BLOCKED && (moshServer == null || moshServer.path != null),
        herdrViews = views.filterKeys { it.first == hostId }.mapKeys { it.key.second },
        refresh = { refreshes++; reads++ },
        tmuxShown = { if (reading == null) reads++ },
        open = { target -> active?.let { openTerminal(it, target) } },
        directories = directories,
        readingDirectories = readingDirectories,
        refreshDirectories = { if (!readingDirectories) directoryReads++ },
    )
}

/**
 * Where the picker's choices on [host] go: each closes the sheet first ([dismiss]). A target opens on the connection
 * ([open]: an open session's terminal is shown as it is); an agent takes the Inbox tap's path ([openAgent]).
 */
internal class PickerChoices(
    private val host: Host,
    private val open: (TerminalTarget) -> Unit,
    private val openAgent: OpenAgent,
    private val dismiss: () -> Unit,
) {
    fun shell() = choose { open(TerminalTarget.Shell) }

    fun directory(path: String) = choose { open(TerminalTarget.ShellIn(path)) }

    fun tmux(name: String) = choose { open(TerminalTarget.Tmux(name)) }

    /** A herdr session's `Whole session` row (or its single row): its terminal, opened without a pane. */
    fun herdr(session: String?) = choose { open(TerminalTarget.Herdr(session, null)) }

    /** An agent under its session: its pane is focused and the session's open terminal reused. */
    fun agent(session: String?, paneId: String) = choose { openAgent(host.id, host.label, session, paneId) }

    private inline fun choose(then: () -> Unit) {
        dismiss()
        then()
    }
}

/**
 * The session picker over Home, from a host card's header. It shows at once: while [host] is not connected it shows
 * the host's progress ([unlocking]: waiting on the biometric unlock), or why it is not connected with Retry or Connect
 * ([connect]) or, for a host without a key, "Select a key" ([edit]); the lists follow once the host is connected.
 * Choosing a target closes it ([dismiss]) and opens the terminal, or switches to the one already open on that session
 * (its row is marked `Open`); choosing an agent opens it as the Inbox does ([openAgent]).
 */
@Composable
internal fun HomePickerSheet(
    host: Host, terminals: List<ActiveTerminal>, connections: HostConnections, unlocking: Boolean, busy: Boolean,
    openTerminal: (ActiveHost, TerminalTarget) -> Unit, openAgent: OpenAgent, connect: () -> Unit, edit: () -> Unit,
    dismiss: () -> Unit,
) {
    // No key(active) here: a connection that starts while the sheet is up must not close and reopen the sheet.
    val active = connections.hosts.collectAsStateWithLifecycle().value[host.id]
    val source = pickerSource(active, connections, openTerminal)
    val choices = PickerChoices(host, source.open, openAgent, dismiss)
    SessionPickerSheet(
        source.caps, source.capsError, source.tmux, openSessionsOf(terminals.filter { it.host.id == host.id }),
        openShell = choices::shell, openTmux = choices::tmux, openHerdr = choices::herdr,
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
        herdrViews = source.herdrViews, openAgent = choices::agent,
        refreshing = source.refreshing, tmuxShown = source.tmuxShown,
        directories = source.directories, openDirectory = choices::directory,
        refreshDirectories = source.refreshDirectories, readingDirectories = source.readingDirectories,
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

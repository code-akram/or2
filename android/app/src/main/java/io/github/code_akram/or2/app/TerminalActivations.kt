package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.targetTitle
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.session.hostErrorMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** The outcome of getting a terminal ready to be shown. */
sealed interface Activation {
    /** Safe to navigate to: its pane is focused (or it needs no focus). */
    data class Ready(val terminal: ActiveTerminal) : Activation

    /** Do not navigate: showing the terminal could show another pane, or there is none. */
    data class Failed(val message: String) : Activation
}

/**
 * Every way into a terminal that needs work first: an agent asked for explicitly (the inbox, a notification), a
 * picker choice and a reattach. One herdr session has one terminal per host: every herdr open reuses the one already
 * open on that session, whatever pane it was opened on ([reusable]).
 *
 * Only an explicit agent request focuses a pane ([openAgent]): herdr's focus is shared state, so that path awaits
 * `focus_herdr_pane` before the terminal is shown, and shows nothing for a pane that has gone (`PaneNotFound`) or
 * that could not be focused. An open terminal activated again (the Terminals sheet, a Home thumbnail, a picker row
 * marked `Open`, a reattach) is shown as it is: nothing is focused, so the app navigates to it directly.
 *
 * The suspend functions return an [Activation]; the `launch*` variants run one at a time (a newer
 * request supersedes the older wait; a focus already sent still happens), expose [pending] for
 * in-place progress and call `done` on the holder's dispatcher. [cancel] drops a wait the user
 * navigated away from.
 */
class TerminalActivations(private val connections: HostConnections, private val scope: CoroutineScope) {
    private val mutablePending = MutableStateFlow<String?>(null)

    /** What is being waited for, for example `Focusing Fixture: herdr`; null when idle. */
    val pending: StateFlow<String?> = mutablePending.asStateFlow()
    private var job: Job? = null
    private var generation = 0

    /**
     * An agent tapped in the inbox or a notification: its pane is focused, then the terminal already open on its
     * herdr session is shown ([reusable], whatever pane it was opened on), or a new one opens while the pane is
     * being focused (they **start together**: the terminal's own focus joins the one in flight, Rust shares it, and
     * the wait ends when both are done). A pane that cannot be focused (it vanished) leaves no terminal: the one
     * that was opened is dismissed, and so is one whose wait was cancelled.
     */
    suspend fun openAgent(hostId: Long, hostLabel: String, session: String?, paneId: String): Activation {
        return activate(hostId, hostLabel, TerminalTarget.Herdr(session, paneId), "tap host=$hostId pane=$paneId", begin = true,
            focusReused = true)
    }

    /**
     * Reattach: reopens the terminal the user last had on a connected host. An open terminal for the same target (a
     * herdr one: on the same session) is shown as it is; otherwise a new one opens on the target (a herdr pane
     * target awaits its own focus with it, as an agent's does). The transport is chosen afresh by the host's
     * preference; what the target ran over before is not carried along. [connectedInThisTap] (a Resume that had to
     * connect the host first) lets AUTO await `mosh_server()`, one round trip, so a shell can still choose mosh (see
     * [HostConnections.awaitTransportChoice]). [span] is the timing path this belongs to (a Resume's own, else a
     * `reopen` span of its own).
     */
    suspend fun reopen(last: LastTerminal, hostLabel: String, span: String? = null, connectedInThisTap: Boolean = false): Activation {
        return activate(last.hostId, hostLabel, last.target, span ?: "reopen host=${last.hostId}", begin = span == null,
            focusReused = false, connectedInThisTap)
    }

    /**
     * The one path of [openAgent] and [reopen]: the terminal for [target] on [hostId]'s connection. An open one is
     * reused ([reusable]; its pane focused first only when [focusReused]); otherwise a new one is opened while its
     * pane (if any) is being focused. Timing marks go to [span], which this starts when [begin]. What the focus or the
     * open threw is the failure's message, after dismissing a terminal it had opened.
     */
    private suspend fun activate(
        hostId: Long, hostLabel: String, target: TerminalTarget, span: String, begin: Boolean, focusReused: Boolean,
        connectedInThisTap: Boolean = false,
    ): Activation {
        val active = connections.host(hostId) ?: return Activation.Failed("$hostLabel is no longer connected.")
        val timing = connections.timing
        if (begin) timing.begin(span)
        val herdr = target as? TerminalTarget.Herdr
        val paneId = herdr?.paneId
        var opened: ActiveTerminal? = null
        try {
            reusable(hostId, target)?.let { existing ->
                if (focusReused && herdr != null && paneId != null) {
                    connections.focusHerdrPane(active, herdr.session, paneId)
                    timing.mark(span, "focused")
                }
                timing.watchTerminal(existing.id, span)
                return Activation.Ready(existing)
            }
            return Activation.Ready(coroutineScope {
                val focus = if (herdr != null && paneId != null) async {
                    connections.focusHerdrPane(active, herdr.session, paneId)
                    timing.mark(span, "focused")
                } else null
                connections.awaitTransportChoice(active, connectedInThisTap)
                val terminal = connections.openTerminal(active, target)
                opened = terminal
                timing.watchTerminal(terminal.id, span)
                focus?.await()
                terminal
            })
        } catch (error: CancellationException) {
            opened?.let(connections::dismissTerminal)
            throw error
        } catch (error: Exception) {
            opened?.let(connections::dismissTerminal)
            return Activation.Failed(focusMessage(error))
        }
    }

    /**
     * The one reuse rule of every open (picker, inbox, notification, reattach): the terminal already open on [hostId]
     * that [target] brings to the front instead of opening a second one, or null to open a new one. A herdr target,
     * with a pane or without, reuses the open herdr terminal on its session whatever pane that was opened on, one
     * opened without a pane first: herdr's focus is shared, so a second client on the same session would only show the
     * same focused pane. A tmux session reuses the open terminal on that session; a shell never. A closed terminal,
     * or one being closed, is never reused ([HostConnections.openTerminals]); duplicates opened before this rule are
     * left alone.
     */
    private fun reusable(hostId: Long, target: TerminalTarget): ActiveTerminal? = when (target) {
        TerminalTarget.Shell -> null
        is TerminalTarget.Herdr -> {
            val open = connections.openTerminals(hostId) { it is TerminalTarget.Herdr && it.session == target.session }
            open.firstOrNull { (it.target as TerminalTarget.Herdr).paneId == null } ?: open.firstOrNull()
        }
        is TerminalTarget.Tmux -> connections.findOpenTerminal(hostId, target)
    }

    /**
     * A choice in the session picker (shell, tmux, a herdr session) on a connected host. A target already open there
     * is brought to the front as it is ([reusable]: the row marked `Open`); anything else opens at once, never
     * waiting for the probe or for UDP (see [HostConnections.openTerminal]).
     */
    suspend fun open(active: ActiveHost, target: TerminalTarget): Activation {
        reusable(active.host.id, target)?.let { return Activation.Ready(it) }
        try {
            return Activation.Ready(connections.openTerminal(active, target))
        } catch (error: CancellationException) {
            throw error
        } catch (error: HostException) {
            return Activation.Failed(hostErrorMessage(error))
        }
    }

    fun launchReopen(last: LastTerminal, hostLabel: String, span: String? = null, connectedInThisTap: Boolean = false, done: (Activation) -> Unit) =
        launch("Resuming $hostLabel: ${targetTitle(last.target)}", done) { reopen(last, hostLabel, span, connectedInThisTap) }

    fun launchOpen(active: ActiveHost, target: TerminalTarget, done: (Activation) -> Unit) =
        launch("Opening ${active.host.label}: ${targetTitle(target)}", done) { open(active, target) }

    fun launchOpenAgent(hostId: Long, hostLabel: String, session: String?, paneId: String, done: (Activation) -> Unit) =
        launch("Focusing $hostLabel: ${targetTitle(TerminalTarget.Herdr(session, paneId))}", done) {
            openAgent(hostId, hostLabel, session, paneId)
        }

    /**
     * The `×` of a terminal (Home's thumbnails, the Terminals sheet) and Ctrl+Shift+W: an open terminal is disconnected
     * (`disconnectTerminal`, so Rust stops its mosh server and the ledger is cleared on its `Closed`) and dismissed
     * together, a closed one only dismissed. A tmux or herdr session keeps running on the host: only or2's view of it
     * ends. Whether the user is asked first is [closeAsks].
     */
    fun close(terminal: ActiveTerminal) {
        if (terminal.state.value !is SessionState.Closed) connections.disconnectTerminal(terminal)
        connections.dismissTerminal(terminal)
    }

    /** Drops the wait in progress, if any, without calling its `done`. */
    fun cancel() {
        generation++
        job?.cancel()
        job = null
        mutablePending.value = null
    }

    private fun launch(label: String, done: (Activation) -> Unit, block: suspend () -> Activation) {
        job?.cancel()
        val mine = ++generation
        mutablePending.value = label
        job = scope.launch {
            try {
                val result = block()
                if (mine == generation) {
                    mutablePending.value = null
                    job = null
                    done(result)
                }
            } finally {
                if (mine == generation) mutablePending.value = null
            }
        }
    }
}

/**
 * Whether closing a terminal asks first: only an open shell, whose close ends the shell and the programs running in
 * it ([CLOSE_SHELL_TITLE]). A tmux or herdr terminal closes in one tap (the session keeps running on the host), and
 * a terminal that has already closed has nothing left to end.
 */
fun closeAsks(target: TerminalTarget, closed: Boolean): Boolean = target == TerminalTarget.Shell && !closed

/** The confirmation of closing an open shell ([closeAsks]). */
const val CLOSE_SHELL_TITLE = "Close shell?"
const val CLOSE_SHELL_TEXT = "Programs running in it end."

/** What the user reads when a pane could not be focused: the agent is gone, or the error. */
fun focusMessage(error: Exception): String = when (error) {
    is HostException.PaneNotFound -> hostErrorMessage(error)
    is HostException.CommandFailed -> "Could not focus the agent's pane: ${error.reason}"
    is HostException -> hostErrorMessage(error)
    else -> "Could not focus the agent's pane: ${error.message ?: error::class.simpleName}"
}

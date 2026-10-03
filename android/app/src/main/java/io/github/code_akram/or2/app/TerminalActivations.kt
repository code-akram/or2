package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.ActiveHost
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.targetTitle
import io.github.code_akram.or2.ffi.HostException
import io.github.code_akram.or2.ffi.HostState
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

/** The outcome of getting an agent terminal ready to be shown. */
sealed interface Activation {
    /** Safe to navigate to: its pane is focused (or it needs no focus). */
    data class Ready(val terminal: ActiveTerminal) : Activation

    /** Do not navigate: showing the terminal could show another pane, or there is none. */
    data class Failed(val message: String) : Activation
}

/**
 * Every way into a terminal that runs `herdr` for one pane (`TerminalTarget.Herdr` with a pane id):
 * the inbox, a notification, the session switcher, a Home thumbnail and the host screen's open list. herdr's
 * focus is shared state, so a terminal that was already open shows whichever pane is focused
 * *now*; each of these paths therefore awaits `focus_herdr_pane` before the terminal is shown, and
 * shows nothing for a pane that has gone (`PaneNotFound`) or that could not be focused. For the same
 * reason a herdr session has one terminal per host: every herdr open (picker, inbox, notification,
 * reattach) reuses the one already open on that session, whatever pane it was opened on ([openFor]).
 *
 * The suspend functions return an [Activation]; the `launch*` variants run one at a time (a newer
 * request supersedes the older wait; a focus already sent still happens), expose [pending] for
 * in-place progress and call `done` on the holder's dispatcher. [cancel] drops a wait the user
 * navigated away from.
 */
class TerminalActivations(private val connections: HostConnections, private val scope: CoroutineScope) {
    private val mutablePending = MutableStateFlow<String?>(null)

    /** What is being waited for, for example `Focusing Fixture: herdr w1:p1`; null when idle. */
    val pending: StateFlow<String?> = mutablePending.asStateFlow()
    private var job: Job? = null
    private var generation = 0

    /**
     * An agent tapped in the inbox or a notification: reuses the terminal already open on its herdr session
     * ([openFor], whatever pane it was opened on, after focusing this one) or opens a new one. A new terminal and the pane focus **start together**: the terminal's own
     * focus joins the one in flight (Rust shares it), and the wait ends when both are done. A pane that
     * cannot be focused (it vanished) leaves no terminal: the one that was opened is dismissed, and so is
     * one whose wait was cancelled.
     */
    suspend fun openAgent(hostId: Long, hostLabel: String, session: String?, paneId: String): Activation {
        val target = TerminalTarget.Herdr(session, paneId)
        val active = connections.host(hostId) ?: return Activation.Failed("$hostLabel is no longer connected.")
        val span = "tap host=$hostId pane=$paneId"
        connections.timing.begin(span)
        try {
            return Activation.Ready(openOrReuse(active, hostId, target, span))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            return Activation.Failed(focusMessage(error))
        }
    }

    /**
     * The terminal for [target] on [active]: an open one is reused ([openFor]; a target's herdr pane is
     * focused first); otherwise a new one is opened while its pane (if any) is being focused. Timing marks go to
     * [span]. Throws what the focus or the open threw, after dismissing a terminal it had opened.
     */
    private suspend fun openOrReuse(
        active: ActiveHost, hostId: Long, target: TerminalTarget, span: String, connectedInThisTap: Boolean = false,
    ): ActiveTerminal {
        val timing = connections.timing
        val herdr = target as? TerminalTarget.Herdr
        val paneId = herdr?.paneId
        openFor(hostId, target)?.let { existing ->
            if (herdr != null && paneId != null) {
                connections.focusHerdrPane(active, herdr.session, paneId)
                timing.mark(span, "focused")
            }
            timing.watchTerminal(existing.id, span)
            return existing
        }
        var opened: ActiveTerminal? = null
        try {
            return coroutineScope {
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
            }
        } catch (error: Throwable) {
            opened?.let(connections::dismissTerminal)
            throw error
        }
    }

    /**
     * The session picker's reuse rule (the host screen's picker and the one over Home): the terminal already open on
     * [hostId] that a choice of [target] brings to the front instead of opening a second one, or null to open a new
     * one. A tmux session reuses the open terminal on that session; a herdr session or pane the open herdr terminal
     * on that session ([openFor]); a shell never.
     */
    fun reusable(hostId: Long, target: TerminalTarget): ActiveTerminal? =
        if (target == TerminalTarget.Shell) null else openFor(hostId, target)

    /**
     * The one reuse rule of every open (picker, inbox, notification, reattach): the terminal already open on [hostId]
     * for [target], or null. A herdr target, with a pane or without, reuses the open herdr terminal on its session
     * whatever pane that was opened on, one opened without a pane first: herdr's focus is shared, so a second client
     * on the same session would only show the same focused pane. Anything else reuses a terminal on exactly that
     * target. A closed terminal, or one being closed, is never reused ([HostConnections.openTerminals]); duplicates
     * opened before this rule are left alone.
     */
    private fun openFor(hostId: Long, target: TerminalTarget): ActiveTerminal? = when (target) {
        is TerminalTarget.Herdr -> {
            val open = connections.openTerminals(hostId) { it is TerminalTarget.Herdr && it.session == target.session }
            open.firstOrNull { (it.target as TerminalTarget.Herdr).paneId == null } ?: open.firstOrNull()
        }
        else -> connections.findOpenTerminal(hostId, target)
    }

    /**
     * A choice in the session picker (shell, tmux, a herdr session) on a connected host. A target already open there
     * is brought to the front ([reusable]; a herdr pane terminal is focused again first, as from the switcher); a
     * herdr pane goes the inbox tap's way ([openAgent]); anything else opens at once, never waiting for the probe or
     * for UDP (see [HostConnections.openTerminal]).
     */
    suspend fun open(active: ActiveHost, target: TerminalTarget): Activation {
        val pane = (target as? TerminalTarget.Herdr)?.paneId
        if (target is TerminalTarget.Herdr && pane != null) return openAgent(active.host.id, active.host.label, target.session, pane)
        reusable(active.host.id, target)?.let { return reuse(it) }
        try {
            return Activation.Ready(connections.openTerminal(active, target))
        } catch (error: CancellationException) {
            throw error
        } catch (error: HostException) {
            return Activation.Failed(hostErrorMessage(error))
        }
    }

    /**
     * An open terminal is activated again (switcher, Home thumbnail, recent list). Only a terminal
     * for one herdr pane needs the focus; a closed one shows its final frame and sends nothing, and
     * so does one whose host connection is not up: a mosh session outlives its SSH connection, and
     * it is shown as it is (see [needsFocus]).
     */
    suspend fun reuse(terminal: ActiveTerminal): Activation {
        val target = terminal.target
        val paneId = (target as? TerminalTarget.Herdr)?.paneId
        if (paneId == null || !needsFocus(terminal)) return Activation.Ready(terminal)
        val active = connections.host(terminal.host.id) ?: return Activation.Ready(terminal)
        val span = "reuse host=${terminal.host.id} pane=$paneId"
        connections.timing.begin(span)
        try {
            connections.focusHerdrPane(active, target.session, paneId)
            connections.timing.mark(span, "focused")
            connections.timing.watchTerminal(terminal.id, span)
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            return Activation.Failed(focusMessage(error))
        }
        return if (connections.terminal(terminal.id) != null) Activation.Ready(terminal)
        else Activation.Failed("That terminal is no longer open.")
    }

    /**
     * Reattach: reopens the terminal the user last had on a connected host. A herdr pane is focused
     * as for an inbox tap (together with a new terminal's start); an open terminal for the same target
     * is reused. The transport is chosen afresh by the host's preference; what the target ran over
     * before is not carried along. [connectedInThisTap] (a Resume that had to connect the host first)
     * lets AUTO await `mosh_server()`, one round trip, so a shell can still choose mosh (see
     * [HostConnections.awaitTransportChoice]). [span] is the timing path this belongs to (a Resume's
     * own, else a `reopen` span of its own).
     */
    suspend fun reopen(last: LastTerminal, hostLabel: String, span: String? = null, connectedInThisTap: Boolean = false): Activation {
        val active = connections.host(last.hostId) ?: return Activation.Failed("$hostLabel is no longer connected.")
        val path = span ?: "reopen host=${last.hostId}".also { connections.timing.begin(it) }
        try {
            return Activation.Ready(openOrReuse(active, last.hostId, last.target, path, connectedInThisTap))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            return Activation.Failed(focusMessage(error))
        }
    }

    fun launchReopen(last: LastTerminal, hostLabel: String, span: String? = null, connectedInThisTap: Boolean = false, done: (Activation) -> Unit) {
        val title = when (val target = last.target) {
            TerminalTarget.Shell -> "shell"
            is TerminalTarget.Tmux -> "tmux ${target.sessionName}"
            is TerminalTarget.Herdr -> "herdr" + (target.paneId?.let { " $it" } ?: "")
        }
        launch("Resuming $hostLabel: $title", done) { reopen(last, hostLabel, span, connectedInThisTap) }
    }

    fun launchOpen(active: ActiveHost, target: TerminalTarget, done: (Activation) -> Unit) {
        launch("Opening ${active.host.label}: ${targetTitle(target)}", done) { open(active, target) }
    }

    fun launchOpenAgent(hostId: Long, hostLabel: String, session: String?, paneId: String, done: (Activation) -> Unit) =
        launch("Focusing $hostLabel: herdr $paneId", done) { openAgent(hostId, hostLabel, session, paneId) }

    fun launchReuse(terminal: ActiveTerminal, done: (Activation) -> Unit) {
        if (!needsFocus(terminal)) {
            // A shell, tmux, a pane-less herdr or a closed terminal: nothing to wait for.
            cancel()
            done(Activation.Ready(terminal))
            return
        }
        launch("Focusing ${terminal.host.label}: ${terminal.title}", done) { reuse(terminal) }
    }

    /**
     * A herdr-pane terminal that is still open and whose host connection is up. Without the
     * connection nothing can be focused (a mosh session keeps running after its SSH connection was
     * lost), so the terminal is shown as it is: herdr's focus is shared state and it may show
     * another pane until the host is connected again.
     */
    private fun needsFocus(terminal: ActiveTerminal) = (terminal.target as? TerminalTarget.Herdr)?.paneId != null &&
        terminal.state.value !is SessionState.Closed && !terminal.retired &&
        connections.host(terminal.host.id)?.state?.value is HostState.Connected

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

/** What the user reads when a pane could not be focused: the agent is gone, or the error. */
fun focusMessage(error: Exception): String = when (error) {
    is HostException.PaneNotFound -> hostErrorMessage(error)
    is HostException.CommandFailed -> "Could not focus the agent's pane: ${error.reason}"
    is HostException -> hostErrorMessage(error)
    else -> "Could not focus the agent's pane: ${error.message ?: error::class.simpleName}"
}

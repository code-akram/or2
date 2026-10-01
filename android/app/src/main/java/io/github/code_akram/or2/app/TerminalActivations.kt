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
 * the inbox, the session switcher, a Home thumbnail and the host screen's recent list. herdr's
 * focus is shared state, so a terminal that was already open shows whichever pane is focused
 * *now*; each of these paths therefore awaits `focus_herdr_pane` before the terminal is shown, and
 * shows nothing for a pane that has gone (`PaneNotFound`) or that could not be focused.
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
     * An agent tapped in the inbox: focuses its pane, then reuses the terminal already open for it
     * or opens a new one. A first open is focused too, so a vanished pane never opens a terminal
     * that would close with a command failure.
     */
    suspend fun openAgent(hostId: Long, hostLabel: String, session: String?, paneId: String): Activation {
        val target = TerminalTarget.Herdr(session, paneId)
        val active = connections.host(hostId) ?: return Activation.Failed("$hostLabel is no longer connected.")
        try {
            connections.focusHerdrPane(active, session, paneId)
            connections.findOpenTerminal(hostId, target)?.let { return Activation.Ready(it) }
            connections.awaitTransportChoice(active)
            return Activation.Ready(connections.openTerminal(active, target))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            return Activation.Failed(focusMessage(error))
        }
    }

    /**
     * A terminal opened from the host screen (shell, tmux, a herdr session): under AUTO it first
     * waits for the capability probe (see [HostConnections.awaitTransportChoice]) so a quick tap
     * after connecting does not silently pick SSH.
     */
    suspend fun open(active: ActiveHost, target: TerminalTarget): Activation {
        try {
            connections.awaitTransportChoice(active)
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
        try {
            connections.focusHerdrPane(active, target.session, paneId)
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
     * first, exactly as for an inbox tap; an open terminal for the same target is reused. The
     * transport is chosen afresh by the host's preference (the capability probe is awaited briefly
     * so AUTO can still choose mosh right after connecting); what the target ran over before is not
     * carried along.
     */
    suspend fun reopen(last: LastTerminal, hostLabel: String): Activation {
        val active = connections.host(last.hostId) ?: return Activation.Failed("$hostLabel is no longer connected.")
        try {
            val target = last.target
            (target as? TerminalTarget.Herdr)?.paneId?.let { connections.focusHerdrPane(active, target.session, it) }
            connections.findOpenTerminal(last.hostId, target)?.let { return Activation.Ready(it) }
            connections.awaitTransportChoice(active)
            return Activation.Ready(connections.openTerminal(active, target))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            return Activation.Failed(focusMessage(error))
        }
    }

    fun launchReopen(last: LastTerminal, hostLabel: String, done: (Activation) -> Unit) {
        val title = when (val target = last.target) {
            TerminalTarget.Shell -> "shell"
            is TerminalTarget.Tmux -> "tmux ${target.sessionName}"
            is TerminalTarget.Herdr -> "herdr" + (target.paneId?.let { " $it" } ?: "")
        }
        launch("Resuming $hostLabel: $title", done) { reopen(last, hostLabel) }
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

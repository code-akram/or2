package io.github.code_akram.or2.app

import androidx.compose.runtime.saveable.Saver
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.notify.AgentPaneKey

/**
 * A terminal the app opens once its host is connected: a Resume (the card, the reconnect chip, a cold launch) or an
 * agent notification's tap. One at a time: a newer one replaces it. [started] is true once its own connect was asked
 * for; a tap that came while another unlock ran waits for that one to end, then connects ([pendingStep]).
 */
sealed interface PendingOpen {
    val hostId: Long
    val started: Boolean

    /** The same request, its connect asked for. */
    fun started(): PendingOpen

    data class Resume(val last: LastTerminal, override val started: Boolean = false) : PendingOpen {
        override val hostId get() = last.hostId
        override fun started() = copy(started = true)
    }

    data class Agent(val pane: AgentPaneKey, override val started: Boolean = false) : PendingOpen {
        override val hostId get() = pane.hostId
        override fun started() = copy(started = true)
    }

    companion object {
        /**
         * As saved state (the activity can be recreated while the biometric or the connect is in flight, and the
         * cold-launch marker is taken once per process): `resume|<started>|<LastTerminal>` or
         * `agent|<started>|<AgentPaneKey parts>`. Nothing pending saves nothing; anything unreadable restores nothing.
         */
        val Saver: Saver<PendingOpen?, Array<String>> = Saver(
            save = { pending ->
                when (pending) {
                    null -> null
                    is Resume -> arrayOf("resume", pending.started.toString(), pending.last.encode())
                    is Agent -> arrayOf("agent", pending.started.toString(), *pending.pane.toParts())
                }
            },
            restore = { parts -> restore(parts) },
        )

        fun restore(parts: Array<String>): PendingOpen? {
            if (parts.size < 3) return null
            val started = parts[1].toBooleanStrictOrNull() ?: return null
            return when (parts[0]) {
                "resume" -> if (parts.size == 3) LastTerminal.decode(parts[2])?.let { Resume(it, started) } else null
                "agent" -> AgentPaneKey.fromParts(parts.copyOfRange(2, parts.size))?.let { Agent(it, started) }
                else -> null
            }
        }
    }
}

/** The next step of a [PendingOpen]. */
enum class PendingStep {
    /** The host is on its way (or an unlock runs): nothing yet. */
    WAIT,

    /** Nothing is connecting it and no unlock runs: ask for its connect now (the usual unlock). */
    CONNECT,

    /** The host is connected: open the terminal. */
    OPEN,

    /** Its own connect ended without a connection (biometric cancelled, connect failed): give up. */
    ABORT,
}

/**
 * The one rule for a [PendingOpen] on a host in [state] (null: no connection; a lost connection's `Closed` is still
 * listed until a new attempt replaces it), while [busy] (an unlock or connect is in flight) and once its own connect
 * was asked for ([started]). Connected opens. While an unlock runs, or the host is on its way (connecting,
 * authenticating, a host-key decision), it waits. Once nothing runs, one that has not connected yet asks for its own
 * connect (a tap that came during another unlock is not dropped), and one whose connect ended without a connection
 * gives up.
 */
fun pendingStep(state: HostState?, busy: Boolean, started: Boolean): PendingStep = when {
    state is HostState.Connected -> PendingStep.OPEN
    busy -> PendingStep.WAIT
    state != null && state !is HostState.Closed -> PendingStep.WAIT
    !started -> PendingStep.CONNECT
    else -> PendingStep.ABORT
}

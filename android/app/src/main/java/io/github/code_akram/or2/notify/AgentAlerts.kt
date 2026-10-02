package io.github.code_akram.or2.notify

import io.github.code_akram.or2.app.PrefStore
import io.github.code_akram.or2.connection.HerdrObserver
import io.github.code_akram.or2.connection.HerdrSessionWatch
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.AgentStatus
import io.github.code_akram.or2.ffi.HerdrState
import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.inbox.agentName
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.WeakHashMap

/*
 * Agent notifications (v0.1.1, see contracts.md "Agent notifications"): exactly one notification per
 * Blocked or Done edge (`HerdrAgent.state_change_seq` advancing into `Blocked` or `Done`) observed by a
 * live herdr watch, none while that pane is on screen, and a tap opens the pane the way an inbox tap
 * does. Everything here is plain JVM logic; [AgentNotifications] posts through Android.
 */

/** One agent pane: what a notification is about, and where its tap leads. */
data class AgentPaneKey(val hostId: Long, val session: String?, val paneId: String) {
    /**
     * The notification's tag (its id is [AgentNotifications.NOTIFICATION_ID]): unique per host, session and pane.
     * The default session (null) is `d`; a named one is `s`, its length and the name, so no session name and pane id
     * (herdr's look like `w1:p2`) can read as another pair.
     */
    val tag: String get() = "agent:$hostId:" + (session?.let { "s${it.length}:$it" } ?: "d") + ":$paneId"

    /** As saved state (a pending open survives the activity's recreation); see [fromParts]. */
    fun toParts(): Array<String> = arrayOf(hostId.toString(), session?.let { "s$it" } ?: "d", paneId)

    companion object {
        fun fromParts(parts: Array<String>): AgentPaneKey? {
            if (parts.size != 3) return null
            val hostId = parts[0].toLongOrNull() ?: return null
            val session = when {
                parts[1] == "d" -> null
                parts[1].startsWith("s") -> parts[1].substring(1)
                else -> return null
            }
            return AgentPaneKey(hostId, session, parts[2])
        }
    }
}

/** What one notification says. Nothing from the pane's output. */
data class AgentAlert(val key: AgentPaneKey, val title: String, val text: String, val subText: String)

/** Where alerts go: the system's notifications in the app, a list in tests. */
interface AgentAlertSink {
    /** Posts [alert], replacing the pane's earlier one. */
    fun post(alert: AgentAlert)

    fun cancel(key: AgentPaneKey)
}

/** The terminal on screen in a resumed app: the host and the target it runs. */
data class OnScreen(val hostId: Long, val target: TerminalTarget)

/** `Needs input` (Blocked) or `Done`; null for every other status, which never notifies. */
fun alertText(status: AgentStatus): String? = when (status) {
    AgentStatus.BLOCKED -> "Needs input"
    AgentStatus.DONE -> "Done"
    else -> null
}

/**
 * The edge rule, fed every state of every live herdr watch ([HerdrObserver], on the holder's main dispatcher) and
 * the terminal on screen ([screenChanged]):
 *
 * - A watch's first `Live` view after it started (a connect) or after it was unavailable sets the baseline: what
 *   happened while the app was not watching never notifies. A pane that first appears in a later view is baselined
 *   too.
 * - A later view in which a pane's `state_change_seq` advanced and its status is `Blocked` or `Done` posts one
 *   notification for that pane (replacing its earlier one), unless the alerts are switched off ([enabled]) or the
 *   pane is on screen.
 * - A pane's notification is cancelled when the pane goes back to `Working`, disappears from its session's view, is
 *   shown on screen ([screenChanged]) or opened from the notification ([opened]); all are cancelled when the alerts
 *   are switched off ([enabledChanged]).
 *
 * "On screen" is the visible terminal of a resumed app running herdr for the pane's host and session, whose
 * shown pane is the session's focused one (herdr's focus is shared state; a terminal opened for one pane shows
 * whichever is focused) or, before herdr reported a focus, the pane it was opened for.
 */
class AgentAlerts(private val sink: AgentAlertSink, private val enabled: () -> Boolean = { true }) : HerdrObserver {
    /** Per watch object (a new connection makes new ones): each pane's last seen sequence number. */
    private val baselines = WeakHashMap<Any, Map<String, ULong>>()

    /** The focused pane of every live session view, by host id and session. */
    private val focused = mutableMapOf<Pair<Long, String?>, String?>()
    private val posted = mutableSetOf<AgentPaneKey>()
    private var screen: OnScreen? = null

    /** The panes with a notification up, for tests and diagnostics. */
    val active: Set<AgentPaneKey> get() = posted.toSet()

    override fun herdrStateChanged(host: Host, watch: HerdrSessionWatch, state: HerdrState) =
        viewChanged(watch, host.id, host.label, watch.session, (state as? HerdrState.Live)?.view)

    /**
     * One state of the watch [watch] (any object identifying it) of [session] on host [hostId]: [view] is null when
     * the state is not `Live` (starting, unavailable, closed), which ends the baseline.
     */
    fun viewChanged(watch: Any, hostId: Long, hostLabel: String, session: String?, view: HerdrView?) {
        if (view == null) {
            baselines.remove(watch)
            return
        }
        focused[hostId to session] = view.focusedPaneId
        val previous = baselines[watch]
        val seen = view.agents.associate { it.paneId to it.stateChangeSeq }
        baselines[watch] = seen
        for (agent in view.agents) {
            val key = AgentPaneKey(hostId, session, agent.paneId)
            if (agent.status == AgentStatus.WORKING) {
                cancel(key)
                continue
            }
            val text = alertText(agent.status) ?: continue
            val before = previous?.get(agent.paneId) ?: continue
            if (agent.stateChangeSeq <= before) continue
            when {
                isOnScreen(key) -> cancel(key)
                enabled() -> {
                    posted += key
                    sink.post(AgentAlert(key, agentName(agent), text, hostLabel))
                }
            }
        }
        // Gone from the session: nothing left to open.
        posted.filter { it.hostId == hostId && it.session == session && it.paneId !in seen }.forEach(::cancel)
        // The focus moved to a pane that is on screen now.
        posted.filter { it.hostId == hostId && it.session == session && isOnScreen(it) }.forEach(::cancel)
    }

    /** The terminal on screen in a resumed app, or null (another screen, or the app is not resumed). */
    fun screenChanged(onScreen: OnScreen?) {
        screen = onScreen
        posted.filter(::isOnScreen).forEach(::cancel)
    }

    /**
     * The pane was opened from its notification: cancelled even when this process never posted it (one left up by a
     * process that died).
     */
    fun opened(key: AgentPaneKey) {
        posted -= key
        sink.cancel(key)
    }

    /** The setting changed: switched off, every notification goes. */
    fun enabledChanged() {
        if (!enabled()) posted.toList().forEach(::cancel)
    }

    private fun isOnScreen(key: AgentPaneKey): Boolean {
        val shown = screen ?: return false
        val target = shown.target as? TerminalTarget.Herdr ?: return false
        if (shown.hostId != key.hostId || target.session != key.session) return false
        return (focused[key.hostId to key.session] ?: target.paneId) == key.paneId
    }

    private fun cancel(key: AgentPaneKey) {
        if (posted.remove(key)) sink.cancel(key)
    }
}

/**
 * The `Agent notifications` switch in Settings: on by default (zero configuration), so the stored flag is the
 * opposite one. One switch for every host; there is no per-host setting.
 */
class AgentAlertSettings(private val store: PrefStore) {
    private val mutableEnabled = MutableStateFlow(!store.getBoolean(OFF))
    val enabled: StateFlow<Boolean> = mutableEnabled.asStateFlow()

    fun set(on: Boolean) {
        store.putBoolean(OFF, !on)
        mutableEnabled.value = on
    }

    private companion object {
        const val OFF = "agent_alerts_off"
    }
}

/**
 * A notification's tap, handed from the activity's intent to the UI, which opens the pane through
 * `TerminalActivations.launchOpenAgent`. One request at a time: a newer tap replaces one not yet taken.
 */
class AgentOpenRequests {
    private val mutableRequest = MutableStateFlow<AgentPaneKey?>(null)
    val request: StateFlow<AgentPaneKey?> = mutableRequest.asStateFlow()

    fun request(key: AgentPaneKey) {
        mutableRequest.value = key
    }

    /** Takes the pending request (null when there is none). */
    fun take(): AgentPaneKey? = mutableRequest.value.also { mutableRequest.value = null }
}

/** What a notification's tap does first, given its host's connection. */
enum class AgentOpenStart {
    /** Connected: open the pane now. */
    OPEN,

    /** Connecting, authenticating or waiting for a host-key decision: open it once connected. */
    WAIT,

    /** Not connected (never, or the connection closed): connect first, as a Resume does, then open it. */
    CONNECT,
}

fun agentOpenStart(state: HostState?): AgentOpenStart = when (state) {
    is HostState.Connected -> AgentOpenStart.OPEN
    null, is HostState.Closed -> AgentOpenStart.CONNECT
    else -> AgentOpenStart.WAIT
}

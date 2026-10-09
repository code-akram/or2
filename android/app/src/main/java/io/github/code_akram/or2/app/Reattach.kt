package io.github.code_akram.or2.app

import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.UserCloseListener
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.ffi.TerminalTarget
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.net.URLDecoder
import java.net.URLEncoder

/** The terminal the user last had in front of them: which host, which target, over which transport. */
data class LastTerminal(val hostId: Long, val target: TerminalTarget, val transport: TerminalTransport) {
    /** `7|MOSH|herdr|-|=w1%3Ap1`: URL-encoded parts joined by `|`; `-` is null, `=` marks a value. */
    fun encode(): String {
        fun opt(value: String?) = if (value == null) "-" else "=" + URLEncoder.encode(value, "UTF-8")
        val targetParts = when (target) {
            TerminalTarget.Shell -> listOf("shell")
            is TerminalTarget.ShellIn -> listOf("shell-in", opt(target.path))
            is TerminalTarget.Tmux -> listOf("tmux", opt(target.sessionName))
            is TerminalTarget.Herdr -> listOf("herdr", opt(target.session), opt(target.paneId))
        }
        return (listOf(hostId.toString(), transport.name) + targetParts).joinToString("|")
    }

    companion object {
        /** Null for anything that is not an encoding this app wrote. */
        fun decode(text: String?): LastTerminal? {
            if (text == null) return null
            val parts = text.split("|")
            if (parts.size < 3) return null
            fun opt(value: String): String? = if (value == "-") null else value.removePrefix("=").let { URLDecoder.decode(it, "UTF-8") }
            return try {
                val hostId = parts[0].toLong()
                val transport = TerminalTransport.valueOf(parts[1])
                val target = when (parts[2]) {
                    "shell" -> if (parts.size == 3) TerminalTarget.Shell else return null
                    "shell-in" -> if (parts.size == 4 && parts[3].startsWith("=")) TerminalTarget.ShellIn(opt(parts[3])!!) else return null
                    "tmux" -> if (parts.size == 4 && parts[3] != "-") TerminalTarget.Tmux(opt(parts[3])!!) else return null
                    "herdr" -> if (parts.size == 5) TerminalTarget.Herdr(opt(parts[3]), opt(parts[4])) else return null
                    else -> return null
                }
                LastTerminal(hostId, target, transport)
            } catch (_: IllegalArgumentException) {
                null
            }
        }
    }
}

/**
 * The last focused terminal, in app-private preferences (Rust has no storage). The user closing a
 * host or terminal (or the remote shell exiting) forgets it: reattach never resurrects what was
 * ended on purpose, only what the network took.
 */
class ReattachMemory(private val store: PrefStore) : UserCloseListener {
    private val mutableLast = MutableStateFlow(LastTerminal.decode(store.getString(KEY)))

    val last: StateFlow<LastTerminal?> = mutableLast.asStateFlow()

    fun remember(terminal: LastTerminal) {
        if (mutableLast.value == terminal) return
        mutableLast.value = terminal
        store.putString(KEY, terminal.encode())
    }

    override fun terminalClosed(hostId: Long, target: TerminalTarget) {
        val last = mutableLast.value ?: return
        if (last.hostId == hostId && last.target == target) forget()
    }

    override fun hostClosed(hostId: Long) {
        if (mutableLast.value?.hostId == hostId) forget()
    }

    fun forget() {
        mutableLast.value = null
        store.putString(KEY, null)
    }

    private companion object {
        const val KEY = "last_terminal"
    }
}

/**
 * What the terminal screen's remembering effect calls once it is showing [terminal]: only a terminal
 * that is connected *now* and is not being closed becomes the Resume target. `hasConnected` is
 * history (it stays true through `Closed`) and says nothing about the present: a terminal the user
 * disconnected, or whose shell exited, stays listed with its final frame, and the effect starts again
 * on every recreation or revisit, so remembering on history would bring back what the user ended and
 * the next foreground return would reopen it. The state is read when this runs, not when the effect was
 * queued, so a close the holder processed in between (the user's Disconnect or Close, which also
 * forgot the memory; a remote exit) wins.
 */
fun ReattachMemory.rememberShown(terminal: ActiveTerminal, transport: TerminalTransport) {
    if (terminal.isOpenForReattach) remember(LastTerminal(terminal.host.id, terminal.target, transport))
}

/** A terminal the app holds, as the reattach decision sees it. [alive] is false once it closed. */
data class OpenSession(val id: Long, val hostId: Long, val target: TerminalTarget, val alive: Boolean)

/** What to do about the last terminal when the app is back. */
sealed interface Reattach {
    data object None : Reattach

    /** Its session is alive: show it (`request_full_frame`). */
    data class Show(val terminalId: Long) : Reattach

    /** Only its host connection is up: reopen the same target there (or show the open terminal on its herdr session as it is). */
    data class Reopen(val last: LastTerminal) : Reattach

    /** The host is not connected: Home offers "Resume", which unlocks and then reopens. */
    data class Resume(val last: LastTerminal) : Reattach
}

/**
 * Decides, in order: nothing remembered or its host is gone; a live session for the same host and
 * target; a connected host; otherwise resume after unlocking. [liveHosts] are hosts whose SSH
 * connection is open, [knownHosts] every stored host.
 */
fun decideReattach(last: LastTerminal?, sessions: List<OpenSession>, liveHosts: Set<Long>, knownHosts: Set<Long>): Reattach {
    if (last == null || last.hostId !in knownHosts) return Reattach.None
    sessions.firstOrNull { it.alive && it.hostId == last.hostId && it.target == last.target }?.let { return Reattach.Show(it.id) }
    return if (last.hostId in liveHosts) Reattach.Reopen(last) else Reattach.Resume(last)
}

/**
 * Whether coming back to a process that died on a terminal screen resumes at once (the grouped
 * unlock, then the host connects and the remembered target reopens, with no further tap): there is
 * a remembered terminal whose host still exists with a key and is not connected. Without a key
 * nothing could unlock; the Resume card is all that is left.
 */
fun shouldAutoResume(last: LastTerminal?, hosts: List<Host>, connectedHosts: Set<Long>): Boolean =
    last != null && last.hostId !in connectedHosts && hosts.any { it.id == last.hostId && it.keyId != null }

/**
 * Whether a **cold launcher start** resumes at once, like the recents path: the previous process died
 * with sessions open ([diedWithSessions], see [SessionMarker]), and there is a remembered terminal
 * that [shouldAutoResume] accepts. OxygenOS removes a killed app from recents, so the launcher is
 * how the user comes back, and the saved-destination path never sees that start. A target the user
 * closed on purpose is not remembered ([ReattachMemory] forgets it), and a process that ended in
 * order (Disconnect all, the last session closed) cleared the marker, so neither resumes.
 */
fun shouldAutoResumeOnLaunch(diedWithSessions: Boolean, last: LastTerminal?, hosts: List<Host>, connectedHosts: Set<Long>): Boolean =
    diedWithSessions && shouldAutoResume(last, hosts, connectedHosts)

/**
 * Whether the launch's recovery resumes the remembered terminal at once: the setting "Reopen the last terminal on
 * launch" ([reopenOnLaunch]) is on, no agent notification's tap started the app ([tapped]: the user asked for that
 * pane), and either the restored screen was a terminal that is gone ([deadTerminalScreen], the recents path:
 * [shouldAutoResume]) or this is a cold start after a process that died with sessions open ([diedWithSessions]:
 * [shouldAutoResumeOnLaunch]). With the setting off the app stays on Home, where the Resume card offers the same.
 */
fun resumesOnLaunch(
    reopenOnLaunch: Boolean, tapped: Boolean, deadTerminalScreen: Boolean, diedWithSessions: Boolean,
    last: LastTerminal?, hosts: List<Host>, connectedHosts: Set<Long>,
): Boolean = reopenOnLaunch && !tapped && if (deadTerminalScreen) {
    shouldAutoResume(last, hosts, connectedHosts)
} else {
    shouldAutoResumeOnLaunch(diedWithSessions, last, hosts, connectedHosts)
}

/**
 * A "sessions open" marker in app-private preferences, written while any terminal is open and
 * cleared the moment none is (an orderly end: Disconnect all, the last close, a remote exit).
 * A process that is killed leaves it set, so the next process finds [diedWithSessions] true.
 * It is read once, when this object is created, **before** this process writes anything.
 * [takeColdResume] hands that fact to the first activity that asks and to nobody after it, so a
 * rotation or a recreated activity never resumes a second time.
 */
class SessionMarker(private val store: PrefStore) {
    /** The previous process died with a session open. */
    val diedWithSessions: Boolean = store.getBoolean(KEY)
    private var taken = false
    private var written = diedWithSessions

    /** Records whether any terminal is open now; writes only on a change. */
    fun onOpenSessions(open: Boolean) {
        if (open == written) return
        written = open
        store.putBoolean(KEY, open)
    }

    /** True once per process, and only if the previous process died with sessions open. */
    fun takeColdResume(): Boolean {
        if (taken) return false
        taken = true
        return diedWithSessions
    }

    private companion object {
        const val KEY = "sessions_open"
    }
}

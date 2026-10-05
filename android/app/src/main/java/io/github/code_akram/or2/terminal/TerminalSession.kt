package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.TerminalFrame

/**
 * The session a terminal's input goes to, resolved at each call: the terminal's own (stable) route,
 * not a view's handle. The SSH-to-mosh swap (and the AUTO fallback) replaces a terminal's handle
 * before the screen has recomposed a view for the new one; a key, an IME commit, a paste or a submit
 * the old view takes meanwhile still reaches the session the terminal shows now, exactly once.
 */
fun interface SessionRoute {
    fun current(): SessionInterface?
}

/**
 * One view's access to a borrowed handle; never owns disconnect/close. Main thread only. Frames
 * ([takeFrame], [callOwn]) come from the bound handle; input ([call]) goes through the terminal's
 * [SessionRoute] when there is one, else to the bound handle too.
 */
internal class TerminalSession {
    private var handle: SessionInterface? = null
    private var route: SessionRoute? = null
    private var bound = false
    var gone = false
        private set

    fun bind(session: SessionInterface, route: SessionRoute? = null) {
        check(!bound || handle === session) { "A terminal view belongs to one session" }
        handle = session
        this.route = route
        bound = true
    }

    /** Input and anything else meant for the session the terminal shows now (the route's, at call time). */
    fun call(block: SessionInterface.() -> Unit): Boolean = invoke(route?.current() ?: handle, block)

    /** A call about this view's own frames (full frame, size): always the bound handle. */
    fun callOwn(block: SessionInterface.() -> Unit): Boolean = invoke(handle, block)

    private fun invoke(target: SessionInterface?, block: SessionInterface.() -> Unit): Boolean {
        val session = target ?: return false
        return try {
            session.block()
            true
        } catch (_: SessionException.NotConnected) {
            false
        } catch (_: SessionException.Closed) {
            false // The final frame remains takeable after Closed, until close() destroys it.
        } catch (_: SessionException.InvalidKey) {
            false
        } catch (_: IllegalStateException) {
            // Only this view's own handle being destroyed ends the view; a destroyed route target is the terminal's end.
            if (session === handle) {
                handle = null
                gone = true
            }
            false
        }
    }

    /** Capture on main; only the native call/record decoding runs on the reader's worker. */
    fun prepareFrameRead(): (() -> TerminalFrame?)? = handle?.let { source -> { source.takeFrame() } }

    /** The reader reports destruction on main, never mutating session ownership from its worker. */
    fun frameSourceGone() {
        handle = null
        gone = true
    }

    fun takeFrame(): TerminalFrame? = try {
        handle?.takeFrame()
    } catch (_: IllegalStateException) {
        handle = null
        gone = true
        null
    }
}

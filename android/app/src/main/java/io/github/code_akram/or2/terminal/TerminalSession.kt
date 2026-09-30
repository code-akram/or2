package io.github.code_akram.or2.terminal

import io.github.code_akram.or2.ffi.SessionException
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.TerminalFrame

/** One view's access to a borrowed handle; never owns disconnect/close. Main thread only. */
internal class TerminalSession {
    private var handle: SessionInterface? = null
    private var bound = false
    var gone = false
        private set

    fun bind(session: SessionInterface) {
        check(!bound || handle === session) { "A terminal view belongs to one session" }
        handle = session
        bound = true
    }

    fun call(block: SessionInterface.() -> Unit): Boolean {
        val session = handle ?: return false
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
            handle = null
            gone = true
            false
        }
    }

    fun takeFrame(): TerminalFrame? = try {
        handle?.takeFrame()
    } catch (_: IllegalStateException) {
        handle = null
        gone = true
        null
    }
}

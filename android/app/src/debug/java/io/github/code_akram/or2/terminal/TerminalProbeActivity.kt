package io.github.code_akram.or2.terminal

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.Modifier
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.Session
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.contractProbeSession
import io.github.code_akram.or2.ffi.generateEd25519Key
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.receiveAsFlow

/** Debug-only, local native contract fixture. Never connects to a host. */
class TerminalProbeActivity : ComponentActivity() {
    private lateinit var session: Session
    private val state = MutableStateFlow<SessionState>(SessionState.Connecting)
    private val frames = Channel<Unit>(Channel.CONFLATED)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val key = generateEd25519Key("terminal probe")
        try {
            session = contractProbeSession(
                ConnectRequest("probe.invalid", 22u, "probe", key.privateKey, emptyList(), 40u, 12u),
                object : SessionListener {
                    override fun onStateChanged(state: SessionState) {
                        this@TerminalProbeActivity.state.value = state
                        if (state is SessionState.AwaitingHostKeyDecision) {
                            // Posting defers until the synchronous factory has assigned the handle.
                            runOnUiThread { session.approveHostKey(state.presented.fingerprint) }
                        }
                    }
                    override fun onFrameReady() { frames.trySend(Unit) }
                },
            )
        } finally {
            key.privateKey.fill(0)
        }
        setContent {
            MaterialTheme {
                TerminalScreen(session, state, frames.receiveAsFlow(), Modifier.windowInsetsPadding(WindowInsets.safeDrawing))
            }
        }
    }

    override fun onDestroy() {
        session.disconnect()
        session.close()
        frames.close()
        super.onDestroy()
    }
}

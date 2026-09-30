package io.github.code_akram.or2.terminal

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

@Composable
fun TerminalScreen(
    session: SessionInterface,
    state: StateFlow<SessionState>,
    frameReady: Flow<Unit>,
    modifier: Modifier = Modifier,
) {
    key(session) {
        val context = LocalContext.current
        val view = remember(session, context) { TerminalView(context).apply { bind(session) } }
        LaunchedEffect(view, state, frameReady) {
            launch { state.collect { view.sessionState(it) } }
            launch { frameReady.collect { view.frameReady() } }
        }
        Column(modifier = modifier.fillMaxSize().imePadding()) {
            AndroidView(factory = { view }, modifier = Modifier.weight(1f))
        }
    }
}

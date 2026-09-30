package io.github.code_akram.or2.terminal

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalKey
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
        var ctrl by remember { mutableStateOf(false) }
        var alt by remember { mutableStateOf(false) }
        var selecting by remember { mutableStateOf(false) }
        DisposableEffect(view) {
            view.onInputChanged = { ctrl = view.input.ctrl; alt = view.input.alt }
            view.onSelectionChanged = { selecting = view.selection != null }
            onDispose {
                view.onInputChanged = {}
                view.onSelectionChanged = {}
            }
        }
        LaunchedEffect(view, state, frameReady) {
            launch { state.collect { view.sessionState(it) } }
            launch { frameReady.collect { view.frameReady() } }
        }
        Column(modifier = modifier.fillMaxSize().imePadding()) {
            AndroidView(factory = { view }, modifier = Modifier.weight(1f).clipToBounds())
            Surface {
                Column {
                    Row(Modifier.horizontalScroll(rememberScrollState()).heightIn(min = 48.dp)) {
                        TerminalButton("Esc") { view.input.key(TerminalKey.Escape) }
                        TerminalButton("Tab") { view.input.key(TerminalKey.Tab) }
                        TerminalButton("Ctrl", ctrl) { view.input.toggleCtrl() }
                        TerminalButton("Alt", alt) { view.input.toggleAlt() }
                        listOf("←" to TerminalKey.ArrowLeft, "↓" to TerminalKey.ArrowDown,
                            "↑" to TerminalKey.ArrowUp, "→" to TerminalKey.ArrowRight,
                            "Home" to TerminalKey.Home, "End" to TerminalKey.End,
                            "PgUp" to TerminalKey.PageUp, "PgDn" to TerminalKey.PageDown).forEach { (label, key) ->
                            TerminalButton(label) { view.input.key(key) }
                        }
                    }
                    Row(Modifier.horizontalScroll(rememberScrollState()).heightIn(min = 48.dp)) {
                        TerminalButton("Keyboard") { view.showKeyboard() }
                        TerminalButton("Bottom") { view.jumpToBottom() }
                        if (selecting) {
                            TerminalButton("Copy") { view.copySelection() }
                            TerminalButton("Clear") { view.clearSelection() }
                        }
                        listOf("/", "-", "|", "~", "_", "$", "&", "*", "{", "}", "(", ")", "[", "]", "=", ";", "'", "\"").forEach { symbol ->
                            TerminalButton(symbol) { view.input.key(TerminalKey.Character(symbol)) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TerminalButton(label: String, selected: Boolean = false, action: () -> Unit) {
    TextButton(onClick = action, modifier = Modifier.heightIn(min = 48.dp).semantics {
        if (label == "Ctrl" || label == "Alt") stateDescription = if (selected) "Armed for next key" else "Off"
    }) {
        Text(if (selected) "$label ●" else label)
    }
}

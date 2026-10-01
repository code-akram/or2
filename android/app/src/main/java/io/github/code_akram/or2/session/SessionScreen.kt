package io.github.code_akram.or2.session

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.connection.ActiveTerminal
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.TerminalScreen

/**
 * One terminal session, with a switcher among the open ones. Leaving ([back], or switching) never
 * disconnects: the session keeps running and stays listed until the user closes it. The display
 * lease keeps the native handle alive while this screen shows it.
 */
@Composable
fun SessionScreen(
    holder: HostConnections,
    terminal: ActiveTerminal?,
    open: List<ActiveTerminal>,
    back: () -> Unit,
    select: (ActiveTerminal) -> Unit,
) {
    if (terminal == null) {
        Text("This terminal is no longer open.")
        Button(onClick = back) { Text("Back") }
        return
    }
    key(terminal) {
        DisposableEffect(terminal) {
            holder.attachDisplay(terminal)
            onDispose { holder.detachDisplay(terminal) }
        }
        val state by terminal.state.collectAsStateWithLifecycle()
        val handle by terminal.handle.collectAsStateWithLifecycle()
        val hasConnected by terminal.hasConnected.collectAsStateWithLifecycle()
        val closed = state is SessionState.Closed
        val endSession = { if (closed) { holder.dismissTerminal(terminal); back() } else holder.disconnectTerminal(terminal) }
        Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(if (hasConnected) 0.dp else 12.dp)) {
            if (hasConnected) {
                Surface(color = MaterialTheme.colorScheme.surface) {
                    Column {
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            TextButton(onClick = back, modifier = Modifier.testTag("terminal-back")) { Text("‹ Back") }
                            Column(Modifier.weight(1f)) {
                                Text(terminal.host.label, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(terminal.title + " · " + sessionMessage(state), style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            TextButton(onClick = endSession) { Text(if (closed) "Close" else "Disconnect") }
                        }
                        if (open.size > 1) Switcher(terminal, open, select)
                    }
                }
            } else {
                Text(terminal.host.label, style = MaterialTheme.typography.titleLarge)
                Text(terminal.title + " · " + sessionMessage(state))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = endSession) { Text(if (closed) "Close session" else "Disconnect") }
                    TextButton(onClick = back) { Text("Back") }
                }
                if (open.size > 1) Switcher(terminal, open, select)
            }
            // Keep the borrowed handle composed through Closed so its final frame stays visible.
            if (hasConnected) handle?.let { TerminalScreen(it, terminal.state, terminal.frameReady, Modifier.weight(1f)) }
        }
    }
}

/** The open sessions as tabs; the current one is marked. */
@Composable
private fun Switcher(current: ActiveTerminal, open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit) {
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).testTag("terminal-switcher")) {
        open.forEach { other ->
            val state by other.state.collectAsStateWithLifecycle()
            val label = other.host.label + " · " + other.title + if (state is SessionState.Closed) " (closed)" else ""
            TextButton(onClick = { if (other !== current) select(other) }, modifier = Modifier.heightIn(min = 48.dp).testTag("terminal-tab:${other.id}")) {
                Text(if (other === current) "• $label" else label, maxLines = 1,
                    fontWeight = if (other === current) FontWeight.Bold else FontWeight.Normal)
            }
        }
    }
}

/** Shortcuts to open terminals: inbox and host screens list them so a session is never lost. */
@Composable
fun OpenTerminalsRow(open: List<ActiveTerminal>, select: (ActiveTerminal) -> Unit) {
    if (open.isEmpty()) return
    Column(Modifier.testTag("open-terminals"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Open terminals", style = MaterialTheme.typography.titleSmall)
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
            open.forEach { terminal ->
                val state by terminal.state.collectAsStateWithLifecycle()
                TextButton(onClick = { select(terminal) }, modifier = Modifier.heightIn(min = 48.dp).testTag("open-terminal:${terminal.id}")) {
                    Text(terminal.host.label + " · " + terminal.title + if (state is SessionState.Closed) " (closed)" else "", maxLines = 1)
                }
            }
        }
    }
}

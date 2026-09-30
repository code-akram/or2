package io.github.code_akram.or2.session

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.terminal.TerminalScreen

@Composable
fun SessionScreen(holder: SessionHolder, busy: Boolean, approve: (ActiveSession, SessionState.AwaitingHostKeyDecision) -> Unit,
    reject: (ActiveSession) -> Unit) {
    val current by holder.active.collectAsStateWithLifecycle()
    if (current == null) {
        Text("No active session. Choose Connect on a host to unlock its key.")
        return
    }
    val displayed = current!!
    key(displayed) {
        DisposableEffect(displayed) {
            holder.attachDisplay(displayed)
            onDispose { holder.detachDisplay(displayed) }
        }
        val state by displayed.state.collectAsStateWithLifecycle()
        val handle by displayed.handle.collectAsStateWithLifecycle()
        val hasConnected by displayed.hasConnected.collectAsStateWithLifecycle()
        Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(if (hasConnected) 0.dp else 12.dp)) {
            if (hasConnected) {
                Surface(color = MaterialTheme.colorScheme.surface) {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(displayed.host.label, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(sessionMessage(state), style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                        TextButton(onClick = { if (state is SessionState.Closed) holder.dismiss() else holder.disconnect() }) {
                            Text(if (state is SessionState.Closed) "Close" else "Disconnect")
                        }
                    }
                }
            } else {
                Text(displayed.host.label, style = MaterialTheme.typography.titleLarge)
                Text(sessionMessage(state))
                Button(onClick = { if (state is SessionState.Closed) holder.dismiss() else holder.disconnect() }) {
                    Text(if (state is SessionState.Closed) "Close session" else "Disconnect")
                }
            }
            // Keep the borrowed handle composed through Closed so its final frame stays visible.
            if (hasConnected) handle?.let { TerminalScreen(it, displayed.state, displayed.frameReady, Modifier.weight(1f)) }
        }
        (state as? SessionState.AwaitingHostKeyDecision)?.let { prompt ->
            HostTrustDialog(prompt, busy, { approve(displayed, prompt) }, { reject(displayed) })
        }
    }
}

@Composable
fun HostTrustDialog(prompt: SessionState.AwaitingHostKeyDecision, busy: Boolean, approve: () -> Unit, reject: () -> Unit) {
    val changed = prompt.previouslyTrusted.isNotEmpty()
    AlertDialog(onDismissRequest = { if (!busy) reject() },
        title = { Text(if (changed) "WARNING: HOST KEY CHANGED" else "Trust this host key?",
            color = if (changed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(if (changed) "This may be an impersonation attack. Do not continue unless you independently verified the new key. Approving replaces ALL previous trusted keys."
                else "First connection to this host. Verify this fingerprint through an independent trusted channel before approving.")
                Text("Presented: ${prompt.presented.algorithm}")
                Text(prompt.presented.fingerprint)
                if (changed) {
                    Text("Previously trusted fingerprints:")
                    prompt.previouslyTrusted.forEach { Text("${it.algorithm}\n${it.fingerprint}") }
                }
            }
        },
        confirmButton = { TextButton(onClick = approve, enabled = !busy) { Text(if (changed) "Replace trust and connect" else "Trust and connect") } },
        dismissButton = { TextButton(onClick = reject, enabled = !busy) { Text("Reject") } })
}

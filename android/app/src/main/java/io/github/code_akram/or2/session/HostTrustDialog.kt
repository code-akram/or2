package io.github.code_akram.or2.session

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.HostState

/** First-use and changed-key decisions belong to the host, whichever of its addresses answered. */
@Composable
fun HostTrustDialog(
    prompt: HostState.AwaitingHostKeyDecision, busy: Boolean, approve: () -> Unit, reject: () -> Unit,
    hostLabel: String? = null,
) {
    val changed = prompt.previouslyTrusted.isNotEmpty()
    AlertDialog(onDismissRequest = { if (!busy) reject() },
        title = { Text(if (changed) "WARNING: HOST KEY CHANGED" else "Trust this host key?",
            color = if (changed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                hostLabel?.let { Text("Host: $it") }
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

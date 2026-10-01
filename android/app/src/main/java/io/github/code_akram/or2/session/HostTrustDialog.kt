package io.github.code_akram.or2.session

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ui.MonoBlock
import io.github.code_akram.or2.ui.Or2Card
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.TextAction

/**
 * First-use and changed-key decisions belong to the host, whichever of its addresses answered.
 * Fingerprints are mono; a changed key is shown prominently in the danger colour, with every
 * previously trusted fingerprint, and approving it replaces them all.
 */
@Composable
fun HostTrustDialog(
    prompt: HostState.AwaitingHostKeyDecision, busy: Boolean, approve: () -> Unit, reject: () -> Unit,
    hostLabel: String? = null,
) {
    val changed = prompt.previouslyTrusted.isNotEmpty()
    val haptics = LocalHapticFeedback.current
    val accent = if (changed) Or2Colors.Danger else Or2Colors.Accent
    Or2Dialog(
        onDismiss = { if (!busy) reject() },
        title = if (changed) "WARNING: HOST KEY CHANGED" else "Trust this host key?",
        titleColor = if (changed) Or2Colors.Danger else Or2Colors.Text,
        // The warning is not a thin light heading: it is the one place a regular weight is wanted.
        titleStyle = if (changed) Or2Type.CardTitle.copy(fontWeight = FontWeight.Normal) else Or2Type.CardTitle,
        confirm = {
            TextAction(
                if (changed) "Replace trust and connect" else "Trust and connect",
                { haptics.performHapticFeedback(HapticFeedbackType.Confirm); approve() },
                color = accent, enabled = !busy, modifier = Modifier.testTag("hostkey-approve"),
            )
        },
        dismiss = { TextAction("Reject", reject, color = Or2Colors.Text, enabled = !busy, modifier = Modifier.testTag("hostkey-reject")) },
    ) {
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            hostLabel?.let { Text("Host: $it", style = Or2Type.Body, color = Or2Colors.Text) }
            if (changed) {
                Or2Card(color = Or2Colors.AttentionSurface, border = BorderStroke(1.dp, Or2Colors.Danger)) {
                    Text(
                        "This may be an impersonation attack. Do not continue unless you independently verified the new key. Approving replaces ALL previous trusted keys.",
                        style = Or2Type.Body, color = Or2Colors.Danger, modifier = Modifier.padding(12.dp),
                    )
                }
            } else {
                Text(
                    "First connection to this host. Verify this fingerprint through an independent trusted channel before approving.",
                    style = Or2Type.Body, color = Or2Colors.Text,
                )
            }
            Text(
                (if (changed) "NEW KEY · " else "") + prompt.presented.algorithm, style = Or2Type.Kicker,
                color = accent,
            )
            MonoBlock(prompt.presented.fingerprint, color = if (changed) Or2Colors.Danger else Or2Colors.Text,
                modifier = Modifier.testTag("hostkey-presented"))
            if (changed) {
                // The user compares these with the new key, so they stay legible: text at 70 %, not muted.
                Text("Previously trusted fingerprints:", style = Or2Type.Secondary, color = Or2Colors.Text)
                prompt.previouslyTrusted.forEach {
                    Text(it.algorithm, style = Or2Type.Kicker, color = Or2Colors.TextMuted)
                    MonoBlock(it.fingerprint, color = Or2Colors.Text.copy(alpha = 0.7f))
                }
            }
        }
    }
}

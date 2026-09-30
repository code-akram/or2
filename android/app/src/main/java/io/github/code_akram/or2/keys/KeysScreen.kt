package io.github.code_akram.or2.keys

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.KeyRecord

@Composable
fun KeysScreen(
    keys: List<KeyRecord>, busy: Boolean,
    generate: (String, String) -> Unit,
    import: (Uri, String, String?) -> Unit,
    delete: (String) -> Unit,
) {
    val context = LocalContext.current
    var label by remember { mutableStateOf("") }
    var comment by remember { mutableStateOf("") }
    // Do not save URI/passphrase to SavedState. Retry re-reads the source; file bytes are wiped per attempt.
    var uri by remember { mutableStateOf<Uri?>(null) }
    var passphrase by remember { mutableStateOf("") }
    var publicKey by remember { mutableStateOf<KeyRecord?>(null) }
    var deleting by remember { mutableStateOf<KeyRecord?>(null) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) {
        uri = it
        passphrase = ""
    }
    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Private keys stay encrypted with hardware-backed AES-GCM. Every save and connect requires a strong biometric.")
        OutlinedTextField(label, { label = it }, label = { Text("Key label") }, singleLine = true, enabled = !busy)
        OutlinedTextField(comment, { comment = it }, label = { Text("Public-key comment (optional)") }, singleLine = true, enabled = !busy)
        Button(onClick = { generate(label, comment) }, enabled = !busy && label.isNotBlank()) { Text("Generate Ed25519") }
        Button(onClick = { picker.launch(arrayOf("*/*")) }, enabled = !busy) { Text("Choose OpenSSH private key") }
        uri?.let { selected ->
            Text("File selected. Enter a passphrase if it is encrypted.")
            OutlinedTextField(passphrase, { passphrase = it }, label = { Text("Import passphrase (optional)") },
                visualTransformation = PasswordVisualTransformation(), singleLine = true, enabled = !busy)
            Row {
                Button(onClick = {
                    val value = passphrase.takeIf { it.isNotEmpty() }
                    passphrase = ""
                    import(selected, label, value)
                }, enabled = !busy && label.isNotBlank()) { Text("Import selected key") }
                TextButton(onClick = { uri = null; passphrase = "" }, enabled = !busy) { Text("Clear") }
            }
        }
        if (keys.isEmpty()) Text("No stored keys. Public keys can be copied or shared for manual authorized_keys setup.")
        keys.forEach { key ->
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(key.label, style = MaterialTheme.typography.titleMedium)
                    Text(key.algorithm)
                    Text(key.fingerprint)
                    Row {
                        TextButton(onClick = { publicKey = key }) { Text("Public key") }
                        TextButton(onClick = { deleting = key }, enabled = !busy) { Text("Delete") }
                    }
                }
            }
        }
    }
    publicKey?.let { key ->
        AlertDialog(onDismissRequest = { publicKey = null }, title = { Text("Public key · ${key.label}") },
            text = { Column(Modifier.verticalScroll(rememberScrollState())) {
                Text("Add this line to authorized_keys yourself. or2 never installs keys automatically.")
                SelectionContainer { Text(key.openssh) }
                Row {
                    TextButton(onClick = {
                        (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                            .setPrimaryClip(ClipData.newPlainText("SSH public key", key.openssh))
                    }) { Text("Copy") }
                    TextButton(onClick = {
                        context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).apply {
                            type = "text/plain"
                            putExtra(Intent.EXTRA_TEXT, key.openssh)
                        }, "Share public key"))
                    }) { Text("Share") }
                }
            } }, confirmButton = { TextButton(onClick = { publicKey = null }) { Text("Done") } })
    }
    deleting?.let { key ->
        AlertDialog(onDismissRequest = { deleting = null }, title = { Text("Delete key?") },
            text = { Text("Delete ${key.label} and its encrypted private key? Hosts using it will need a new key selection. This cannot be undone.") },
            confirmButton = { TextButton(onClick = { delete(key.id); deleting = null }) { Text("Delete") } },
            dismissButton = { TextButton(onClick = { deleting = null }) { Text("Cancel") } })
    }
}

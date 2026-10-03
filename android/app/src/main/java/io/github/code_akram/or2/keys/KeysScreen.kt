package io.github.code_akram.or2.keys

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.LocalCardColor
import io.github.code_akram.or2.ui.MonoBlock
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dialog
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.TextAction
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.copyText
import io.github.code_akram.or2.ui.scrolledUnder
import io.github.code_akram.or2.ui.shareText

/** Keys as grouped list cards with mono fingerprints; generate and import are the primary actions. */
@Composable
fun KeysScreen(
    keys: List<KeyRecord>, busy: Boolean,
    generate: (String, String) -> Unit,
    import: (Uri, String, String?) -> Unit,
    delete: (String) -> Unit,
    back: () -> Unit = {},
) {
    var label by rememberSaveable { mutableStateOf("") }
    var comment by rememberSaveable { mutableStateOf("") }
    // Do not save URI/passphrase to SavedState. Retry re-reads the source; file bytes are wiped per attempt.
    var uri by remember { mutableStateOf<Uri?>(null) }
    var passphrase by remember { mutableStateOf("") }
    var publicKey by remember { mutableStateOf<KeyRecord?>(null) }
    var deleting by remember { mutableStateOf<KeyRecord?>(null) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) {
        uri = it
        passphrase = ""
    }
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "SSH keys", back = back, scrolled = scroll.scrolledUnder())
        Column(Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("keys-list")) {
            Text(
                "Private keys stay encrypted with hardware-backed AES-GCM. Every save and connect requires a strong biometric.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 4.dp),
            )
            SectionHeader("Your keys")
            if (keys.isEmpty()) {
                GroupCard {
                    ListRow(
                        "No stored keys", icon = Or2Icons.Key, modifier = Modifier.testTag("keys-empty"),
                        subtitle = "Public keys can be copied or shared for manual authorized_keys setup.",
                    )
                }
            } else {
                GroupCard {
                    keys.forEachIndexed { index, key ->
                        if (index > 0) GroupDivider(inset = 44.dp)
                        ListRow(
                            key.label, subtitle = shortFingerprint(key.fingerprint), subtitleMono = true, icon = Or2Icons.Key, chevron = true,
                            modifier = Modifier.testTag("key:${key.id}"), onClick = { publicKey = key },
                        )
                    }
                }
            }
            SectionHeader("New key")
            Spacer(Modifier.height(6.dp)) // The first label gets the room its card-less group lacks.
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Or2Field(label, { label = it }, label = "Key label", placeholder = "my-phone", enabled = !busy, tag = "key-label")
                Or2Field(comment, { comment = it }, label = "Public-key comment (optional)",
                    placeholder = "me@phone", enabled = !busy, tag = "key-comment")
                PrimaryButton("Generate Ed25519", { generate(label, comment) }, Modifier.testTag("key-generate"),
                    enabled = !busy && label.isNotBlank())
                if (uri == null) {
                    PillButton("Choose OpenSSH private key", { picker.launch(arrayOf("*/*")) }, Modifier.fillMaxWidth().testTag("key-choose"),
                        icon = Or2Icons.Upload, enabled = !busy)
                }
            }
            uri?.let { selected ->
                Spacer(Modifier.height(12.dp))
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("File selected. Enter a passphrase if it is encrypted.", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
                    Or2Field(passphrase, { passphrase = it }, tag = "key-passphrase", label = "Import passphrase (optional)",
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                        enabled = !busy)
                    PrimaryButton("Import selected key", {
                        val value = passphrase.takeIf { it.isNotEmpty() }
                        passphrase = ""
                        import(selected, label, value)
                    }, Modifier.testTag("key-import"), enabled = !busy && label.isNotBlank())
                    PillButton("Clear", { uri = null; passphrase = "" }, Modifier.fillMaxWidth().testTag("key-clear"), enabled = !busy)
                }
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
    publicKey?.let { key ->
        PublicKeySheet(key, busy, delete = { deleting = key; publicKey = null }, dismiss = { publicKey = null })
    }
    deleting?.let { key ->
        Or2Dialog(
            onDismiss = { deleting = null }, title = "Delete key?",
            confirm = { TextAction("Delete", { delete(key.id); deleting = null }, color = Or2Colors.Danger, modifier = Modifier.testTag("key-delete-confirm")) },
            dismiss = { TextAction("Cancel", { deleting = null }, color = Or2Colors.Text) },
        ) { Text("Delete ${key.label} and its encrypted private key? Hosts using it will need a new key selection. This cannot be undone.") }
    }
}

/** A key's own sheet: its fingerprint and full public line (selectable), Copy, Share and Delete. */
@Composable
fun PublicKeySheet(key: KeyRecord, busy: Boolean, delete: () -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = key.label, modifier = Modifier.testTag("key-sheet")) {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("${key.algorithm} · Add this line to authorized_keys yourself. or2 never installs keys automatically.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            SelectionContainer {
                Text(key.fingerprint, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.testTag("key-fingerprint"))
            }
            SelectionContainer { MonoBlock(key.openssh, Modifier.testTag("key-openssh")) }
            PublicKeyActions(key.openssh, tagPrefix = "key") {
                GroupDivider(inset = 44.dp)
                ListRow("Delete key", icon = Or2Icons.Trash, titleColor = Or2Colors.Danger, enabled = !busy,
                    modifier = Modifier.testTag("key-delete"), onClick = delete)
            }
        }
    }
}

/**
 * The one Copy / Share group for a public key line ([publicKey]), shared by a key's sheet (which adds Delete through
 * [more]) and the "Add the key to the host" screen. Its rows are tagged `<tagPrefix>-copy` and `<tagPrefix>-share`.
 */
@Composable
fun PublicKeyActions(
    publicKey: String, tagPrefix: String, modifier: Modifier = Modifier, color: Color = LocalCardColor.current,
    more: @Composable ColumnScope.() -> Unit = {},
) {
    val context = LocalContext.current
    GroupCard(modifier, color = color) {
        ListRow("Copy public key", icon = Or2Icons.Copy, modifier = Modifier.testTag("$tagPrefix-copy"),
            onClick = { copyText(context, "SSH public key", publicKey) })
        GroupDivider(inset = 44.dp)
        ListRow("Share public key", icon = Or2Icons.Share, modifier = Modifier.testTag("$tagPrefix-share"),
            onClick = { shareText(context, "Share public key", publicKey) })
        more()
    }
}

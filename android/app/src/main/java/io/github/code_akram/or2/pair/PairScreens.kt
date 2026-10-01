package io.github.code_akram.or2.pair

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.hosts.hostFieldError
import io.github.code_akram.or2.keys.shortFingerprint
import io.github.code_akram.or2.ui.ActionCard
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.MonoBlock
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Sheet
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.TopBar

/**
 * Home's "+": two cards in a sheet, like Moshi's. Easy pair is the recommended one (a command on the host and a
 * scan); the manual form stays exactly as it was.
 */
@Composable
fun AddHostSheet(easyPair: () -> Unit, manual: () -> Unit, dismiss: () -> Unit) {
    Or2Sheet(dismiss, title = null, done = null, modifier = Modifier.testTag("add-host-sheet")) {
        Column(
            Modifier.padding(horizontal = Or2Dimens.Gutter).padding(bottom = Or2Dimens.Gutter),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            ActionCard(
                "Fastest", "Easy pair with QR",
                "Run one command on your Mac or Linux box and scan the QR. or2 installs the SSH key for you.",
                meta = "Recommended · ~1 min", icon = Or2Icons.QrCode, onClick = easyPair,
                modifier = Modifier.testTag("add-host-easy"),
            )
            ActionCard(
                "SSH-fluent", "Set up manually",
                "Already comfortable with SSH? Enter the hostname, user and key yourself.",
                meta = "~3 min · needs hostname + key", icon = Or2Icons.Server, onClick = manual,
                modifier = Modifier.testTag("add-host-manual"),
            )
        }
    }
}

/** The pairing screens in turn, driven by the flow's state; [PairFlow] holds all the logic. */
@Composable
fun PairDestination(
    state: PairState, keys: List<KeyRecord>, flow: PairFlow, deviceLabel: String,
    generateKey: suspend (label: String, comment: String) -> KeyRecord,
    close: () -> Unit, done: () -> Unit,
) {
    when (state) {
        is PairState.Scanning -> PairScanScreen(
            error = state.error, access = rememberCameraAccess(),
            onCode = { flow.onCode(it, keys) }, back = close,
        )
        is PairState.Review -> PairReviewScreen(
            state.review, keys, edit = flow::edit, submit = { flow.submit(keys, deviceLabel, generateKey) }, back = flow::rescan,
        )
        is PairState.Submitting -> PairProgressScreen(state.review.username, state.phoneFingerprint, cancel = flow::cancel)
        is PairState.KeyToInstall -> PairInstallKeyScreen(state.host.label, state.keyLine, state.fingerprint, done)
        is PairState.Paired -> Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally) {
            TopBar()
            Spacer(Modifier.height(96.dp))
            Spinner(Modifier.testTag("pair-saving"), size = 32.dp)
        }
    }
}

/** The camera (or a way to allow it) and a paste field: the two ways to hand over a pairing code. */
@Composable
fun PairScanScreen(error: String?, access: CameraAccess, onCode: (String) -> Unit, back: () -> Unit) {
    var pasting by rememberSaveable { mutableStateOf(false) }
    var pasted by remember { mutableStateOf("") }
    var asked by rememberSaveable { mutableStateOf(false) }
    // The user chose to scan: ask once now. After a denial the screen explains and offers pasting.
    LaunchedEffect(access.granted) {
        if (!access.granted && !asked) {
            asked = true
            access.request()
        }
    }
    val context = LocalContext.current
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Easy pair", back = back)
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("pair-scan"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                "On the host, run or2-pair and scan the QR code it prints.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted,
            )
            if (!pasting) {
                Box(
                    Modifier.fillMaxWidth().aspectRatio(1f).clip(Or2Shapes.Sheet).background(Or2Colors.Crust).testTag("pair-camera"),
                    contentAlignment = Alignment.Center,
                ) {
                    if (access.granted) {
                        QrScanner(onCode, Modifier.fillMaxSize())
                    } else {
                        Column(
                            Modifier.padding(Or2Dimens.Gutter * 2), horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Icon(Or2Icons.QrCode, null, Modifier.size(Or2Dimens.EmptyIcon), tint = Or2Colors.Subtle)
                            Text(
                                if (access.denied) {
                                    "The camera is not allowed, so nothing can be scanned. Allow it in this app's settings, or paste the code."
                                } else {
                                    "or2 uses the camera only to read the code. Nothing is recorded or sent."
                                },
                                style = Or2Type.Secondary, color = Or2Colors.TextMuted, textAlign = TextAlign.Center,
                            )
                            if (!access.denied) PillButton("Allow camera", access.request, Modifier.testTag("pair-allow-camera"))
                        }
                    }
                }
            }
            if (error != null) {
                Text(error, style = Or2Type.Secondary, color = Or2Colors.Danger, modifier = Modifier.testTag("pair-scan-error"))
            }
            if (pasting) {
                Or2Field(
                    pasted, { pasted = it }, label = "Pairing code", placeholder = "or2-pair:1?…", singleLine = false,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false),
                    tag = "pair-paste-field",
                    trailing = {
                        IconAction(Or2Icons.Paste, "Paste from the clipboard", { pasted = clipboardText(context) }, Modifier.testTag("pair-paste-clipboard"))
                    },
                )
                PrimaryButton("Continue", { onCode(pasted.trim()) }, Modifier.testTag("pair-paste-continue"), enabled = pasted.isNotBlank())
                PillButton("Scan instead", { pasting = false }, Modifier.fillMaxWidth().testTag("pair-scan-instead"), icon = Or2Icons.QrCode)
            } else {
                PillButton("Paste pairing code", { pasting = true }, Modifier.fillMaxWidth().testTag("pair-paste"), icon = Or2Icons.Paste)
            }
            Text(
                "No or2-pair on the host yet? Install it with cargo install --path core/or2-pair, or follow the Pair a host guide (docs/pairing.md).",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted,
            )
            Spacer(Modifier.height(16.dp))
            BottomInsetSpacer()
        }
    }
}

private fun clipboardText(context: Context): String {
    val manager = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    return manager.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString().orEmpty()
}

/**
 * What the code says, before anything is sent: the host's name and user, the addresses it will try, the host
 * key's fingerprint (trusted from the code, so there is no first-use prompt), and which phone key to authorize.
 */
@Composable
fun PairReviewScreen(
    review: PairReview, keys: List<KeyRecord>,
    edit: (name: String?, username: String?, choice: KeyChoice?) -> Unit,
    submit: () -> Unit, back: () -> Unit,
) {
    val offer = review.offer
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Review", back = back)
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("pair-review"),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Or2Field(review.name, { edit(it, null, null) }, label = "Name", placeholder = "My server", mono = false,
                enabled = !review.working, tag = "pair-name")
            Or2Field(review.username, { edit(null, it, null) }, label = "Username", placeholder = "your-username",
                enabled = !review.working, tag = "pair-username",
                errorText = if (review.username.isNotEmpty()) hostFieldError(review.username) else null)
            Column {
                Text("Addresses", style = Or2Type.Body, color = Or2Colors.Text)
                Text("Tried in this order, port ${offer.port}.", style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                    modifier = Modifier.padding(top = 2.dp, bottom = 6.dp))
                GroupCard(Modifier.testTag("pair-addresses")) {
                    offer.addresses.forEachIndexed { index, address ->
                        if (index > 0) GroupDivider(inset = Or2Dimens.Gutter)
                        Row(Modifier.fillMaxWidth().padding(horizontal = Or2Dimens.Gutter, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text("${index + 1}", style = Or2Type.MonoSmall, color = Or2Colors.Subtle, modifier = Modifier.width(18.dp))
                            Text("${address.host}:${address.port}", style = Or2Type.Mono, color = Or2Colors.Text)
                        }
                    }
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Host key", style = Or2Type.Body, color = Or2Colors.Text)
                SelectionContainer {
                    MonoBlock(offer.hostKey.fingerprint, Modifier.testTag("pair-host-fingerprint"))
                }
                Text(
                    "${offer.hostKey.algorithm}. It comes from the code, so or2 trusts it from the start: no first-use prompt. A different key later is a warning.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                )
            }
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Key to authorize", style = Or2Type.Body, color = Or2Colors.Text)
                Text(
                    if (review.listens) "Its public half is added to authorized_keys on the host, after you confirm there."
                    else "This code has no listener, so you add its public half to authorized_keys yourself afterwards.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                )
                GroupCard(Modifier.selectableGroup()) {
                    keys.forEach { key ->
                        val selected = review.choice == KeyChoice.Existing(key.id)
                        ListRow(
                            key.label, subtitle = shortFingerprint(key.fingerprint), subtitleMono = true, icon = Or2Icons.Key,
                            modifier = Modifier.testTag("pair-key:${key.id}").semantics(mergeDescendants = true) {}
                                .selectable(selected, enabled = !review.working, role = Role.RadioButton) { edit(null, null, KeyChoice.Existing(key.id)) },
                            trailing = if (selected) ({ Icon(Or2Icons.Check, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Accent) }) else null,
                        )
                        GroupDivider(inset = 44.dp)
                    }
                    val newSelected = review.choice == KeyChoice.New
                    ListRow(
                        "New key", subtitle = "Ed25519, generated on this phone. Asks for your biometric to save it.", icon = Or2Icons.Plus,
                        modifier = Modifier.testTag("pair-key-new").semantics(mergeDescendants = true) {}
                            .selectable(newSelected, enabled = !review.working, role = Role.RadioButton) { edit(null, null, KeyChoice.New) },
                        trailing = if (newSelected) ({ Icon(Or2Icons.Check, null, Modifier.size(Or2Dimens.Icon), tint = Or2Colors.Accent) }) else null,
                    )
                }
            }
            if (review.error != null) {
                Text(review.error, style = Or2Type.Secondary, color = Or2Colors.Danger, modifier = Modifier.testTag("pair-error"))
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PrimaryButton(
                    if (review.working) "Working…" else if (review.listens) "Pair and add host" else "Add host",
                    submit, Modifier.testTag("pair-submit"), enabled = review.valid && !review.working,
                )
                Text(
                    if (review.listens) "Only the public key is sent. You confirm it on the host."
                    else "The host key above is trusted. Connecting needs the key installed first.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.fillMaxWidth().padding(horizontal = 6.dp),
                )
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

/** The key is with the host; its user is asked to confirm the fingerprint shown here. */
@Composable
fun PairProgressScreen(user: String, phoneFingerprint: String, cancel: () -> Unit) {
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Easy pair")
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter * 2).testTag("pair-progress"),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Spacer(Modifier.height(48.dp))
            Spinner(size = 32.dp)
            Text("Confirm on the host", style = Or2Type.ScreenTitle, color = Or2Colors.Text, textAlign = TextAlign.Center)
            SelectionContainer {
                MonoBlock(phoneFingerprint, Modifier.testTag("pair-phone-fingerprint"))
            }
            Text(
                "The host asks: Authorize this key for $user? Check that the fingerprint matches, then type y. It waits up to two minutes.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted, textAlign = TextAlign.Center,
            )
            PillButton("Cancel", cancel, Modifier.testTag("pair-cancel"))
        }
    }
}

/** A code without a listener: the host is saved and trusted; the key is to be installed by hand. */
@Composable
fun PairInstallKeyScreen(hostLabel: String, keyLine: String, fingerprint: String, done: () -> Unit) {
    val context = LocalContext.current
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Add the key to the host")
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("pair-install"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                "$hostLabel is saved and its host key is trusted. Add this line to ~/.ssh/authorized_keys on the host, then connect from Home.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted,
            )
            Text(fingerprint, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.testTag("pair-install-fingerprint"))
            SelectionContainer { MonoBlock(keyLine, Modifier.testTag("pair-install-key")) }
            GroupCard(color = Or2Colors.SurfaceRaisedRow) {
                ListRow("Copy public key", icon = Or2Icons.Copy, modifier = Modifier.testTag("pair-install-copy"), onClick = {
                    (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                        .setPrimaryClip(ClipData.newPlainText("SSH public key", keyLine))
                })
                GroupDivider(inset = 44.dp)
                ListRow("Share public key", icon = Or2Icons.Share, modifier = Modifier.testTag("pair-install-share"), onClick = {
                    context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).apply {
                        type = "text/plain"
                        putExtra(Intent.EXTRA_TEXT, keyLine)
                    }, "Share public key"))
                })
            }
            PrimaryButton("Done", done, Modifier.testTag("pair-install-done"))
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

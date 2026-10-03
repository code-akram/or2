package io.github.code_akram.or2.pair

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
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.hosts.hostFieldError
import io.github.code_akram.or2.keys.KeyPicker
import io.github.code_akram.or2.keys.PublicKeyActions
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.IconAction
import io.github.code_akram.or2.ui.MonoBlock
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Field
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Shapes
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.PillButton
import io.github.code_akram.or2.ui.PrimaryButton
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.TopBar
import io.github.code_akram.or2.ui.clipboardText
import io.github.code_akram.or2.ui.copyText
import io.github.code_akram.or2.ui.scrolledUnder

/** The pairing screens in turn, driven by the flow's state; [PairFlow] holds all the logic. */
@Composable
fun PairDestination(
    state: PairState, keys: List<KeyRecord>, flow: PairFlow, deviceLabel: String,
    generateKey: suspend (label: String, comment: String) -> KeyRecord,
    close: () -> Unit, done: () -> Unit,
) {
    when (state) {
        is PairState.Scanning -> PairScanScreen(
            code = state.code.text, error = state.error, access = rememberCameraAccess(),
            onCode = { flow.onCode(it, keys) }, back = close,
        )
        is PairState.Review -> PairReviewScreen(
            state.review, keys, edit = flow::edit, submit = { flow.submit(keys, deviceLabel, generateKey) }, back = flow::rescan,
        )
        is PairState.Pairing -> PairProgressScreen(state.review.name.trim(), cancel = flow::cancel)
        is PairState.KeyToInstall -> PairInstallKeyScreen(state.host.label, state.keyLine, state.fingerprint, done)
        is PairState.Paired -> Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally) {
            TopBar()
            Spacer(Modifier.height(96.dp))
            Spinner(Modifier.testTag("pair-saving"), size = 32.dp)
        }
    }
}

/** The command to run on the host; the one line a person copies from this screen. */
const val PAIR_COMMAND = "or2-pair"

/** Installs or2-pair on a Linux or macOS host from the latest release (scripts/install-or2-pair.sh). */
const val INSTALL_COMMAND = "curl -fsSL https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh | sh"

/**
 * The pairing code to type on the host, then the camera (or a way to allow it) and a paste field: the two ways to hand
 * over the host's QR. [code] is the phone's `K` (`7KQ4-M2XD-9PTM`).
 */
@Composable
fun PairScanScreen(code: String, error: String?, access: CameraAccess, onCode: (String) -> Unit, back: () -> Unit) {
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
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Easy pair", back = back, scrolled = scroll.scrolledUnder())
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("pair-scan"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                // The only large element of the app: it is read off the phone and typed on the host.
                SelectionContainer {
                    Text(
                        code, style = Or2Type.PairCode, color = Or2Colors.Text, maxLines = 1,
                        modifier = Modifier.padding(top = 4.dp).testTag("pair-code"),
                    )
                }
                Text("Type this code into or2-pair on the host", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
                Row(
                    Modifier.fillMaxWidth().clip(Or2Shapes.Field).background(Or2Colors.SurfaceRaisedRow).padding(start = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(PAIR_COMMAND, style = Or2Type.Mono, color = Or2Colors.Text, modifier = Modifier.weight(1f).testTag("pair-command"))
                    IconAction(Or2Icons.Copy, "Copy the command", { copyText(context, "or2-pair command", PAIR_COMMAND) }, Modifier.testTag("pair-copy-command"))
                }
                Text("Then scan the QR code it prints.", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
            }
            if (!pasting) {
                Box(
                    Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(24.dp)).background(Or2Colors.Crust).testTag("pair-camera").semantics(mergeDescendants = true) {},
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
                    pasted, { pasted = it }, label = "Pairing code", placeholder = "or2-pair:2?…", singleLine = false,
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
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    "No or2-pair on the host yet? On Linux or macOS, install it (checksum checked, no sudo):",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                )
                Row(
                    Modifier.fillMaxWidth().clip(Or2Shapes.Field).background(Or2Colors.SurfaceRaisedRow).padding(start = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(INSTALL_COMMAND, style = Or2Type.MonoSmall, color = Or2Colors.Text, modifier = Modifier.weight(1f).padding(vertical = 6.dp).testTag("pair-install-command"))
                    IconAction(Or2Icons.Copy, "Copy the install command", { copyText(context, "or2-pair install command", INSTALL_COMMAND) }, Modifier.testTag("pair-copy-install"))
                }
                Text(
                    "Or build it from source: see the Pair a host guide (docs/pairing.md).",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                )
            }
            Spacer(Modifier.height(16.dp))
            BottomInsetSpacer()
        }
    }
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
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Review", back = back, scrolled = scroll.scrolledUnder())
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("pair-review"),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Or2Field(review.name, { edit(it, null, null) }, label = "Name", placeholder = "My server", mono = false,
                enabled = !review.working, tag = "pair-name")
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Or2Field(review.username, { edit(null, it, null) }, label = "Username", placeholder = "your-username",
                    // With a pairing id the key is authorized for the account the code names, so it is not editable.
                    enabled = !review.working && !review.enrolls, tag = "pair-username",
                    errorText = if (review.username.isNotEmpty()) hostFieldError(review.username) else null)
                if (review.enrolls) {
                    Text("The host authorizes this key for ${review.username} only.", style = Or2Type.Secondary, color = Or2Colors.TextMuted)
                }
            }
            Column {
                Text("Addresses", style = Or2Type.Body, color = Or2Colors.Text)
                Text("Tried in this order, port ${offer.port}.", style = Or2Type.Secondary, color = Or2Colors.TextMuted,
                    modifier = Modifier.padding(top = 2.dp, bottom = 6.dp))
                GroupCard(Modifier.testTag("pair-addresses").semantics(mergeDescendants = true) {}) {
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
                    when {
                        // The host installed this key: a retry only saves the host, so the key cannot change.
                        review.keyLocked -> "The host added this key already; only saving the host is left."
                        review.enrolls -> "Its public half is added to authorized_keys on the host."
                        else -> "This code was made with --manual, so you add its public half to authorized_keys yourself afterwards."
                    },
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.testTag("pair-key-note"),
                )
                KeyPicker(
                    keys, selectedKeyId = (review.choice as? KeyChoice.Existing)?.keyId, newSelected = review.choice == KeyChoice.New,
                    enabled = !review.working && !review.keyLocked,
                    choose = { edit(null, null, KeyChoice.Existing(it)) }, chooseNew = { edit(null, null, KeyChoice.New) },
                    tagPrefix = "pair-key",
                )
            }
            if (review.error != null) {
                Text(review.error, style = Or2Type.Secondary, color = Or2Colors.Danger, modifier = Modifier.testTag("pair-error"))
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PrimaryButton(
                    if (review.working) "Working…" else if (review.enrolls) "Pair" else "Add host",
                    submit, Modifier.testTag("pair-submit"), enabled = review.valid && !review.working,
                )
                Text(
                    if (review.enrolls) "Only the public key is sent, over SSH, with the code you typed on the host."
                    else "The host key above is trusted. Connecting needs the key installed first.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.fillMaxWidth(),
                )
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

/** The phone is logged in to the host and sending its key: one to a few seconds. */
@Composable
fun PairProgressScreen(name: String, cancel: () -> Unit) {
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Easy pair", scrolled = scroll.scrolledUnder())
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter * 2).testTag("pair-progress"),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Spacer(Modifier.height(48.dp))
            Spinner(size = 32.dp)
            Text("Pairing with $name…", style = Or2Type.CardTitle, color = Or2Colors.Text, textAlign = TextAlign.Center, modifier = Modifier.testTag("pair-progress-title"))
            PillButton("Cancel", cancel, Modifier.testTag("pair-cancel"))
        }
    }
}

/**
 * "Add the key to the host": [hostLabel] is saved, and its key line ([keyLine], with its [fingerprint]) is to be added to
 * the host's `~/.ssh/authorized_keys` by hand, with the shared Copy / Share group ([PublicKeyActions]) and **Done**. It
 * follows Easy pair with a `--manual` code ([trusted]: the host key came with the code) and the host form saving a host
 * with a **New key** (not [trusted]: the host-key dialog asks on the first connect).
 */
@Composable
fun PairInstallKeyScreen(hostLabel: String, keyLine: String, fingerprint: String, done: () -> Unit, trusted: Boolean = true) {
    val scroll = rememberScrollState()
    Column(Modifier.fillMaxSize()) {
        TopBar(title = "Add the key to the host", scrolled = scroll.scrolledUnder())
        Column(
            Modifier.weight(1f).verticalScroll(scroll).padding(horizontal = Or2Dimens.Gutter).testTag("pair-install"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                if (trusted) "$hostLabel is saved and its host key is trusted. Add this line to ~/.ssh/authorized_keys on the host, then connect from Home."
                else "$hostLabel is saved. Add this line to ~/.ssh/authorized_keys on the host, then connect from Home and trust its host key once.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.testTag("pair-install-note"),
            )
            Text(fingerprint, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.testTag("pair-install-fingerprint"))
            SelectionContainer { MonoBlock(keyLine, Modifier.testTag("pair-install-key")) }
            PublicKeyActions(keyLine, tagPrefix = "pair-install", color = Or2Colors.SurfaceRaisedRow)
            PrimaryButton("Done", done, Modifier.testTag("pair-install-done"))
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

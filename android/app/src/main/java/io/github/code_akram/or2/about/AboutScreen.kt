package io.github.code_akram.or2.about

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ffi.buildInfo
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupCard
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Icons
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.TopBar

/** Where or2's source lives. */
const val SOURCE_URL = "https://github.com/code-akram/or2"

/** or2's own licence, as SPDX names it. */
const val APP_LICENSE = "GPL-3.0-or-later"

/** The thanks printed on the About screen; the README's Acknowledgements section names the same projects with links. */
const val ACKNOWLEDGEMENT =
    "or2 stands on other people's open-source work. Ghostty's libghostty-vt draws the terminal, russh " +
        "speaks SSH, and mosh and mosh-rs keep a session alive across networks. herdr and tmux are what or2 " +
        "drives, and UniFFI, tokio, Jetpack Compose, Room and AndroidX Biometric hold the app together, " +
        "in the Catppuccin colours. Thank you to everyone who writes and shares software like this. " +
        "Every project that ships inside or2, with its licence, is listed below."

/** What the About screen reports about the build. */
data class AboutInfo(val appVersion: String, val coreVersion: String, val apiVersion: String) {
    companion object {
        fun read(context: Context): AboutInfo {
            val app = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull()
            val native = runCatching { buildInfo() }.getOrNull()
            return AboutInfo(app ?: "unknown", native?.version ?: "unknown", native?.apiVersion?.toString() ?: "unknown")
        }
    }
}

internal fun readAsset(context: Context, path: String): String = context.assets.open(path).bufferedReader().use { it.readText() }

internal fun openUrl(context: Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

/** The About destination: reads the build info and the licence text, and opens links in the browser. */
@Composable
fun AboutRoute(back: () -> Unit, openLicenses: () -> Unit) {
    val context = LocalContext.current
    val info = remember { AboutInfo.read(context) }
    AboutScreen(
        info = info, gplText = { readAsset(context, LicenseData.COPYING) },
        openSource = { openUrl(context, SOURCE_URL) }, openLicenses = openLicenses, back = back,
    )
}

/**
 * About or2: version, API version, the GPL-3.0-or-later licence (its full text one tap away), the
 * source link, a thank-you and the entry to the open-source list. Stateless apart from the
 * licence-text page: [gplText] is read only when that page opens.
 */
@Composable
fun AboutScreen(
    info: AboutInfo,
    gplText: () -> String,
    openSource: () -> Unit,
    openLicenses: () -> Unit,
    back: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var showingLicense by rememberSaveable { mutableStateOf(false) }
    if (showingLicense) {
        BackHandler { showingLicense = false }
        LicenseTextPage(
            title = APP_LICENSE, summary = "The licence of or2 itself", details = null, repository = null,
            texts = listOf(LicenseText("COPYING", remember { gplText() })), back = { showingLicense = false }, modifier = modifier,
        )
        return
    }
    Column(modifier.fillMaxSize()) {
        TopBar(title = "About or2", back = back)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("about-list")) {
            Text("or2", style = Or2Type.ScreenTitle, color = Or2Colors.Text, modifier = Modifier.padding(top = 8.dp))
            Text(
                "A free, open-source Android client for SSH and mosh, built for driving coding agents on your own machines.",
                style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 4.dp),
            )
            SectionHeader("Version")
            GroupCard {
                ListRow("Version", value = info.appVersion, modifier = Modifier.testTag("about-version"))
                GroupDivider()
                ListRow("API version", value = info.apiVersion, modifier = Modifier.testTag("about-api-version"))
                GroupDivider()
                ListRow("Native core", value = info.coreVersion, modifier = Modifier.testTag("about-core-version"))
            }
            SectionHeader("License")
            GroupCard {
                ListRow(
                    APP_LICENSE, subtitle = "Free software: use, study, share and change it. Tap for the full text.",
                    icon = Or2Icons.Document, chevron = true, modifier = Modifier.testTag("about-license"), onClick = { showingLicense = true },
                )
            }
            SectionHeader("Source code")
            GroupCard {
                ListRow(
                    "code-akram/or2", subtitle = SOURCE_URL, subtitleMono = true, icon = Or2Icons.External,
                    modifier = Modifier.testTag("about-source"), onClick = openSource,
                )
            }
            SectionHeader("Acknowledgements")
            Text(ACKNOWLEDGEMENT, style = Or2Type.Body, color = Or2Colors.TextMuted, modifier = Modifier.testTag("about-thanks"))
            Spacer(Modifier.height(Or2Dimens.Gutter))
            GroupCard {
                ListRow(
                    "Open source licenses", subtitle = "Rust crates, Android libraries and vendored code",
                    icon = Or2Icons.Layers, chevron = true, modifier = Modifier.testTag("about-licenses"), onClick = openLicenses,
                )
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

/**
 * One licence page: a title bar, then the entry's facts (vendored notices), its note and each full
 * text in a selectable block. Used for the app's own GPL text and for every row of the list.
 */
@Composable
fun LicenseTextPage(
    title: String,
    summary: String?,
    details: String?,
    repository: String?,
    texts: List<LicenseText>,
    back: () -> Unit,
    modifier: Modifier = Modifier,
    note: String? = null,
) {
    Column(modifier.fillMaxSize()) {
        TopBar(title = title, back = back)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = Or2Dimens.Gutter).testTag("license-text")) {
            if (summary != null) Text(summary, style = Or2Type.MonoSmall, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 4.dp))
            if (!repository.isNullOrBlank()) {
                SelectionContainer { Text(repository, style = Or2Type.MonoSmall, color = Or2Colors.Accent, modifier = Modifier.padding(top = 2.dp)) }
            }
            if (note != null) Text(note, style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 8.dp))
            if (details != null) {
                SectionHeader("Notice")
                GroupCard {
                    SelectionContainer {
                        Text(details, style = Or2Type.MonoSmall, color = Or2Colors.Text, modifier = Modifier.padding(Or2Dimens.Gutter))
                    }
                }
            }
            texts.forEach { text ->
                SectionHeader(text.file)
                GroupCard {
                    SelectionContainer {
                        Text(text.body, style = Or2Type.MonoSmall, color = Or2Colors.Text, modifier = Modifier.padding(Or2Dimens.Gutter))
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
            BottomInsetSpacer()
        }
    }
}

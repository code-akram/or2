package io.github.code_akram.or2.about

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.code_akram.or2.ui.BottomInsetSpacer
import io.github.code_akram.or2.ui.GroupDivider
import io.github.code_akram.or2.ui.ListRow
import io.github.code_akram.or2.ui.Or2Colors
import io.github.code_akram.or2.ui.Or2Dimens
import io.github.code_akram.or2.ui.Or2Type
import io.github.code_akram.or2.ui.SectionHeader
import io.github.code_akram.or2.ui.Spinner
import io.github.code_akram.or2.ui.TopBar
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** The Open source licenses destination: loads the assets off the main thread, then shows the list. */
@Composable
fun LicensesRoute(back: () -> Unit) {
    val context = LocalContext.current
    val loaded by produceState<Result<LicenseData>?>(null) {
        value = withContext(Dispatchers.IO) { runCatching { LicenseData.load { readAsset(context, it) } } }
    }
    val result = loaded
    if (result == null) {
        Column(Modifier.fillMaxSize()) {
            TopBar(title = "Open source licenses", back = back)
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Spinner(size = 24.dp) }
        }
    } else {
        result.fold(
            onSuccess = { LicensesScreen(it, back) },
            onFailure = {
                Column(Modifier.fillMaxSize()) {
                    TopBar(title = "Open source licenses", back = back)
                    Text(
                        "The licence data could not be read: ${it.message}", style = Or2Type.Secondary,
                        color = Or2Colors.Danger, modifier = Modifier.padding(Or2Dimens.Gutter).testTag("licenses-error"),
                    )
                }
            },
        )
    }
}

/**
 * Open source licenses: one grouped list per group (Rust, Android, Vendored), each row a project's
 * name with its version and licence in muted mono; a tap opens the full licence text, and Back
 * returns to the list. Stateless: [data] comes from the assets.
 */
@Composable
fun LicensesScreen(data: LicenseData, back: () -> Unit, modifier: Modifier = Modifier) {
    var selectedKey by rememberSaveable { mutableStateOf<String?>(null) }
    val selected = selectedKey?.let(data::find)
    if (selected != null) {
        BackHandler { selectedKey = null }
        LicenseTextPage(
            title = selected.displayName, summary = listOf(selected.name.takeIf { it != selected.displayName }, selected.summary)
                .filterNotNull().joinToString(" · "),
            details = selected.details, repository = selected.repository, texts = selected.texts, note = selected.note,
            back = { selectedKey = null }, modifier = modifier,
        )
        return
    }
    Column(modifier.fillMaxSize()) {
        TopBar(title = "Open source licenses", back = back)
        LazyColumn(Modifier.weight(1f).padding(horizontal = Or2Dimens.Gutter).testTag("licenses-list")) {
            item(key = "intro") {
                Text(
                    "or2 is GPL-3.0-or-later. These projects ship inside it under their own licences; tap one for its full text.",
                    style = Or2Type.Secondary, color = Or2Colors.TextMuted, modifier = Modifier.padding(top = 4.dp),
                )
            }
            LicenseGroup.entries.forEach { group ->
                val entries = data.group(group)
                item(key = "header-${group.name}") { SectionHeader("${group.title} · ${entries.size}") }
                itemsIndexed(entries, key = { _, entry -> entry.key }) { index, entry ->
                    GroupedRow(index, entries.size) {
                        if (index > 0) GroupDivider()
                        ListRow(
                            entry.displayName, subtitle = entry.summary, subtitleMono = true, chevron = true,
                            modifier = Modifier.testTag("license:${entry.key}"), onClick = { selectedKey = entry.key },
                        )
                    }
                }
            }
            item(key = "end") { BottomInsetSpacer() }
        }
    }
}

/** One row of a grouped list that lives in a lazy list: only the first and last rows round their outer corners. */
@Composable
private fun GroupedRow(index: Int, count: Int, content: @Composable () -> Unit) {
    val radius = 16.dp
    val shape = RoundedCornerShape(
        topStart = if (index == 0) radius else 0.dp, topEnd = if (index == 0) radius else 0.dp,
        bottomStart = if (index == count - 1) radius else 0.dp, bottomEnd = if (index == count - 1) radius else 0.dp,
    )
    Column(Modifier.fillMaxWidth().clip(shape).background(Or2Colors.Surface)) { content() }
}

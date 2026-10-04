package io.github.code_akram.or2.host

import io.github.code_akram.or2.ffi.HerdrView
import io.github.code_akram.or2.ffi.mergeDirectoryPaths

/**
 * The same host/session views used by the herdr tab, not another query. Current live cwd comes
 * first (default session, then named sessions; picker agent order, then all panes by id), followed
 * by history. Includes pi and plain-shell panes, without depending on any agent's history format.
 * Rust owns literal-path validation, exact deduplication and the shared 20-path cap.
 * A history read/failure cannot hide useful live paths; when live views disappear, use history alone.
 */
internal fun projectDirectories(
    history: DirectoryList, views: Map<String?, HerdrView>,
    merge: (List<String>, List<String>) -> List<String> = ::mergeDirectoryPaths,
): DirectoryList {
    val live = views.entries.sortedWith(compareBy({ it.key != null }, { it.key.orEmpty() })).asSequence().flatMap { (_, view) ->
        pickerWorkspaces(view).asSequence().flatMap { it.agents }.mapNotNull { it.cwd } +
            view.panes.sortedBy { it.paneId }.asSequence().mapNotNull { it.cwd }
    }
    // Avoid copying an entire large view over FFI. Length >4096 UTF-16 units necessarily
    // exceeds Rust's 4096-byte path limit; all other validation stays in Rust. Process small
    // batches without letting invalid/duplicate entries crowd later valid paths out of the cap.
    val candidates = (live + (history as? DirectoryList.Loaded)?.paths.orEmpty().asSequence()).filter { it.length <= 4096 }
    var paths = emptyList<String>()
    for (batch in candidates.chunked(32)) {
        paths = merge(paths, batch) // At most 20 accepted paths + 32 bounded candidates per call.
        if (paths.size == 20) break
    }
    return if (paths.isNotEmpty() || history is DirectoryList.Loaded) DirectoryList.Loaded(paths) else history
}

package io.github.code_akram.or2.paste

/**
 * Where an uploaded image's path goes and how (contracts.md, "Image paste", Insert): the host's
 * absolute path, shell-quoted only when it needs it, after a space, and never followed by Enter.
 * Claude Code and Codex read an image path in the prompt.
 */
enum class InsertTarget { COMPOSER, TERMINAL }

/** The composer when it is open (the path joins the message being written), else the terminal itself. */
fun insertTarget(composerOpen: Boolean): InsertTarget = if (composerOpen) InsertTarget.COMPOSER else InsertTarget.TERMINAL

/** Characters a POSIX shell takes literally in a word. */
private val SHELL_SAFE = Regex("[A-Za-z0-9_@%+=:,./-]+")

/** The longest path inserted (Linux's `PATH_MAX`, as Rust checks it). */
private const val MAX_PATH_CHARS = 4096

/**
 * Whether [path] may be typed into a terminal: absolute, at most 4096 characters, with no control
 * character (C0, DEL or C1: no NUL, ETX, ESC, CR or LF, so no bracketed paste marker either) and no
 * U+FFFD. Quoting cannot make a control character harmless: typed, ETX or ESC act before the shell
 * sees the quote. Rust already refuses such an answer from the host; this is the second line.
 */
fun insertablePath(path: String): Boolean =
    path.startsWith('/') && path.length <= MAX_PATH_CHARS && path.none { Character.isISOControl(it) || it == '�' }

/** [path] as one shell word: unchanged when every character is safe, else single-quoted (`'` as `'\''`). */
fun shellQuote(path: String): String =
    if (path.isNotEmpty() && SHELL_SAFE.matches(path)) path else "'" + path.replace("'", "'\\''") + "'"

/** What is inserted for [path]: a space, then the quoted path. Refuses a path that is not [insertablePath]. */
fun pathInsertion(path: String): String {
    require(insertablePath(path)) { "not a path to insert" }
    return " " + shellQuote(path)
}

/**
 * What is inserted for several [paths] at once (a queue run, contracts.md, "Several images at once"): each
 * [pathInsertion] in order, so `" /a.png /b.jpg"`. Refuses any path that is not [insertablePath].
 */
fun pathsInsertion(paths: List<String>): String = paths.joinToString("") { pathInsertion(it) }

/** The composer's [text] with [paths] inserted at its end, in order ([pathsInsertion]). */
fun composerWithPaths(text: String, paths: List<String>): String = text + pathsInsertion(paths)

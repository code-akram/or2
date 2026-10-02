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

/** [path] as one shell word: unchanged when every character is safe, else single-quoted (`'` as `'\''`). */
fun shellQuote(path: String): String =
    if (path.isNotEmpty() && SHELL_SAFE.matches(path)) path else "'" + path.replace("'", "'\\''") + "'"

/** What is inserted for [path]: a space, then the quoted path. */
fun pathInsertion(path: String): String = " " + shellQuote(path)

/** The composer's [text] with [path] inserted at its end. */
fun composerWithPath(text: String, path: String): String = text + pathInsertion(path)

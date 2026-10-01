package io.github.code_akram.or2.about

/** The three groups of the "Open source licenses" list. */
enum class LicenseGroup(val title: String) {
    Rust("Rust"),
    Android("Android"),
    Vendored("Vendored"),
}

/** One licence or notice text of an entry; [file] names where it came from (`LICENSE-MIT`, ...). */
data class LicenseText(val file: String, val body: String)

/**
 * A project the app ships: a crate or Zig component (Rust), a Maven artifact (Android) or a row of
 * `THIRD_PARTY_NOTICES.md` (Vendored). [license] is the SPDX expression (or the notice's own
 * wording) shown in the list; [texts] are the full licence texts.
 */
data class LicenseEntry(
    val group: LicenseGroup,
    /** The identifying name: the crate, `group:artifact`, or the vendored project. */
    val name: String,
    val version: String,
    val license: String,
    val repository: String,
    val texts: List<LicenseText>,
    val note: String? = null,
    /** A line of facts about the entry (vendored: source, commit, path, licence, copyright). */
    val details: String? = null,
) {
    val key: String get() = "${group.name}:$name:$version"

    /** The short name the list shows: a Maven artifact without its group. */
    val displayName: String
        get() = if (group == LicenseGroup.Android) name.substringAfter(':') else name

    /** The muted mono line under the name: `version · licence`. */
    val summary: String get() = listOf(version, license).filter { it.isNotBlank() }.joinToString(" · ")
}

/** Everything the About screens show, parsed from the assets under `licenses/`. */
data class LicenseData(
    val rust: List<LicenseEntry>,
    val android: List<LicenseEntry>,
    val vendored: List<LicenseEntry>,
    /** The full GPL-3.0 text of or2's own licence (`COPYING`). */
    val gpl: String,
) {
    fun group(group: LicenseGroup): List<LicenseEntry> = when (group) {
        LicenseGroup.Rust -> rust
        LicenseGroup.Android -> android
        LicenseGroup.Vendored -> vendored
    }

    fun find(key: String): LicenseEntry? = LicenseGroup.entries.firstNotNullOfOrNull { g -> group(g).find { it.key == key } }

    companion object {
        const val RUST = "licenses/rust.json"
        const val ANDROID = "licenses/android.json"
        const val NOTICES = "licenses/notices.md"
        const val COPYING = "licenses/COPYING"

        /** Reads the four assets through [read] (an asset path to its text) and parses them. */
        fun load(read: (String) -> String): LicenseData {
            val rust = LicenseParser.parseJson(LicenseGroup.Rust, read(RUST))
            val standard = LicenseParser.standardTexts(read(RUST))
            val gpl = read(COPYING)
            return LicenseData(
                rust = rust,
                android = LicenseParser.parseJson(LicenseGroup.Android, read(ANDROID)),
                vendored = LicenseParser.parseNotices(read(NOTICES), standard + ("GPL-3.0" to gpl)),
                gpl = gpl,
            )
        }
    }
}

object LicenseParser {
    private fun document(json: String): Map<*, *> =
        MiniJson.parse(json) as? Map<*, *> ?: throw IllegalArgumentException("licence data is not a JSON object")

    /** The `standard` section: SPDX id to its full text (Apache-2.0 and MIT), for the vendored notices. */
    fun standardTexts(json: String): Map<String, String> {
        val doc = document(json)
        val texts = doc["texts"] as Map<*, *>
        return (doc["standard"] as? Map<*, *>).orEmpty().entries.associate { (id, textId) -> id as String to texts[textId] as String }
    }

    /** Parses `rust.json` or `android.json`; each package lists the ids of its texts. */
    fun parseJson(group: LicenseGroup, json: String): List<LicenseEntry> {
        val doc = document(json)
        val texts = doc["texts"] as Map<*, *>
        return (doc["packages"] as List<*>).map { raw ->
            val p = raw as Map<*, *>
            LicenseEntry(
                group = group,
                name = p["name"] as String,
                version = p["version"] as? String ?: "",
                license = p["license"] as? String ?: "",
                repository = p["repository"] as? String ?: "",
                note = p["note"] as? String,
                texts = (p["texts"] as List<*>).map { t ->
                    t as Map<*, *>
                    LicenseText(t["file"] as String, texts[t["id"]] as? String ?: throw IllegalArgumentException("missing text ${t["id"]}"))
                },
            )
        }
    }

    private val link = Regex("""\[([^\]]+)]\(([^)]+)\)""")
    private val leadingLicense = Regex("""^[A-Za-z0-9.+-]+(?: (?:OR|AND|WITH) [A-Za-z0-9.+-]+)*""")
    private val commit = Regex("""`([0-9a-f]{7,40})`""")

    /**
     * The vendored entries: the rows of the first table of `THIRD_PARTY_NOTICES.md` (the one headed
     * `Source`). [standard] maps `Apache-2.0`, `MIT` and `GPL-3.0` to their texts, which are attached
     * to a row whose licence names them (the MIT text takes the row's copyright line).
     */
    fun parseNotices(markdown: String, standard: Map<String, String>): List<LicenseEntry> {
        val rows = markdown.lines().dropWhile { !it.startsWith("| Source |") }.drop(2).takeWhile { it.startsWith("|") }
        return rows.map { line ->
            val cells = line.trim().removePrefix("|").removeSuffix("|").split("|").map { it.trim() }
            require(cells.size == 5) { "notice row has ${cells.size} cells: $line" }
            val (source, pin, path, licence, copyright) = cells
            val match = link.find(source)
            val name = match?.groupValues?.get(1) ?: source
            val url = match?.groupValues?.get(2).orEmpty()
            val short = leadingLicense.find(licence)?.value ?: licence
            val details = buildString {
                appendLine("Source: ${plain(source)}")
                appendLine("Commit: ${plain(pin)}")
                appendLine("Path: ${plain(path)}")
                appendLine("Licence: ${plain(licence)}")
                append("Copyright: ${plain(copyright)}")
            }
            val texts = buildList {
                if (short.contains("Apache-2.0")) standard["Apache-2.0"]?.let { add(LicenseText("Apache-2.0 (standard text)", it)) }
                if (short.contains("MIT")) {
                    standard["MIT"]?.let {
                        add(LicenseText("MIT (standard text)", it.replace("<year> <copyright holders>", plain(copyright).removePrefix("© "))))
                    }
                }
                if (short.contains("GPL-3.0")) standard["GPL-3.0"]?.let { add(LicenseText("GPL-3.0 (COPYING)", it)) }
            }
            LicenseEntry(
                group = LicenseGroup.Vendored,
                name = name,
                version = commit.find(pin)?.groupValues?.get(1)?.take(7).orEmpty(),
                license = short,
                repository = url,
                texts = texts,
                details = details,
            )
        }
    }

    /** Markdown to plain text for the notice details: links as `text (url)`, no backticks. */
    private fun plain(markdown: String) = link.replace(markdown) { "${it.groupValues[1]} (${it.groupValues[2]})" }.replace("`", "")
}

package io.github.code_akram.or2.app

import android.content.Context

/**
 * Small app-private settings (the one-time prompts, the last terminal). Rust has no storage, so
 * this lives in the Android app; the interface keeps the logic on top of it testable on the JVM.
 */
interface PrefStore {
    fun getBoolean(key: String): Boolean
    fun putBoolean(key: String, value: Boolean)
    fun getString(key: String): String?

    /** A null [value] removes the entry. May reach the disk after it returns. */
    fun putString(key: String, value: String?)

    /**
     * [putString], on the disk before it returns: for a record that must survive the process dying right
     * after it is written (the mosh-server ledger). It blocks the caller for the write, so it is for such
     * records only, off the main thread. A store without a disk just writes.
     */
    fun putStringDurably(key: String, value: String?) = putString(key, value)
}

/** A [PrefStore] in memory: the default for screens under test and the fake for unit tests. */
class MemoryPrefStore : PrefStore {
    private val values = mutableMapOf<String, Any>()
    override fun getBoolean(key: String) = values[key] as? Boolean ?: false
    override fun putBoolean(key: String, value: Boolean) { values[key] = value }
    override fun getString(key: String) = values[key] as? String
    override fun putString(key: String, value: String?) { if (value == null) values.remove(key) else values[key] = value }
}

/** [PrefStore] over a private `SharedPreferences` file. A store that cannot be read or written behaves as empty. */
class SharedPrefsStore(context: Context, file: String = DEFAULT_FILE) : PrefStore {
    private val prefs = context.applicationContext.getSharedPreferences(file, Context.MODE_PRIVATE)

    override fun getBoolean(key: String) = try { prefs.getBoolean(key, false) } catch (_: RuntimeException) { false }
    override fun putBoolean(key: String, value: Boolean) = write { putBoolean(key, value) }
    override fun getString(key: String) = try { prefs.getString(key, null) } catch (_: RuntimeException) { null }
    override fun putString(key: String, value: String?) = write { if (value == null) remove(key) else putString(key, value) }

    /** `commit()`: the file is written when this returns, unlike [putString]'s `apply()`. */
    override fun putStringDurably(key: String, value: String?) {
        try {
            prefs.edit().apply { if (value == null) remove(key) else putString(key, value) }.commit()
        } catch (_: RuntimeException) {
            // As with any write here: a record that cannot be kept costs at most one orphaned server.
        }
    }

    private fun write(edit: android.content.SharedPreferences.Editor.() -> Unit) {
        try {
            prefs.edit().apply(edit).apply()
        } catch (_: RuntimeException) {
            // A flag that cannot be remembered only costs a repeated prompt or a missing resume card.
        }
    }

    companion object {
        const val DEFAULT_FILE = "or2-app"
    }
}

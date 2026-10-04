package io.github.code_akram.or2.terminal

/** Painting ignores wrapping and OSC 8 targets; link flash is a separate overlay. */
internal class PaintedRow(val cells: List<ResolvedCell>) {
    private val hash = cells.hashCode()
    override fun hashCode(): Int = hash
    override fun equals(other: Any?): Boolean = other is PaintedRow && cells == other.cells
}

/** Count-bounded access-order cache. Every removal releases the retained display list. */
internal class TerminalRowCache<V>(private val release: (V) -> Unit) {
    private val entries = LinkedHashMap<PaintedRow, V>(16, .75f, true)
    var hits = 0L
        private set
    var misses = 0L
        private set
    val size get() = entries.size
    var limit = 1
        set(value) {
            field = value.coerceAtLeast(1)
            trim()
        }

    fun getOrPut(key: PaintedRow, count: Boolean, record: () -> V): V {
        entries[key]?.let {
            if (count) hits++
            return it
        }
        if (count) misses++
        return record().also {
            entries[key] = it
            trim()
        }
    }

    private fun trim() {
        while (entries.size > limit) {
            val iterator = entries.entries.iterator()
            release(iterator.next().value)
            iterator.remove()
        }
    }

    fun clear() {
        entries.values.forEach(release)
        entries.clear()
    }

    fun resetCounters() {
        hits = 0
        misses = 0
    }
}

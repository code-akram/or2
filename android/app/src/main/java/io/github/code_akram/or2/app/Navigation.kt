package io.github.code_akram.or2.app

/** Where the app is. Inbox, Hosts and Keys are the top-level tabs; the rest are pushed on them. */
sealed interface Destination {
    data object Inbox : Destination
    data object Hosts : Destination
    data object Keys : Destination
    data class HostPage(val hostId: Long) : Destination
    data class Terminal(val terminalId: Long) : Destination

    fun encode(): String = when (this) {
        Inbox -> "inbox"
        Hosts -> "hosts"
        Keys -> "keys"
        is HostPage -> "host:$hostId"
        is Terminal -> "terminal:$terminalId"
    }

    companion object {
        fun decode(text: String): Destination? = when {
            text == "inbox" -> Inbox
            text == "hosts" -> Hosts
            text == "keys" -> Keys
            text.startsWith("host:") -> text.removePrefix("host:").toLongOrNull()?.let(::HostPage)
            text.startsWith("terminal:") -> text.removePrefix("terminal:").toLongOrNull()?.let(::Terminal)
            else -> null
        }
    }
}

/** A back stack; Inbox is the start destination and the bottom of every stack. */
data class NavStack(val entries: List<Destination> = listOf(Destination.Inbox)) {
    init {
        require(entries.isNotEmpty())
    }

    val current get() = entries.last()

    /** The tab the stack is rooted in. */
    val tab get() = entries.first()

    /** Pushes [destination]; the same destination twice in a row is one entry. */
    fun push(destination: Destination) = if (destination == current) this else NavStack(entries + destination)

    /** A tab switch starts a fresh stack at that tab. */
    fun top(destination: Destination) = NavStack(listOf(destination))

    /** Switching terminals replaces the screen instead of stacking them. */
    fun replaceTop(destination: Destination) = NavStack(entries.dropLast(1) + destination)

    /** The stack after Back, or null when already at the bottom (the system handles Back then). */
    fun back(): NavStack? = if (entries.size > 1) NavStack(entries.dropLast(1)) else null

    fun encode() = entries.joinToString("|") { it.encode() }

    companion object {
        fun decode(text: String): NavStack {
            val entries = text.split("|").mapNotNull(Destination::decode)
            // A stack always starts at a tab; anything else saved first is dropped to the inbox.
            val rooted = if (entries.firstOrNull().let { it == Destination.Inbox || it == Destination.Hosts || it == Destination.Keys }) entries
            else listOf(Destination.Inbox) + entries
            return NavStack(rooted)
        }
    }
}

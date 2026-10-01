package io.github.code_akram.or2.app

/**
 * Where the app is. Home and the agents inbox are the two top-level screens (an icon button
 * switches between them); keys, a host, the host form and terminals are pushed on top.
 */
sealed interface Destination {
    data object Home : Destination
    data object Inbox : Destination
    data object Keys : Destination

    /** About or2, and the open-source list pushed on top of it. */
    data object About : Destination
    data object Licenses : Destination
    data class HostPage(val hostId: Long) : Destination

    /** The add/edit host form; [hostId] 0 adds a new host. */
    data class HostForm(val hostId: Long) : Destination
    data class Terminal(val terminalId: Long) : Destination

    fun encode(): String = when (this) {
        Home -> "home"
        Inbox -> "inbox"
        Keys -> "keys"
        About -> "about"
        Licenses -> "licenses"
        is HostPage -> "host:$hostId"
        is HostForm -> "hostform:$hostId"
        is Terminal -> "terminal:$terminalId"
    }

    companion object {
        fun decode(text: String): Destination? = when {
            text == "home" || text == "hosts" -> Home // "hosts" is the M2 tab this screen replaced.
            text == "inbox" -> Inbox
            text == "keys" -> Keys
            text == "about" -> About
            text == "licenses" -> Licenses
            text.startsWith("hostform:") -> text.removePrefix("hostform:").toLongOrNull()?.let(::HostForm)
            text.startsWith("host:") -> text.removePrefix("host:").toLongOrNull()?.let(::HostPage)
            text.startsWith("terminal:") -> text.removePrefix("terminal:").toLongOrNull()?.let(::Terminal)
            else -> null
        }
    }
}

/** A back stack; Home is the start destination and the bottom of every stack. */
data class NavStack(val entries: List<Destination> = listOf(Destination.Home)) {
    init {
        require(entries.isNotEmpty())
    }

    val current get() = entries.last()

    /** The top-level screen the stack is rooted in. */
    val tab get() = entries.first()

    /** Pushes [destination]; the same destination twice in a row is one entry. */
    fun push(destination: Destination) = if (destination == current) this else NavStack(entries + destination)

    /** A top-level switch starts a fresh stack at that screen. */
    fun top(destination: Destination) = NavStack(listOf(destination))

    /** Switching terminals replaces the screen instead of stacking them. */
    fun replaceTop(destination: Destination) = NavStack(entries.dropLast(1) + destination)

    /** The stack after Back, or null when already at the bottom (the system handles Back then). */
    fun back(): NavStack? = if (entries.size > 1) NavStack(entries.dropLast(1)) else null

    /**
     * What Back leads to: the previous screen, or Home from a top-level screen that is not Home
     * (the Inbox), or null at Home itself, where the system leaves the app.
     */
    fun backOrHome(): NavStack? = back() ?: if (current != Destination.Home) NavStack() else null

    fun encode() = entries.joinToString("|") { it.encode() }

    companion object {
        fun decode(text: String): NavStack {
            val entries = text.split("|").mapNotNull(Destination::decode)
            // A stack always starts at a top-level screen; anything else saved first is dropped to Home.
            val rooted = if (entries.firstOrNull().let { it == Destination.Home || it == Destination.Inbox }) entries
            else listOf(Destination.Home) + entries
            return NavStack(rooted)
        }
    }
}

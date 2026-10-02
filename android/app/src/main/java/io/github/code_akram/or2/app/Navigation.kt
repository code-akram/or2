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

    /** App-wide switches (agent notifications), pushed from Home. */
    data object Settings : Destination
    data class HostPage(val hostId: Long) : Destination

    /** The add/edit host form; [hostId] 0 adds a new host. */
    data class HostForm(val hostId: Long) : Destination
    data class Terminal(val terminalId: Long) : Destination

    /** Easy pair: show the code, scan or paste, review, enrol the key. Its state lives in the pairing flow, not here. */
    data object EasyPair : Destination

    /**
     * The last step of adding a host: "Keep sessions alive in the background?" ([BatteryPrompt]). [hostId] is the
     * paired host to open and connect after it; 0 after the manual form (or a pairing code whose key is installed by
     * hand), which returns to the screen the host was added from.
     */
    data class KeepAlive(val hostId: Long) : Destination

    fun encode(): String = when (this) {
        Home -> "home"
        Inbox -> "inbox"
        Keys -> "keys"
        About -> "about"
        Licenses -> "licenses"
        Settings -> "settings"
        is HostPage -> "host:$hostId"
        is HostForm -> "hostform:$hostId"
        is Terminal -> "terminal:$terminalId"
        EasyPair -> "pair"
        is KeepAlive -> "keepalive:$hostId"
    }

    companion object {
        fun decode(text: String): Destination? = when {
            text == "home" || text == "hosts" -> Home // "hosts" is the M2 tab this screen replaced.
            text == "inbox" -> Inbox
            text == "keys" -> Keys
            text == "pair" -> EasyPair
            text == "about" -> About
            text == "licenses" -> Licenses
            text == "settings" -> Settings
            text.startsWith("hostform:") -> text.removePrefix("hostform:").toLongOrNull()?.let(::HostForm)
            text.startsWith("host:") -> text.removePrefix("host:").toLongOrNull()?.let(::HostPage)
            text.startsWith("terminal:") -> text.removePrefix("terminal:").toLongOrNull()?.let(::Terminal)
            text.startsWith("keepalive:") -> text.removePrefix("keepalive:").toLongOrNull()?.let(::KeepAlive)
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

    /**
     * The manual form saved a host (after its key-line screen, with **New key**). A new host ends on the battery step
     * when [keepAlive] (see [BatteryPrompt.shouldOffer]), which replaces the form; otherwise, and after an edit, the
     * form closes.
     */
    fun afterHostFormSaved(keepAlive: Boolean): NavStack =
        if (keepAlive) replaceTop(Destination.KeepAlive(0)) else backOrHome() ?: NavStack()

    /**
     * The battery step was answered (or had nothing left to ask): a paired host's page, where it connects; after the
     * manual form, the screen the host was added from. Anything else is left alone.
     */
    fun afterKeepAlive(): NavStack {
        val step = current as? Destination.KeepAlive ?: return this
        return if (step.hostId != 0L) NavStack().push(Destination.HostPage(step.hostId)) else backOrHome() ?: NavStack()
    }

    fun encode() = entries.joinToString("|") { it.encode() }

    companion object {
        /**
         * Easy pair saved [hostId]: its page from Home, where it connects at once, or first the battery step when
         * [keepAlive] (the step then opens the page and connects).
         */
        fun afterPaired(hostId: Long, keepAlive: Boolean): NavStack =
            NavStack().push(if (keepAlive) Destination.KeepAlive(hostId) else Destination.HostPage(hostId))

        /** A pairing code made with `--manual` ended on its key line: Home, or first the battery step when [keepAlive]. */
        fun afterKeyToInstall(keepAlive: Boolean): NavStack = if (keepAlive) NavStack().push(Destination.KeepAlive(0)) else NavStack()

        fun decode(text: String): NavStack {
            val entries = text.split("|").mapNotNull(Destination::decode)
            // A stack always starts at a top-level screen; anything else saved first is dropped to Home.
            val rooted = if (entries.firstOrNull().let { it == Destination.Home || it == Destination.Inbox }) entries
            else listOf(Destination.Home) + entries
            return NavStack(rooted)
        }
    }
}

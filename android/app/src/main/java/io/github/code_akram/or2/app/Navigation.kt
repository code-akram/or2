package io.github.code_akram.or2.app

/**
 * Where the app is. Home and the agents inbox are the two top-level screens (an icon button
 * switches between them); keys, the host form and terminals are pushed on top. A host has no
 * screen of its own: Home's host card is the one place for a host and its terminals.
 */
sealed interface Destination {
    data object Home : Destination
    data object Inbox : Destination
    data object Keys : Destination

    /** About or2, and the open-source list pushed on top of it. */
    data object About : Destination
    data object Licenses : Destination

    /** The app's settings (Agent notifications, Copy from the host), pushed from Home. */
    data object Settings : Destination

    /** The add/edit host form; [hostId] 0 adds a new host. */
    data class HostForm(val hostId: Long) : Destination
    data class Terminal(val terminalId: Long) : Destination

    /** Easy pair: show the code, scan or paste, review, enrol the key. Its state lives in the pairing flow, not here. */
    data object EasyPair : Destination

    /**
     * The last step of adding a host: "Keep sessions alive in the background?" ([BatteryPrompt]). [hostId] is the
     * paired host to connect (with its picker open over Home) after it; 0 after the manual form (or a pairing code
     * whose key is installed by hand), which returns to the screen the host was added from.
     */
    data class KeepAlive(val hostId: Long) : Destination

    fun encode(): String = when (this) {
        Home -> "home"
        Inbox -> "inbox"
        Keys -> "keys"
        About -> "about"
        Licenses -> "licenses"
        Settings -> "settings"
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
            // The host screen (v0.1.2 and before) is gone: a saved one is Home, where its host's card is.
            text.startsWith("host:") -> Home
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
     * What Back leads to: Home from a terminal (what its minimise disc does, wherever the terminal was opened from),
     * the previous screen, or Home from a top-level screen that is not Home (the Inbox), or null at Home itself,
     * where the system leaves the app.
     */
    fun backOrHome(): NavStack? = when {
        current is Destination.Terminal -> NavStack()
        else -> back() ?: if (current != Destination.Home) NavStack() else null
    }

    /**
     * The manual form saved a host (after its key-line screen, with **New key**). A new host ends on the battery step
     * when [keepAlive] (see [BatteryPrompt.shouldOffer]), which replaces the form; otherwise, and after an edit, the
     * form closes.
     */
    fun afterHostFormSaved(keepAlive: Boolean): NavStack =
        if (keepAlive) replaceTop(Destination.KeepAlive(0)) else backOrHome() ?: NavStack()

    /**
     * The battery step was answered (or had nothing left to ask): Home for a paired host (the app opens that host's
     * picker over it and connects); after the manual form, the screen the host was added from. Anything else is left
     * alone.
     */
    fun afterKeepAlive(): NavStack {
        val step = current as? Destination.KeepAlive ?: return this
        return if (step.hostId != 0L) NavStack() else backOrHome() ?: NavStack()
    }

    fun encode() = entries.joinToString("|") { it.encode() }

    companion object {
        /**
         * Easy pair saved [hostId]: Home, where the app opens its picker and it connects at once, or first the battery
         * step when [keepAlive] (the step then lands on Home the same way).
         */
        fun afterPaired(hostId: Long, keepAlive: Boolean): NavStack =
            if (keepAlive) NavStack().push(Destination.KeepAlive(hostId)) else NavStack()

        /** A pairing code made with `--manual` ended on its key line: Home, or first the battery step when [keepAlive]. */
        fun afterKeyToInstall(keepAlive: Boolean): NavStack = if (keepAlive) NavStack().push(Destination.KeepAlive(0)) else NavStack()

        fun decode(text: String): NavStack {
            val decoded = text.split("|").mapNotNull(Destination::decode)
            // Home is only ever the bottom: a saved host screen decodes to Home, and what was under it goes.
            val entries = decoded.drop(decoded.lastIndexOf(Destination.Home).coerceAtLeast(0))
            // A stack always starts at a top-level screen; anything else saved first is dropped to Home.
            val rooted = if (entries.firstOrNull().let { it == Destination.Home || it == Destination.Inbox }) entries
            else listOf(Destination.Home) + entries
            return NavStack(rooted)
        }
    }
}

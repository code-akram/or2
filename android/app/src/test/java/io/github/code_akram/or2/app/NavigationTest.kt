package io.github.code_akram.or2.app

import org.junit.Assert.*
import org.junit.Test

class NavigationTest {
    @Test
    fun homeIsTheStartAndTheBottomOfTheStack() {
        val start = NavStack()
        assertEquals(Destination.Home, start.current)
        assertNull(start.back())
        val host = start.push(Destination.HostPage(7))
        val terminal = host.push(Destination.Terminal(3))
        assertEquals(Destination.Terminal(3), terminal.current)
        assertEquals(Destination.Home, terminal.tab)
        assertEquals(host, terminal.back())
        assertEquals(start, terminal.back()!!.back())
    }

    @Test
    fun switchingTerminalsReplacesAndTopLevelScreensStartAFreshStack() {
        val stack = NavStack().push(Destination.HostPage(1)).push(Destination.Terminal(1))
        val switched = stack.replaceTop(Destination.Terminal(2))
        assertEquals(listOf(Destination.Home, Destination.HostPage(1), Destination.Terminal(2)), switched.entries)
        assertEquals(Destination.Home, switched.tab)
        assertEquals(NavStack(listOf(Destination.Inbox)), switched.top(Destination.Inbox))
        assertSame(stack, stack.push(Destination.Terminal(1))) // The same screen twice is one entry.
    }

    @Test
    fun backFromTheInboxGoesHomeAndOnlyHomeLeavesTheApp() {
        val home = NavStack()
        val inbox = home.push(Destination.Inbox) // Home opens the inbox on top of itself ...
        assertEquals(home, inbox.backOrHome()) // ... so Back returns to Home, not out of the app.
        assertNull(home.backOrHome())
        // A stack saved by an older build can be rooted at the inbox: Back still goes Home.
        val rooted = NavStack(listOf(Destination.Inbox))
        assertEquals(home, rooted.backOrHome())
        assertEquals(rooted, NavStack(listOf(Destination.Inbox, Destination.Keys)).backOrHome())
        // The inbox's Home button starts a fresh stack at Home.
        assertEquals(home, inbox.top(Destination.Home))
    }

    @Test
    fun theStackSurvivesSavedStateAndGarbageFallsBackToHome() {
        val stack = NavStack().push(Destination.HostForm(42)).push(Destination.HostPage(42)).push(Destination.Terminal(9))
        assertEquals("home|hostform:42|host:42|terminal:9", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack(), NavStack.decode(""))
        assertEquals(NavStack(), NavStack.decode("nonsense|host:x"))
        assertEquals(listOf(Destination.Home, Destination.Terminal(5)), NavStack.decode("terminal:5").entries)
        // Keys is pushed on a top-level screen, never the bottom of a stack.
        assertEquals(listOf(Destination.Home, Destination.Keys), NavStack.decode("keys").entries)
        assertEquals(NavStack(listOf(Destination.Inbox, Destination.Keys)), NavStack.decode("inbox|keys"))
        // The M2 "hosts" tab is Home now.
        assertEquals(listOf(Destination.Home, Destination.HostPage(1)), NavStack.decode("hosts|host:1").entries)
        assertEquals(Destination.HostForm(0), NavStack.decode("home|hostform:0").current)
    }

    @Test
    fun easyPairIsAPushedScreenThatSurvivesSavedState() {
        val stack = NavStack().push(Destination.EasyPair)
        assertEquals("home|pair", stack.encode())
        assertEquals(stack, NavStack.decode("home|pair"))
        assertEquals(NavStack(), stack.back())
        // A pair screen saved first is dropped under Home, like any pushed screen.
        assertEquals(listOf(Destination.Home, Destination.EasyPair), NavStack.decode("pair").entries)
    }

    @Test
    fun aboutAndTheLicenseListArePushedOnHomeAndSurviveSavedState() {
        val stack = NavStack().push(Destination.About).push(Destination.Licenses)
        assertEquals("home|about|licenses", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack().push(Destination.About), stack.back())
        assertEquals(listOf(Destination.Home, Destination.About), NavStack.decode("about").entries)
    }
}

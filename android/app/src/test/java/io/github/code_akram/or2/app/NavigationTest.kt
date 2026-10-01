package io.github.code_akram.or2.app

import org.junit.Assert.*
import org.junit.Test

class NavigationTest {
    @Test
    fun theInboxIsTheStartAndTheBottomOfTheStack() {
        val start = NavStack()
        assertEquals(Destination.Inbox, start.current)
        assertNull(start.back())
        val host = start.push(Destination.HostPage(7))
        val terminal = host.push(Destination.Terminal(3))
        assertEquals(Destination.Terminal(3), terminal.current)
        assertEquals(Destination.Inbox, terminal.tab)
        assertEquals(host, terminal.back())
        assertEquals(start, terminal.back()!!.back())
    }

    @Test
    fun switchingTerminalsReplacesAndTabsStartAFreshStack() {
        val stack = NavStack().top(Destination.Hosts).push(Destination.HostPage(1)).push(Destination.Terminal(1))
        val switched = stack.replaceTop(Destination.Terminal(2))
        assertEquals(listOf(Destination.Hosts, Destination.HostPage(1), Destination.Terminal(2)), switched.entries)
        assertEquals(Destination.Hosts, switched.tab)
        assertEquals(NavStack(listOf(Destination.Keys)), switched.top(Destination.Keys))
        assertSame(stack, stack.push(Destination.Terminal(1))) // The same screen twice is one entry.
    }

    @Test
    fun theStackSurvivesSavedStateAndGarbageFallsBackToTheInbox() {
        val stack = NavStack().top(Destination.Hosts).push(Destination.HostPage(42)).push(Destination.Terminal(9))
        assertEquals("hosts|host:42|terminal:9", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack(), NavStack.decode(""))
        assertEquals(NavStack(), NavStack.decode("nonsense|host:x"))
        assertEquals(listOf(Destination.Inbox, Destination.Terminal(5)), NavStack.decode("terminal:5").entries)
        assertEquals(Destination.Keys, NavStack.decode("keys").current)
    }
}

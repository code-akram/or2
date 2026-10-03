package io.github.code_akram.or2.app

import org.junit.Assert.*
import org.junit.Test

class NavigationTest {
    @Test
    fun homeIsTheStartAndTheBottomOfTheStack() {
        val start = NavStack()
        assertEquals(Destination.Home, start.current)
        assertNull(start.back())
        val keys = start.push(Destination.Keys)
        val form = keys.push(Destination.HostForm(3))
        assertEquals(Destination.HostForm(3), form.current)
        assertEquals(Destination.Home, form.tab)
        assertEquals(keys, form.back())
        assertEquals(start, form.back()!!.back())
    }

    @Test
    fun backFromATerminalGoesHomeAsItsMinimiseDiscDoes() {
        val home = NavStack()
        assertEquals(home, home.push(Destination.Terminal(3)).backOrHome())
        // Wherever the terminal was opened from: the inbox (an agent's row), or a stack restored with more under it.
        assertEquals(home, NavStack().push(Destination.Inbox).push(Destination.Terminal(3)).backOrHome())
        assertEquals(home, NavStack(listOf(Destination.Inbox, Destination.Terminal(3))).backOrHome())
        assertEquals(home, NavStack().push(Destination.Keys).push(Destination.Terminal(3)).backOrHome())
        // Other pushed screens still go back one step.
        assertEquals(NavStack().push(Destination.Keys), NavStack().push(Destination.Keys).push(Destination.HostForm(0)).backOrHome())
    }

    @Test
    fun switchingTerminalsReplacesAndTopLevelScreensStartAFreshStack() {
        val stack = NavStack().push(Destination.Keys).push(Destination.Terminal(1))
        val switched = stack.replaceTop(Destination.Terminal(2))
        assertEquals(listOf(Destination.Home, Destination.Keys, Destination.Terminal(2)), switched.entries)
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
        val stack = NavStack().push(Destination.HostForm(42)).push(Destination.Terminal(9))
        assertEquals("home|hostform:42|terminal:9", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack(), NavStack.decode(""))
        assertEquals(NavStack(), NavStack.decode("nonsense|hostform:x"))
        assertEquals(listOf(Destination.Home, Destination.Terminal(5)), NavStack.decode("terminal:5").entries)
        // Keys is pushed on a top-level screen, never the bottom of a stack.
        assertEquals(listOf(Destination.Home, Destination.Keys), NavStack.decode("keys").entries)
        assertEquals(NavStack(listOf(Destination.Inbox, Destination.Keys)), NavStack.decode("inbox|keys"))
        // The M2 "hosts" tab is Home now.
        assertEquals(NavStack(), NavStack.decode("hosts"))
        assertEquals(Destination.HostForm(0), NavStack.decode("home|hostform:0").current)
    }

    @Test
    fun aSavedHostScreenDecodesToHome() {
        // The host screen is gone (v0.1.2): its host's card on Home is where it was.
        assertEquals(NavStack(), NavStack.decode("home|host:1"))
        assertEquals(NavStack(), NavStack.decode("hosts|host:1"))
        assertEquals(NavStack(), NavStack.decode("host:1"))
        assertEquals(NavStack(), NavStack.decode("inbox|host:1"))
        // What was pushed on it stays, on Home.
        assertEquals(NavStack().push(Destination.Terminal(9)), NavStack.decode("home|hostform:42|host:42|terminal:9"))
        assertEquals(NavStack().push(Destination.HostForm(3)), NavStack.decode("home|host:3|hostform:3"))
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
    fun theAddHostChooserPushesEasyPairOrTheNewHostFormOnTheScreenThatShowsIt() {
        // Home's empty state and "+" sheet, and the inbox's empty state, push the same two screens; Back returns.
        for (start in listOf(NavStack(), NavStack().push(Destination.Inbox))) {
            for (target in listOf(Destination.EasyPair, Destination.HostForm(0))) {
                val pushed = start.push(target)
                assertEquals(target, pushed.current)
                assertEquals(start, pushed.back())
                assertEquals(pushed, NavStack.decode(pushed.encode()))
            }
        }
        assertEquals("home|inbox|pair", NavStack().push(Destination.Inbox).push(Destination.EasyPair).encode())
    }

    @Test
    fun theBatteryStepSurvivesSavedState() {
        val stack = NavStack().push(Destination.KeepAlive(5))
        assertEquals("home|keepalive:5", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(Destination.KeepAlive(0), NavStack.decode("home|inbox|keepalive:0").current)
        assertEquals(NavStack(), NavStack.decode("home|keepalive:x")) // Garbage is dropped.
    }

    @Test
    fun easyPairEndsOnTheBatteryStepOrStraightOnHome() {
        assertEquals(NavStack(listOf(Destination.Home, Destination.KeepAlive(5))), NavStack.afterPaired(5, keepAlive = true))
        // Home, where the app opens the paired host's picker over its card while it connects.
        assertEquals(NavStack(), NavStack.afterPaired(5, keepAlive = false))
        // The step leads on to Home the same way, wherever pairing started.
        assertEquals(NavStack(), NavStack.afterPaired(5, keepAlive = true).afterKeepAlive())
        assertEquals(NavStack(), NavStack().push(Destination.Inbox).push(Destination.KeepAlive(5)).afterKeepAlive())
        // A --manual code ends on its key line, then the step, then Home.
        assertEquals(NavStack().push(Destination.KeepAlive(0)), NavStack.afterKeyToInstall(keepAlive = true))
        assertEquals(NavStack(), NavStack.afterKeyToInstall(keepAlive = false))
        assertEquals(NavStack(), NavStack.afterKeyToInstall(keepAlive = true).afterKeepAlive())
    }

    @Test
    fun theManualFormEndsOnTheBatteryStepInItsPlaceThenReturnsWhereItWasOpened() {
        val fromHome = NavStack().push(Destination.HostForm(0))
        val fromInbox = NavStack().push(Destination.Inbox).push(Destination.HostForm(0))
        assertEquals(NavStack().push(Destination.KeepAlive(0)), fromHome.afterHostFormSaved(keepAlive = true))
        assertEquals("home|inbox|keepalive:0", fromInbox.afterHostFormSaved(keepAlive = true).encode())
        assertEquals(NavStack(), fromHome.afterHostFormSaved(keepAlive = true).afterKeepAlive())
        assertEquals(NavStack().push(Destination.Inbox), fromInbox.afterHostFormSaved(keepAlive = true).afterKeepAlive())
        // No step (exempt, asked, or an edit): the form just closes.
        assertEquals(NavStack(), fromHome.afterHostFormSaved(keepAlive = false))
        val edit = NavStack().push(Destination.HostForm(3))
        assertEquals(NavStack(), edit.afterHostFormSaved(keepAlive = false))
        // Anything that is not the step is left alone.
        assertEquals(edit, edit.afterKeepAlive())
    }

    @Test
    fun aboutAndTheLicenseListArePushedOnHomeAndSurviveSavedState() {
        val stack = NavStack().push(Destination.About).push(Destination.Licenses)
        assertEquals("home|about|licenses", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack().push(Destination.About), stack.back())
        assertEquals(listOf(Destination.Home, Destination.About), NavStack.decode("about").entries)
    }

    @Test
    fun settingsArePushedOnHomeAndSurviveSavedState() {
        val stack = NavStack().push(Destination.Settings)
        assertEquals("home|settings", stack.encode())
        assertEquals(stack, NavStack.decode(stack.encode()))
        assertEquals(NavStack(), stack.back())
        assertEquals(listOf(Destination.Home, Destination.Settings), NavStack.decode("settings").entries)
    }
}

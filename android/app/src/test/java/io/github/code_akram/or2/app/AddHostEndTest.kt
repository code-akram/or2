package io.github.code_akram.or2.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * How adding a host ends, as `Or2App` drives it: the battery step comes last (after Easy pair, before the paired host
 * connects; after the manual form saves a new host), at most once ever, and never during a connect.
 */
class AddHostEndTest {
    /** What the app does once a pairing is saved: where it goes, and whether the host connects now. */
    private class Ending(val stack: NavStack, val connectNow: Long?)

    private fun paired(hostId: Long, battery: BatteryPrompt): Ending {
        val keepAlive = battery.shouldOffer()
        return Ending(NavStack.afterPaired(hostId, keepAlive), if (keepAlive) null else hostId)
    }

    /** The step is done: where it leads, and the host it connects (the paired one), as `Or2App`'s effect does. */
    private fun stepDone(stack: NavStack, battery: BatteryPrompt): Ending? {
        val step = stack.current as? Destination.KeepAlive ?: return null
        if (battery.step.value != KeepAliveStep.DONE) return null
        return Ending(stack.afterKeepAlive(), step.hostId.takeIf { it != 0L })
    }

    /** Where a paired host lands: Home, with its session picker open while it connects. */
    private val home = NavStack()

    @Test
    fun easyPairEndsOnTheStepAndOnlyThenConnectsTheHostWithItsPickerOverHome() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false }
        val ending = paired(5, battery)
        assertEquals(Destination.KeepAlive(5), ending.stack.current)
        assertNull(ending.connectNow) // Nothing connects (no biometric) while the step is up.
        assertNull(stepDone(ending.stack, battery)) // Waiting for the answer.
        battery.answer(allow = false)
        val done = stepDone(ending.stack, battery)!!
        assertEquals(home, done.stack)
        assertEquals(5L, done.connectNow)
    }

    @Test
    fun allowWaitsForAndroidsDialogBeforeTheHostConnects() {
        var exempt = false
        val battery = BatteryPrompt(MemoryPrefStore()) { exempt }
        val ending = paired(5, battery)
        battery.answer(allow = true)
        assertNull(stepDone(ending.stack, battery)) // Android's dialog is up: no unlock over it.
        exempt = true
        battery.requestClosed()
        val done = stepDone(ending.stack, battery)!!
        assertEquals(home, done.stack)
        assertEquals(5L, done.connectNow)
        assertFalse(battery.card.value)
    }

    @Test
    fun anExemptOrAlreadyAskedAppPairsStraightIntoTheConnect() {
        for (battery in listOf(
            BatteryPrompt(MemoryPrefStore()) { true },
            BatteryPrompt(MemoryPrefStore().apply { putBoolean("battery_asked", true) }) { false },
        )) {
            val ending = paired(5, battery)
            assertEquals(home, ending.stack)
            assertEquals(5L, ending.connectNow)
        }
    }

    @Test
    fun theStepIsAskedForTheFirstHostOnly() {
        val store = MemoryPrefStore()
        val first = paired(5, BatteryPrompt(store) { false })
        BatteryPrompt(store) { false }.answer(allow = false)
        assertEquals(Destination.KeepAlive(5), first.stack.current)
        val second = paired(6, BatteryPrompt(store) { false })
        assertEquals(home, second.stack)
        assertEquals(6L, second.connectNow)
        // Nor after the manual form.
        assertEquals(NavStack(), NavStack().push(Destination.HostForm(0)).afterHostFormSaved(BatteryPrompt(store) { false }.shouldOffer()))
    }

    @Test
    fun aManualSaveEndsOnTheStepThenReturnsWhereTheHostWasAddedFrom() {
        for (start in listOf(NavStack(), NavStack().push(Destination.Inbox))) {
            val battery = BatteryPrompt(MemoryPrefStore()) { false }
            val form = start.push(Destination.HostForm(0))
            // Save (or Done on the key line, after New key): the step replaces the form.
            val step = form.afterHostFormSaved(previousIsNull(form) && battery.shouldOffer())
            assertEquals(start.push(Destination.KeepAlive(0)), step)
            assertNull(stepDone(step, battery))
            battery.answer(allow = false)
            val done = stepDone(step, battery)!!
            assertEquals(start, done.stack)
            assertNull(done.connectNow) // The manual path never connects by itself.
        }
    }

    @Test
    fun editingAHostNeverEndsOnTheStep() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false }
        val form = NavStack().push(Destination.HostForm(3))
        assertEquals(NavStack(), form.afterHostFormSaved(previousIsNull(form) && battery.shouldOffer()))
        assertTrue(battery.shouldOffer()) // Still to be asked, when a host is added.
    }

    @Test
    fun aKeyToInstallFromEasyPairEndsOnTheStepThenHome() {
        val battery = BatteryPrompt(MemoryPrefStore()) { false }
        val step = NavStack.afterKeyToInstall(battery.shouldOffer())
        assertEquals(NavStack().push(Destination.KeepAlive(0)), step)
        battery.answer(allow = true)
        battery.requestClosed()
        val done = stepDone(step, battery)!!
        assertEquals(NavStack(), done.stack)
        assertNull(done.connectNow)
        assertEquals(NavStack(), NavStack.afterKeyToInstall(battery.shouldOffer()))
    }

    @Test
    fun aStepRestoredAfterItsAnswerGoesOnAtOnce() {
        // The process died after the answer: the saved stack still shows the step, the store has the answer.
        val store = MemoryPrefStore()
        BatteryPrompt(store) { false }.answer(allow = true)
        val restored = NavStack.decode(NavStack.afterPaired(5, keepAlive = true).encode())
        val done = stepDone(restored, BatteryPrompt(store) { false })!!
        assertEquals(home, done.stack)
        assertEquals(5L, done.connectNow)
    }

    /** The form adds a host when its destination is `HostForm(0)` (no stored host to edit). */
    private fun previousIsNull(stack: NavStack) = (stack.current as Destination.HostForm).hostId == 0L
}

package io.github.code_akram.or2.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** The one-time prompts: when the battery step and the notification offer show, and how earlier answers carry over. */
class OneTimePromptsTest {
    // --- the battery step: the last step of adding a host -----------------------------------------

    @Test
    fun theStepAsksOnlyWhenNeverAskedAndNotExemptAndWaitsWhileTheSystemDialogIsUp() {
        assertEquals(KeepAliveStep.ASK, keepAliveStep(asked = false, exempt = false, requesting = false))
        assertEquals(KeepAliveStep.DONE, keepAliveStep(asked = false, exempt = true, requesting = false)) // Already exempt.
        assertEquals(KeepAliveStep.DONE, keepAliveStep(asked = true, exempt = false, requesting = false)) // Already asked.
        assertEquals(KeepAliveStep.DONE, keepAliveStep(asked = true, exempt = true, requesting = false))
        assertEquals(KeepAliveStep.WAIT, keepAliveStep(asked = true, exempt = false, requesting = true)) // "Allow" opened it.
        assertEquals(KeepAliveStep.WAIT, keepAliveStep(asked = true, exempt = true, requesting = true)) // Granted, not closed yet.
    }

    @Test
    fun anExemptAppIsNeverOfferedTheStepAndNeverShowsTheCard() {
        val prompt = BatteryPrompt(MemoryPrefStore()) { true }
        assertFalse(prompt.shouldOffer())
        assertEquals(KeepAliveStep.DONE, prompt.step.value)
        assertFalse(prompt.answer(allow = false)) // Nothing to answer.
        assertFalse(prompt.card.value)
    }

    @Test
    fun notNowIsRecordedAtOnceNeverAskedAgainAndLeavesTheCard() {
        val store = MemoryPrefStore()
        val prompt = BatteryPrompt(store) { false }
        assertTrue(prompt.shouldOffer())
        assertEquals(KeepAliveStep.ASK, prompt.step.value)
        assertFalse(prompt.card.value) // Nothing declined yet.
        assertTrue(prompt.answer(allow = false))
        assertEquals(KeepAliveStep.DONE, prompt.step.value)
        assertTrue(prompt.card.value)
        assertFalse(prompt.shouldOffer()) // Never again...
        assertFalse(BatteryPrompt(store) { false }.shouldOffer()) // ...also after a restart.
        assertFalse(prompt.answer(allow = true)) // A second tap does nothing.
        assertTrue(BatteryPrompt(store) { false }.card.value) // The card is persisted.
    }

    @Test
    fun allowWaitsForTheSystemDialogAndADenialThereLeavesTheCard() {
        val store = MemoryPrefStore()
        var exempt = false
        val prompt = BatteryPrompt(store) { exempt }
        assertTrue(prompt.answer(allow = true))
        assertEquals(KeepAliveStep.WAIT, prompt.step.value)
        assertFalse(prompt.card.value) // The system dialog decides.
        assertFalse(prompt.answer(allow = false)) // Nothing more to answer while it is up.
        prompt.requestClosed() // Refused there, or the device has no such screen.
        assertEquals(KeepAliveStep.DONE, prompt.step.value)
        assertTrue(prompt.card.value)

        val granted = BatteryPrompt(MemoryPrefStore()) { exempt }
        exempt = false
        granted.answer(allow = true)
        exempt = true // Allowed in the system dialog.
        granted.requestClosed()
        assertEquals(KeepAliveStep.DONE, granted.step.value)
        assertFalse(granted.card.value)
    }

    @Test
    fun aNewProcessAfterAllowNeverAsksAgainAndDoesNotWait() {
        // The process died while the system dialog was up: "Allow" was recorded, the wait was memory-only.
        val store = MemoryPrefStore()
        BatteryPrompt(store) { false }.answer(allow = true)
        val restored = BatteryPrompt(store) { false }
        assertEquals(KeepAliveStep.DONE, restored.step.value)
        assertFalse(restored.shouldOffer())
        restored.requestClosed() // The relaunched activity's launcher delivers the result.
        assertTrue(restored.card.value)
    }

    @Test
    fun becomingExemptInSettingsEndsAPendingStepOnRefresh() {
        var exempt = false
        val prompt = BatteryPrompt(MemoryPrefStore()) { exempt }
        assertEquals(KeepAliveStep.ASK, prompt.step.value)
        exempt = true
        prompt.refresh() // Back in the foreground.
        assertEquals(KeepAliveStep.DONE, prompt.step.value)
    }

    @Test
    fun anAnswerToTheEarlierConnectTimeExplanationStillCounts() {
        // The keys of the explanation this step replaced: a user who answered it is not asked again.
        val store = MemoryPrefStore().apply {
            putBoolean("battery_asked", true)
            putBoolean("battery_declined", true)
        }
        val prompt = BatteryPrompt(store) { false }
        assertFalse(prompt.shouldOffer())
        assertEquals(KeepAliveStep.DONE, prompt.step.value)
        assertTrue(prompt.card.value) // Its "Not now" still leaves the card.
    }

    @Test
    fun theCardGoesWithTheExemptionComesBackIfItIsWithdrawnAndIsDismissedForGood() {
        val store = MemoryPrefStore()
        var exempt = false
        val prompt = BatteryPrompt(store) { exempt }
        prompt.answer(allow = false)
        assertTrue(prompt.card.value)
        exempt = true // Granted from the card, or in Settings.
        prompt.refresh()
        assertFalse(prompt.card.value)
        exempt = false // Withdrawn later: the card comes back (declined, not dismissed).
        prompt.refresh()
        assertTrue(prompt.card.value)
        prompt.dismissCard()
        assertFalse(prompt.card.value)
        assertFalse(BatteryPrompt(store) { exempt }.card.value) // Dismissed for good.
        assertFalse(prompt.shouldOffer()) // And the step is still not repeated.
    }

    // --- notifications: offered in context, never on connect ---------------------------------------

    @Test
    fun theConnectionOfferShowsUntilGrantedOrDismissed() {
        val store = MemoryPrefStore()
        var granted = false
        val permission = NotificationPermission(store) { granted }
        val offer = permission.offer(NotificationUse.CONNECTION)
        assertSame(offer, permission.offer(NotificationUse.CONNECTION)) // One per use.
        assertTrue(offer.visible.value)
        granted = true
        permission.refresh() // The dialog closed, or the app is back from Settings.
        assertFalse(offer.visible.value)
        granted = false // Revoked in Settings: offered again.
        permission.refresh()
        assertTrue(offer.visible.value)
        offer.dismiss()
        assertFalse(offer.visible.value)
        assertFalse(NotificationPermission(store) { false }.offer(NotificationUse.CONNECTION).visible.value) // Persisted.
    }

    @Test
    fun aUserWhoAnsweredTheEarlierConnectTimeRequestIsNotOfferedItAgain() {
        val store = MemoryPrefStore().apply { putBoolean("notifications_asked", true) }
        val permission = NotificationPermission(store) { false }
        assertFalse(permission.offer(NotificationUse.CONNECTION).visible.value)
        // That request counts as one: once Android stops showing its dialog, Allow opens the settings.
        assertEquals(NotificationGrant.SETTINGS, permission.grant(rationale = false))
        assertEquals(NotificationGrant.REQUEST, permission.grant(rationale = true))
    }

    @Test
    fun allowRequestsUntilAndroidStopsAskingThenOpensTheSettings() {
        val store = MemoryPrefStore()
        val permission = NotificationPermission(store) { false }
        // Never requested: no rationale yet, and Android shows its dialog.
        assertEquals(NotificationGrant.REQUEST, permission.grant(rationale = false))
        permission.requested()
        // Denied once: Android asks again (the rationale flag is up).
        assertEquals(NotificationGrant.REQUEST, permission.grant(rationale = true))
        // Denied for good: the rationale flag is down again after a request.
        assertEquals(NotificationGrant.SETTINGS, permission.grant(rationale = false))
        assertEquals(NotificationGrant.SETTINGS, NotificationPermission(store) { false }.grant(rationale = false)) // Persisted.
        // A denial does not hide the offer: it stays until it is dismissed.
        assertTrue(permission.offer(NotificationUse.CONNECTION).visible.value)
    }
}

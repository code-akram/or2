package io.github.code_akram.or2.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * `POST_NOTIFICATIONS` is requested once, the first time a connection starts (Android 13+). The
 * foreground service still runs when it is denied, and a denial is never asked again.
 */
class NotificationPermissionPolicy(private val store: PrefStore) {
    fun shouldAsk(sdk: Int, granted: Boolean): Boolean = sdk >= 33 && !granted && !store.getBoolean(ASKED)

    fun markAsked() = store.putBoolean(ASKED, true)

    private companion object {
        const val ASKED = "notifications_asked"
    }
}

/** See [BatteryPrompt.restoreStage]. */
enum class BatteryStage { EXPLANATION, SYSTEM_REQUEST, PROCEED }

/**
 * The battery-optimisation exemption, asked for **up front**: OxygenOS lets the SSH connections die
 * within minutes of the app going to the background unless the app is exempt, and a dialog on the
 * return from the background was modal over the terminal. So the first time the user starts a
 * connection, in the foreground, [shouldExplain] says whether our explanation shows before the
 * biometric prompt ([explain] raises it, [explained] answers it; the system's own request follows an
 * "Allow"). It is shown **once, ever**: [shouldExplain] is false from then on, whatever the answer.
 *
 * If the exemption is not in place afterwards ("Not now", or the system dialog was refused), [card]
 * says so: Home shows a small non-blocking card ("Background connections may drop", with an Allow
 * action that opens the system request again) until the user dismisses it ([dismissCard]) or the
 * exemption arrives ([refresh], called when the app comes to the foreground). [isExempt] reads
 * `PowerManager.isIgnoringBatteryOptimizations`; an app that is already exempt is never asked and
 * never shows the card.
 */
class BatteryPrompt(private val store: PrefStore, private val isExempt: () -> Boolean = { true }) {
    private val mutableExplaining = MutableStateFlow(false)
    private val mutableCard = MutableStateFlow(cardVisible())

    /** Our explanation is on screen, waiting for the user's answer. */
    val explaining: StateFlow<Boolean> = mutableExplaining.asStateFlow()

    /** The Home card is visible: the exemption was declined (or never granted) and the card is not dismissed. */
    val card: StateFlow<Boolean> = mutableCard.asStateFlow()

    /** True when the explanation has never been shown and the app is not exempt: ask before the first connection. */
    fun shouldExplain(): Boolean = !store.getBoolean(ASKED) && !isExempt()

    /** The explanation is on screen now. */
    fun explain() {
        mutableExplaining.value = true
    }

    /**
     * Where a connect that was waiting on the battery flow stands when its activity is recreated
     * (rotation, or the process was killed and restored): the explanation lives in memory only, so a
     * new process has none on screen, and a connect left `busy` with nothing to answer would never end.
     * [BatteryStage.EXPLANATION] raises the explanation again (it was never answered, so nothing was
     * recorded); [BatteryStage.SYSTEM_REQUEST] means "Allow" was tapped and the system's request was
     * launched (the explanation is recorded as asked), whose result is delivered to the activity's
     * launcher: it is never launched twice; [BatteryStage.PROCEED] means there is nothing to ask any
     * more (the app became exempt meanwhile), so the connect goes on.
     */
    fun restoreStage(): BatteryStage = when {
        store.getBoolean(ASKED) -> BatteryStage.SYSTEM_REQUEST
        isExempt() -> {
            mutableExplaining.value = false
            BatteryStage.PROCEED
        }
        else -> {
            mutableExplaining.value = true
            BatteryStage.EXPLANATION
        }
    }

    /** The user answered the explanation (either way): it is never shown again. */
    fun explained() {
        store.putBoolean(ASKED, true)
        mutableExplaining.value = false
    }

    /**
     * The exemption was not given (the user said "Not now", the system dialog was refused or does not
     * exist on this device): the card offers it again, without blocking anything.
     */
    fun declined() {
        store.putBoolean(DECLINED, true)
        refresh()
    }

    /** The user dismissed the card for good. */
    fun dismissCard() {
        store.putBoolean(CARD_DISMISSED, true)
        refresh()
    }

    /** Re-reads whether the exemption is in place: the card goes away once it is. */
    fun refresh() {
        mutableCard.value = cardVisible()
    }

    private fun cardVisible() = store.getBoolean(DECLINED) && !store.getBoolean(CARD_DISMISSED) && !isExempt()

    private companion object {
        const val ASKED = "battery_asked"
        const val DECLINED = "battery_declined"
        const val CARD_DISMISSED = "battery_card_dismissed"
    }
}

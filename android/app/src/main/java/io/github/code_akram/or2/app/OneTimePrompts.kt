package io.github.code_akram.or2.app

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/*
 * The one-time prompts. None of them is asked while connecting: every connect, the first one on a fresh
 * install included, goes straight to the biometric unlock.
 *
 * - The battery-optimisation exemption is the last step of adding a host ([BatteryPrompt]), asked once ever.
 * - `POST_NOTIFICATIONS` is offered in context ([NotificationPermission], [NotificationOffer]): Home's small
 *   "Show connection and agent notifications" card while a host is connected. The foreground service runs
 *   without it; agent alerts need it.
 *
 * Every flag lives in the app's [PrefStore]; the keys of the earlier connect-time prompts are read so that a user
 * who already answered one is not asked again.
 */

/** Where the end-of-setup battery step stands ([BatteryPrompt.step]). */
enum class KeepAliveStep {
    /** Not asked yet and not exempt: the step shows its explanation. */
    ASK,

    /** "Allow" was tapped and the system's own request is up: the step waits for it to close. */
    WAIT,

    /** Nothing (more) to ask: already exempt, or already answered. Setup goes on. */
    DONE,
}

/**
 * The step's decision. [requesting] (the system dialog is up) wins, so a step that was answered with "Allow" waits
 * for the system's answer; otherwise it asks when it was never [asked] and the app is not [exempt], and is done
 * when either holds.
 */
fun keepAliveStep(asked: Boolean, exempt: Boolean, requesting: Boolean): KeepAliveStep = when {
    requesting -> KeepAliveStep.WAIT
    !asked && !exempt -> KeepAliveStep.ASK
    else -> KeepAliveStep.DONE
}

/**
 * The battery-optimisation exemption. OxygenOS lets the SSH connections die within minutes of the app going to the
 * background unless the app is exempt, so or2 asks once, as the **last step of adding a host** (after Easy pair
 * succeeds, before the paired host connects; after the manual form saves a new host, including its key-line
 * screen): never during a connect, never over a terminal on a return.
 *
 * [shouldOffer] says whether setup ends on the step (never asked, and [isExempt] is false); [step] drives the step's
 * screen. [answer] records the answer at once (it is never shown again, whatever it was); after "Allow" the caller
 * opens the system's request and reports its end with [requestClosed].
 *
 * If the exemption is not in place afterwards ("Not now", or the system dialog was refused or does not exist on the
 * device), [card] says so: Home shows a small non-blocking card ("Background connections may drop", Allow opens the
 * system request again) until the user dismisses it ([dismissCard]) or the exemption arrives ([refresh], called when
 * the app comes to the foreground). An app that is already exempt is never asked and never shows the card.
 *
 * The "asked" key is the one the earlier connect-time explanation wrote, so a user who answered it is not asked again.
 */
class BatteryPrompt(private val store: PrefStore, private val isExempt: () -> Boolean = { true }) {
    private var requesting = false
    private val mutableStep = MutableStateFlow(stepNow())
    private val mutableCard = MutableStateFlow(cardVisible())

    /** Where the step stands; see [keepAliveStep]. */
    val step: StateFlow<KeepAliveStep> = mutableStep.asStateFlow()

    /** The Home card is visible: the exemption was declined (or never granted) and the card is not dismissed. */
    val card: StateFlow<Boolean> = mutableCard.asStateFlow()

    /** Whether adding a host ends on the step now: never asked, and the app is not exempt. */
    fun shouldOffer(): Boolean = stepNow() == KeepAliveStep.ASK

    /**
     * The user answered the step: recorded at once, so it never shows again. "Not now" ([allow] false) leaves the
     * Home card; "Allow" waits ([KeepAliveStep.WAIT]) until [requestClosed]. Returns false when there was nothing to
     * answer (already answered: a second tap, or a step restored after its answer), and the caller does nothing.
     */
    fun answer(allow: Boolean): Boolean {
        if (stepNow() != KeepAliveStep.ASK) return false
        store.putBoolean(ASKED, true)
        if (allow) requesting = true else store.putBoolean(DECLINED, true)
        refresh()
        return true
    }

    /**
     * The system's request closed (or could not be opened). Whatever it answered, the exemption is read afresh: if it
     * is not in place, the card offers it again. The step is done either way.
     */
    fun requestClosed() {
        requesting = false
        if (!isExempt()) store.putBoolean(DECLINED, true)
        refresh()
    }

    /** The user dismissed the card for good. */
    fun dismissCard() {
        store.putBoolean(CARD_DISMISSED, true)
        refresh()
    }

    /** Re-reads whether the exemption is in place: the card goes away once it is, and a pending step is done. */
    fun refresh() {
        mutableStep.value = stepNow()
        mutableCard.value = cardVisible()
    }

    private fun stepNow() = keepAliveStep(store.getBoolean(ASKED), isExempt(), requesting)

    private fun cardVisible() = store.getBoolean(DECLINED) && !store.getBoolean(CARD_DISMISSED) && !isExempt()

    private companion object {
        // The same keys as the connect-time explanation this step replaced: an answer given there still counts.
        const val ASKED = "battery_asked"
        const val DECLINED = "battery_declined"
        const val CARD_DISMISSED = "battery_card_dismissed"
    }
}

/** What "Allow" on a notification offer does. */
enum class NotificationGrant {
    /** Android's own permission dialog. */
    REQUEST,

    /** The app's notification settings: Android no longer shows its dialog (denied for good). */
    SETTINGS,
}

/**
 * `POST_NOTIFICATIONS`, asked only in context, never on connect (minSdk 34: it is always a runtime permission).
 * [granted] reads the permission; [grant] says whether "Allow" shows Android's dialog or the app's notification
 * settings (after a request, Android stops showing its dialog once the user denied it for good, which is when
 * `shouldShowRequestPermissionRationale` is false again). Nothing asks on connect: the one in-context [offer] (Home's
 * "Show connection and agent notifications" card) and the Settings switch both call `AppActions.allowNotifications`.
 */
class NotificationPermission(private val store: PrefStore, private val granted: () -> Boolean) {
    /** The one offer: connection status and agent alerts together. */
    val offer = NotificationOffer(store, granted)

    fun isGranted(): Boolean = granted()

    /** [rationale] is `shouldShowRequestPermissionRationale(POST_NOTIFICATIONS)` now. */
    fun grant(rationale: Boolean): NotificationGrant = if (wasRequested() && !rationale) NotificationGrant.SETTINGS else NotificationGrant.REQUEST

    /** Android's dialog is being shown (called just before it is launched). */
    fun requested() = store.putBoolean(REQUESTED, true)

    /** The permission may have changed (its dialog closed, or the app returned from Settings): the offer re-reads it. */
    fun refresh() = offer.refresh()

    /** `notifications_asked` is the flag of the connect-time request the offers replaced: that request counts as one. */
    private fun wasRequested() = store.getBoolean(REQUESTED) || store.getBoolean(LEGACY_ASKED)

    private companion object {
        const val REQUESTED = "notifications_requested"
        const val LEGACY_ASKED = "notifications_asked"
    }
}

/**
 * The in-context offer: [visible] while the permission is not granted and the user has not dismissed it. It stays
 * after a denial, so the user can still allow it (through Settings once Android stops asking), until it is dismissed.
 */
class NotificationOffer internal constructor(private val store: PrefStore, private val granted: () -> Boolean) {
    private val mutableVisible = MutableStateFlow(visibleNow())
    val visible: StateFlow<Boolean> = mutableVisible.asStateFlow()

    fun dismiss() {
        store.putBoolean(DISMISSED, true)
        refresh()
    }

    fun refresh() {
        mutableVisible.value = visibleNow()
    }

    private fun visibleNow() = !granted() && !store.getBoolean(DISMISSED)

    private companion object {
        /**
         * The agent-alerts offer's key (v0.1.1, the card that covers both). The connection-only card's dismissal
         * (`notification_offer_connection_dismissed`) and the connect-time request did not hide it, and still do not.
         */
        const val DISMISSED = "notification_offer_agents_dismissed"
    }
}

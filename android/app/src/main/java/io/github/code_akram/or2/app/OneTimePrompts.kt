package io.github.code_akram.or2.app

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

/**
 * The one-time battery-optimisation explanation. A session open while the app goes to the
 * background marks the prompt due ([onBackgrounded]); the next time the app is in the foreground
 * [takeIfDue] says whether to explain now and records that it was shown, whatever the user answers:
 * it never nags again. [isExempt] reads `PowerManager.isIgnoringBatteryOptimizations`; an app that
 * is already exempt is never asked (and counts as asked).
 */
class BatteryPrompt(private val store: PrefStore, private val isExempt: () -> Boolean = { true }) {
    fun onBackgrounded(sessionOpen: Boolean) {
        if (!sessionOpen || store.getBoolean(ASKED) || store.getBoolean(DUE)) return
        if (isExempt()) {
            store.putBoolean(ASKED, true)
            return
        }
        store.putBoolean(DUE, true)
    }

    /** True exactly once, when the explanation is due and should be shown now. */
    fun takeIfDue(): Boolean {
        if (!store.getBoolean(DUE) || store.getBoolean(ASKED)) return false
        store.putBoolean(DUE, false)
        store.putBoolean(ASKED, true)
        return !isExempt()
    }

    private companion object {
        const val ASKED = "battery_asked"
        const val DUE = "battery_due"
    }
}

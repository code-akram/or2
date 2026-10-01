package io.github.code_akram.or2.pair

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.github.code_akram.or2.data.AppDao

/**
 * Holds the pairing flow across activity recreation (a rotation or the camera permission dialog must not
 * lose a scanned code). The flow is cancelled, and its code wiped, when the activity is finished for good.
 */
class PairViewModel(dao: AppDao, describeKeyError: (Throwable) -> String) : ViewModel() {
    val flow = PairFlow(NativePair, DaoPairStore(dao), viewModelScope, describeKeyError)

    override fun onCleared() {
        flow.cancel()
    }
}

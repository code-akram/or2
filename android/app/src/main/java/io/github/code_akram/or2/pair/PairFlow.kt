package io.github.code_akram.or2.pair

import io.github.code_akram.or2.data.AppDao
import io.github.code_akram.or2.data.Host
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.KeyRecord
import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairOffer
import io.github.code_akram.or2.ffi.PairParseException
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.pairSubmitKey
import io.github.code_akram.or2.ffi.parsePairPayload
import io.github.code_akram.or2.hosts.AddressDraft
import io.github.code_akram.or2.hosts.hostFieldError
import io.github.code_akram.or2.hosts.validHost
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** The native parser and exchange as the flow uses them: [NativePair] in the app, fakes in tests. */
interface PairBackend {
    /** @throws PairParseException when [text] is not a valid pairing code. */
    fun parse(text: String): PairOffer

    /** Sends [publicKeyLine] to the host and returns once its user confirmed. @throws PairException */
    suspend fun submit(offer: PairOffer, publicKeyLine: String, deviceLabel: String)
}

object NativePair : PairBackend {
    override fun parse(text: String) = parsePairPayload(text)
    override suspend fun submit(offer: PairOffer, publicKeyLine: String, deviceLabel: String) =
        pairSubmitKey(offer, publicKeyLine, deviceLabel)
}

/** Saves a paired host together with its trusted host key, in one step. */
interface PairStore {
    /**
     * Stores [host] (id 0 is new) with [hostKey] as its trusted host key, atomically, and returns the stored
     * host. Called before the first connection, so that connection finds the key already trusted.
     */
    suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host
}

class DaoPairStore(private val dao: AppDao) : PairStore {
    override suspend fun saveTrustedHost(host: Host, hostKey: PublicKeyInfo): Host {
        val id = dao.saveHostWithTrust(host, hostKey)
        return checkNotNull(dao.host(id)) { "The paired host was not stored." }
    }
}

/** Which of the phone's keys is authorized on the host. */
sealed interface KeyChoice {
    data class Existing(val keyId: String) : KeyChoice
    data object New : KeyChoice
}

/** The first stored key, or a new one when there is none. */
fun defaultKeyChoice(keys: List<KeyRecord>): KeyChoice = keys.firstOrNull()?.let { KeyChoice.Existing(it.id) } ?: KeyChoice.New

/** What the review screen edits: the offer and the choices made about it. */
data class PairReview(
    val offer: PairOffer,
    val name: String,
    val username: String,
    val choice: KeyChoice,
    /** Why the last attempt failed; shown above the button. */
    val error: String? = null,
    /** The fingerprint of the key the host already accepted, when only saving the host failed: no second exchange. */
    val acceptedKey: String? = null,
    /** An attempt is running (asking for the biometric, or waiting for the host); the button is off. */
    val working: Boolean = false,
) {
    /** Whether a pairing code with a listener was scanned; without one the user installs the key by hand. */
    val listens get() = offer.exchange != null

    /** The label, user and addresses make a host the app can save. */
    val valid get() = validHost(
        name, offer.addresses.map { AddressDraft(it.host, it.port.toString()) }, username,
    )
}

sealed interface PairState {
    /** Waiting for a code, from the camera or pasted; [error] explains the last one that was refused. */
    data class Scanning(val error: String? = null) : PairState

    data class Review(val review: PairReview) : PairState

    /** The key is with the host, whose user is asked to confirm [phoneFingerprint]. */
    data class Submitting(val review: PairReview, val phoneFingerprint: String) : PairState

    /** A code without a listener: the host is saved and trusted, and the key waits to be installed by hand. */
    data class KeyToInstall(val host: Host, val keyLine: String, val fingerprint: String) : PairState

    /** Done: the host is saved with its key trusted. The screen connects to it and calls [PairFlow.consume]. */
    data class Paired(val host: Host) : PairState
}

/**
 * Easy pair on the phone: scan or paste, review, send the key, save the host with its host key
 * trusted. Logic only (the screens and the camera are elsewhere), over [PairBackend] and [PairStore].
 *
 * Rules:
 * - the code and its password are never logged, and the password is wiped once it has served (after
 *   the exchange, and whenever the flow is left, reset or cancelled);
 * - the host is saved, with the host key from the code trusted, only after the host accepted the
 *   key (or at once for a code without a listener), and before anything connects;
 * - a key generated for the pairing is saved first, so a failed exchange leaves a key the retry reuses.
 */
class PairFlow(
    private val backend: PairBackend,
    private val store: PairStore,
    private val scope: CoroutineScope,
    /** Words for a failure while generating or saving the phone's key (biometric cancelled, vault errors). */
    private val describeKeyError: (Throwable) -> String = { "The key could not be created. Try again." },
) {
    private val mutableState = MutableStateFlow<PairState>(PairState.Scanning())
    val state: StateFlow<PairState> = mutableState.asStateFlow()
    private var job: Job? = null

    /** Back to scanning, dropping (and wiping) whatever was in progress. */
    fun start() {
        job?.cancel()
        wipe(mutableState.value)
        mutableState.value = PairState.Scanning()
    }

    /** The scanner or the paste field produced [text]. Ignored unless the flow is scanning. Returns whether it was taken. */
    fun onCode(text: String, keys: List<KeyRecord>): Boolean {
        if (mutableState.value !is PairState.Scanning) return false
        val offer = try {
            backend.parse(text)
        } catch (error: PairParseException) {
            mutableState.value = PairState.Scanning(pairParseMessage(error))
            return false
        }
        mutableState.value = PairState.Review(
            PairReview(offer, name = offer.name, username = offer.username, choice = defaultKeyChoice(keys)),
        )
        return true
    }

    fun edit(name: String? = null, username: String? = null, choice: KeyChoice? = null) {
        val current = (mutableState.value as? PairState.Review)?.review ?: return
        if (current.working) return
        mutableState.value = PairState.Review(
            current.copy(
                name = name ?: current.name, username = username ?: current.username,
                choice = choice ?: current.choice, error = null,
            ),
        )
    }

    /** Back from the review to scanning, wiping the code. */
    fun rescan() = start()

    /**
     * Authorizes the chosen key on the host, saves the host with its key trusted, and ends in [PairState.Paired]
     * (or [PairState.KeyToInstall] for a code without a listener). A failure returns to the review with the reason.
     * [generateKey] creates and stores a new key (it asks for the biometric); it is called only for [KeyChoice.New].
     */
    fun submit(keys: List<KeyRecord>, device: String, generateKey: suspend (label: String, comment: String) -> KeyRecord) {
        val review = (mutableState.value as? PairState.Review)?.review ?: return
        if (review.working) return
        if (!review.valid) {
            mutableState.value = PairState.Review(review.copy(error = "Enter a name and a user name for this host."))
            return
        }
        mutableState.value = PairState.Review(review.copy(error = null, working = true))
        job = scope.launch { run(review, keys, device, generateKey) }
    }

    private suspend fun run(
        review: PairReview, keys: List<KeyRecord>, device: String,
        generateKey: suspend (String, String) -> KeyRecord,
    ) {
        fun fail(from: PairReview, message: String) {
            mutableState.value = PairState.Review(from.copy(error = message, working = false))
        }
        val key = when (val choice = review.choice) {
            is KeyChoice.Existing -> keys.find { it.id == choice.keyId } ?: return fail(review, "That key was deleted. Choose another.")
            KeyChoice.New -> try {
                generateKey("Key for ${review.name.trim()}", "or2@${device.trim()}")
            } catch (error: CancellationException) {
                throw error
            } catch (error: Exception) {
                return fail(review, describeKeyError(error))
            }
        }
        // A key made just now is stored: a retry after a failed exchange picks it up as an existing one.
        val kept = if (review.choice == KeyChoice.New) review.copy(choice = KeyChoice.Existing(key.id)) else review
        val exchange = review.offer.exchange
        if (exchange != null && kept.acceptedKey != key.fingerprint) {
            mutableState.value = PairState.Submitting(kept, key.fingerprint)
            try {
                backend.submit(review.offer, key.openssh, device)
            } catch (error: PairException) {
                return fail(kept, pairErrorMessage(error))
            }
        }
        val saved = try {
            store.saveTrustedHost(
                Host(
                    HostRecord(0, review.name.trim(), review.username.trim(), key.id),
                    review.offer.addresses.map { HostEndpoint(it.host, it.port.toInt()) },
                ),
                review.offer.hostKey,
            )
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            // The host has the key already, and the code is spent: a retry saves the host without exchanging again.
            return fail(
                kept.copy(acceptedKey = key.fingerprint.takeIf { exchange != null }),
                "The host accepted the key, but this phone could not save the host. Try again.",
            )
        }
        wipe(PairState.Review(review))
        mutableState.value =
            if (exchange != null) PairState.Paired(saved) else PairState.KeyToInstall(saved, key.openssh, key.fingerprint)
    }

    /** Takes the paired host (or ends the key-to-install screen) and returns to scanning. */
    fun consume(): Host? {
        val host = when (val current = mutableState.value) {
            is PairState.Paired -> current.host
            is PairState.KeyToInstall -> current.host
            else -> null
        }
        if (host != null) mutableState.value = PairState.Scanning()
        return host
    }

    /** The user left the pairing screens: abort whatever runs, wipe the code, forget everything. */
    fun cancel() {
        job?.cancel()
        job = null
        wipe(mutableState.value)
        mutableState.value = PairState.Scanning()
    }

    private fun wipe(state: PairState) {
        val review = when (state) {
            is PairState.Review -> state.review
            is PairState.Submitting -> state.review
            else -> return
        }
        review.offer.exchange?.secret?.wipe()
    }
}

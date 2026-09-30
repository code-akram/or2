package io.github.code_akram.or2.session

import io.github.code_akram.or2.data.HostRecord
import io.github.code_akram.or2.data.TrustStore
import io.github.code_akram.or2.ffi.ConnectRequest
import io.github.code_akram.or2.ffi.SessionInterface
import io.github.code_akram.or2.ffi.SessionListener
import io.github.code_akram.or2.ffi.SessionState
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

fun interface SessionConnector {
    fun connect(request: ConnectRequest, listener: SessionListener): SessionInterface
}

// Removed when lane A's real connect export is integrated. Never substitute the probe here.
class ConnectionUnavailable : IllegalStateException("SSH connection is not available in this build.")

class ActiveSession(val host: HostRecord) {
    internal val ready = CompletableDeferred<SessionInterface>()
    internal val mutableState = MutableStateFlow<SessionState>(SessionState.Connecting)
    internal val mutableFrames = MutableSharedFlow<Unit>(replay = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    internal val mutableHandle = MutableStateFlow<SessionInterface?>(null)
    val state = mutableState.asStateFlow()
    val frameReady = mutableFrames.asSharedFlow()
    val handle = mutableHandle.asStateFlow()
}

/** Application-owned. All bookkeeping and listener delivery are confined to the main dispatcher. */
class SessionHolder(
    private val connector: SessionConnector,
    private val trust: TrustStore,
    main: CoroutineDispatcher = Dispatchers.Main.immediate,
    private val worker: CoroutineDispatcher = Dispatchers.Default,
) {
    private val scope = CoroutineScope(SupervisorJob() + main)
    private val mutableActive = MutableStateFlow<ActiveSession?>(null)
    val active = mutableActive.asStateFlow()

    // Call on main. The caller transfers the decrypted array; it is wiped after the factory call.
    suspend fun connect(host: HostRecord, privateKey: ByteArray) {
        var attempt: ActiveSession? = null
        try {
            disconnect()
            val keys = trust.trustedKeys(host.id)
            val current = ActiveSession(host)
            attempt = current
            mutableActive.value = current
            val listener = object : SessionListener {
                override fun onStateChanged(state: SessionState) {
                    scope.launch { current.mutableState.value = state }
                }

                override fun onFrameReady() {
                    scope.launch { current.mutableFrames.emit(Unit) }
                }
            }
            val session = withContext(worker) {
                try {
                    connector.connect(
                        ConnectRequest(host.hostname, host.port.toUShort(), host.username, privateKey, keys, 80u, 24u),
                        listener,
                    ).also {
                        // Store before returning through the cancellable dispatcher boundary.
                        current.mutableHandle.value = it
                        current.ready.complete(it)
                    }
                } finally {
                    privateKey.fill(0)
                }
            }
            // Disconnect may have happened while the synchronous factory was running.
            if (mutableActive.value !== current) release(session)
        } catch (error: Exception) {
            attempt?.ready?.completeExceptionally(error)
            attempt?.mutableHandle?.value?.let(::release)
            if (mutableActive.value === attempt) mutableActive.value = null
            throw error
        } finally {
            privateKey.fill(0)
        }
    }

    suspend fun approve(current: ActiveSession, prompt: SessionState.AwaitingHostKeyDecision) {
        val session = current.ready.await()
        check(mutableActive.value === current && current.state.value == prompt) { "Host-key prompt has expired." }
        trust.replaceTrust(current.host, prompt.presented)
        // Persistence failure must never cause approval.
        if (mutableActive.value === current && current.state.value == prompt) {
            session.approveHostKey(prompt.presented.fingerprint)
        }
    }

    suspend fun reject(current: ActiveSession) {
        val session = current.ready.await()
        if (mutableActive.value === current) session.rejectHostKey()
    }

    fun disconnect() {
        val current = mutableActive.value ?: return
        mutableActive.value = null
        current.mutableHandle.value?.let(::release)
    }

    private fun release(session: SessionInterface) {
        session.disconnect()
        (session as? AutoCloseable)?.close()
    }
}

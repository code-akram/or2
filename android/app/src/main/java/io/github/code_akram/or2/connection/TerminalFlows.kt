package io.github.code_akram.or2.connection

import io.github.code_akram.or2.ffi.SessionState
import io.github.code_akram.or2.ffi.TerminalTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/** The real transport of every open terminal, keyed by terminal id (it changes when AUTO falls back). */
@OptIn(ExperimentalCoroutinesApi::class)
fun HostConnections.transports(): Flow<Map<Long, TerminalTransport>> = terminals.flatMapLatest { list ->
    if (list.isEmpty()) flowOf(emptyMap())
    else combine(list.map { t -> t.transport.map { t.id to it } }) { it.toMap() }
}

/** Whether each open terminal has closed, keyed by terminal id. */
@OptIn(ExperimentalCoroutinesApi::class)
fun HostConnections.terminalClosedStates(): Flow<Map<Long, Boolean>> = terminals.flatMapLatest { list ->
    if (list.isEmpty()) flowOf(emptyMap())
    else combine(list.map { t -> t.state.map { t.id to (it is SessionState.Closed) } }) { it.toMap() }
}

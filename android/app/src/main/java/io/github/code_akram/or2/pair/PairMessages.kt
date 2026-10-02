package io.github.code_akram.or2.pair

import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairParseException

/** Why a scanned or pasted code was refused, for the line under the scanner. */
fun pairParseMessage(error: PairParseException): String = when (error) {
    is PairParseException.NotPairingCode ->
        "That is not an or2 pairing code. Run or2-pair on the host and scan the code it prints."
    // Version 1 is the old listener design: the fix is on the host. Anything above 2 is a newer host than this app.
    is PairParseException.UnsupportedVersion ->
        if (error.version < 2u) "This code is from an older or2-pair: update it on the host." else "Update or2 to use this code."
    is PairParseException.TooLong -> "That text is too long to be a pairing code."
    is PairParseException.Malformed -> "The pairing code is damaged. Scan it again, or paste its text."
    is PairParseException.MissingField, is PairParseException.InvalidField, is PairParseException.DuplicateField,
    is PairParseException.UnknownField -> "The pairing code is not valid. Run or2-pair again and scan the new code."
}

/**
 * Why pairing failed, for the screen that explains it: one line, from the host's [name] and the SSH [port] it was
 * tried on. Nothing here carries the pairing code `K`, the QR or a key.
 */
fun pairErrorMessage(error: PairException, name: String, port: Int): String = when (error) {
    is PairException.Unreachable ->
        "Couldn't reach $name on port $port. Pairing uses the same SSH port as connecting: the phone must reach it " +
            "(same network, ZeroTier or Tailscale, or a public address)."
    is PairException.HostKeyMismatch -> "The host presented a different key than the code. Nothing was sent."
    // sshd refused the key derived from the code: the three causes, none of which the phone can tell apart.
    is PairException.BootstrapRefused ->
        "The host didn't accept the pairing key. The code typed into or2-pair may differ, or2-pair may have stopped, " +
            "or sshd may not read ~/.ssh/authorized_keys. Run or2-pair again."
    is PairException.NotOr2Pair -> "Something other than or2-pair answered on the host. Pair manually."
    is PairException.Expired -> "or2-pair has stopped or timed out on the host. Run it again."
    is PairException.Gone -> "Another device already used this pairing."
    // The contract's sentence first, then the reason where one is known.
    is PairException.KeyNotAccepted -> "The host couldn't add the key. It does not accept this kind of key: use an Ed25519 key."
    is PairException.HostFailed -> "The host couldn't add the key. Read what or2-pair printed on the host."
    is PairException.TimedOut -> "$name did not answer in time. Run or2-pair again and retry."
    is PairException.ConnectionLost -> "The connection to the host ended early. Run or2-pair again and retry."
    is PairException.Protocol -> "The host did not understand the request. Update or2-pair on the host and try again."
    is PairException.Refused -> "The host refused the request. Run or2-pair again and retry."
    is PairException.InvalidKey -> "This key cannot be sent to the host. Pick another key."
    is PairException.InvalidDevice -> "This phone's name cannot be sent to the host."
    is PairException.NoPairingId -> "This code is for a host set up by hand: install the key on the host yourself."
    is PairException.InvalidOffer -> "This pairing code cannot be used. Scan a new one."
}

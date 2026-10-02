package io.github.code_akram.or2.pair

import io.github.code_akram.or2.ffi.PairException
import io.github.code_akram.or2.ffi.PairParseException
import io.github.code_akram.or2.ffi.parsePairPayload
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Every refusal has words a person can act on, and none of them carries the code or a key. */
class PairMessagesTest {
    private val everyPairError = listOf(
        PairException.NoPairingId(), PairException.InvalidOffer(), PairException.InvalidKey(), PairException.InvalidDevice(),
        PairException.Unreachable(), PairException.TimedOut(), PairException.HostKeyMismatch(), PairException.BootstrapRefused(),
        PairException.NotOr2Pair(), PairException.Protocol(), PairException.ConnectionLost(), PairException.Expired(),
        PairException.Gone(), PairException.KeyNotAccepted(), PairException.HostFailed(), PairException.Refused(),
    )

    private val everyParseError = listOf(
        PairParseException.NotPairingCode(), PairParseException.UnsupportedVersion(1u), PairParseException.UnsupportedVersion(3u),
        PairParseException.TooLong(), PairParseException.Malformed(), PairParseException.MissingField("user"),
        PairParseException.DuplicateField("a"), PairParseException.UnknownField(), PairParseException.InvalidField("id"),
    )

    private fun message(error: PairException) = pairErrorMessage(error, "Work Mac", 2222)

    @Test
    fun theMessagesOfTheContractAreExact() {
        assertEquals(
            "Couldn't reach Work Mac on port 2222. Pairing uses the same SSH port as connecting: the phone must reach it " +
                "(same network, ZeroTier or Tailscale, or a public address).",
            message(PairException.Unreachable()),
        )
        assertEquals("The host presented a different key than the code. Nothing was sent.", message(PairException.HostKeyMismatch()))
        assertEquals(
            "The host didn't accept this phone's code. Check the code typed into or2-pair, or run it again.",
            message(PairException.BootstrapRefused()),
        )
        assertEquals("Something other than or2-pair answered on the host. Pair manually.", message(PairException.NotOr2Pair()))
        assertEquals("or2-pair has stopped or timed out on the host. Run it again.", message(PairException.Expired()))
        assertEquals("Another device already used this pairing.", message(PairException.Gone()))
        // The host's failure to add the key starts with the contract's sentence; a reason may follow.
        assertTrue(message(PairException.KeyNotAccepted()).startsWith("The host couldn't add the key."))
        assertTrue(message(PairException.HostFailed()).startsWith("The host couldn't add the key."))
        assertEquals("This code is from an older or2-pair: update it on the host.", pairParseMessage(PairParseException.UnsupportedVersion(1u)))
        assertEquals("Update or2 to use this code.", pairParseMessage(PairParseException.UnsupportedVersion(3u)))
    }

    @Test
    fun theUnreachableMessageNamesTheHostAndThePortItWasTriedOn() {
        assertTrue(pairErrorMessage(PairException.Unreachable(), "Lab box", 22).startsWith("Couldn't reach Lab box on port 22."))
    }

    @Test
    fun everyPairErrorHasASentenceThatNamesWhatToDo() {
        for (error in everyPairError) {
            val text = message(error)
            assertTrue("${error::class.simpleName}: $text", text.length > 20 && text.endsWith("."))
        }
    }

    @Test
    fun everyParseErrorHasASentence() {
        for (error in everyParseError) {
            val text = pairParseMessage(error)
            assertTrue("${error::class.simpleName}: $text", text.length > 20 && text.endsWith("."))
        }
    }

    @Test
    fun theRealParserRaisesTheErrorsTheMessagesAreWrittenFor() {
        for ((text, expected) in listOf(
            "not a code" to PairParseException.NotPairingCode::class,
            "or2-pair:9?x=y" to PairParseException.UnsupportedVersion::class,
            "or2-pair:1?name=x" to PairParseException.UnsupportedVersion::class,
            "or2-pair:2?" to PairParseException.Malformed::class,
            "or2-pair:2?name=x" to PairParseException.MissingField::class,
        )) {
            try {
                parsePairPayload(text)
                fail("$text should not parse")
            } catch (error: PairParseException) {
                assertEquals(text, expected, error::class)
            }
        }
    }

    @Test
    fun aVersionOneCodeIsOlderAndAHigherOneIsNewer() {
        for ((text, words) in listOf("or2-pair:1?name=x" to "older or2-pair", "or2-pair:3?name=x" to "Update or2")) {
            try {
                parsePairPayload(text)
                fail("$text should not parse")
            } catch (error: PairParseException) {
                assertTrue(pairParseMessage(error), pairParseMessage(error).contains(words))
            }
        }
    }

    @Test
    fun noMessageCarriesACodeOrAnId() {
        val all = everyPairError.map(::message) + everyParseError.map(::pairParseMessage)
        for (text in all) {
            assertFalse(text, text.contains("or2-pair:2?"))
            assertFalse(text, text.contains("id="))
        }
    }
}

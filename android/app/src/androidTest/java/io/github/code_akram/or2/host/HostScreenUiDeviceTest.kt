package io.github.code_akram.or2.host

import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import io.github.code_akram.or2.MainActivity
import io.github.code_akram.or2.connection.uiHost
import io.github.code_akram.or2.data.HostEndpoint
import io.github.code_akram.or2.ffi.CloseReason
import io.github.code_akram.or2.ffi.HerdrSessionInfo
import io.github.code_akram.or2.ffi.HostCapabilities
import io.github.code_akram.or2.ffi.HostState
import io.github.code_akram.or2.ffi.PublicKeyInfo
import io.github.code_akram.or2.ffi.SessionFailure
import io.github.code_akram.or2.ffi.TmuxSession
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/** The host screen with fabricated state: no connection, Keystore or database is touched. */
class HostScreenUiDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private val host = uiHost(4, "Build box", addresses = listOf(HostEndpoint("a.invalid", 22), HostEndpoint("b.invalid", 2222)))
    private val caps = HostCapabilities("/usr/bin/tmux", "/usr/bin/herdr", null, "C.UTF-8", listOf(
        HerdrSessionInfo("default", true, true), HerdrSessionInfo("work", true, false), HerdrSessionInfo("old", false, false)))
    private val tmux = TmuxList.Loaded(listOf(TmuxSession("main", 3u, 1u, 0L, 20L), TmuxSession("build", 1u, 0u, 0L, 10L)))

    private class Calls {
        val log = mutableListOf<String>()
    }

    private fun show(
        state: HostState?, caps: HostCapabilities? = this.caps, tmux: TmuxList = this.tmux, capsError: String? = null,
        calls: Calls = Calls(), busy: Boolean = false, host: io.github.code_akram.or2.data.Host = this.host,
    ) = compose.runOnUiThread {
        compose.activity.setContent {
            MaterialTheme {
                HostScreen(host, state, caps, capsError, tmux, busy,
                    connect = { calls.log += "connect" }, disconnect = { calls.log += "disconnect" },
                    approve = { calls.log += "approve:${it.presented.fingerprint}" }, reject = { calls.log += "reject" },
                    openShell = { calls.log += "shell" }, openTmux = { calls.log += "tmux:$it" },
                    openHerdr = { calls.log += "herdr:$it" }, refresh = { calls.log += "refresh" })
            }
        }
    }

    @Test
    fun aDisconnectedHostOffersUnlockAndConnectAndHidesTheMultiplexers() {
        val calls = Calls()
        show(null, calls = calls)
        compose.onNodeWithText("Build box").assertIsDisplayed()
        compose.onNodeWithText("fixture-user@a.invalid:22, b.invalid:2222").assertIsDisplayed()
        compose.onNodeWithTag("host-connect").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(listOf("connect"), calls.log) }
        compose.onNodeWithTag("host-shell").assertDoesNotExist()
        compose.onNodeWithText("tmux").assertDoesNotExist()
    }

    @Test
    fun aHostWithoutAKeyCannotConnect() {
        show(null, host = uiHost(5, "Keyless", keyId = null))
        compose.onNodeWithTag("host-connect").assertIsNotEnabled()
        compose.onNodeWithText("Select a key first").assertIsDisplayed()
    }

    @Test
    fun aFailedConnectionExplainsItselfAndCanRetry() {
        show(HostState.Closed(CloseReason.Failed(SessionFailure.AuthenticationRejected)))
        compose.onNodeWithText("Authentication rejected. Check the username and public-key authorization.").assertIsDisplayed()
        compose.onNodeWithTag("host-connect").assertIsEnabled()
    }

    @Test
    fun theHostKeyPromptIsAnsweredOnTheHostScreen() {
        val calls = Calls()
        val prompt = HostState.AwaitingHostKeyDecision(PublicKeyInfo("ssh-ed25519", "k", "SHA256:presented", ""), emptyList())
        show(prompt, calls = calls)
        compose.onNodeWithText("Waiting for host-key approval").assertIsDisplayed()
        compose.onNodeWithText("Trust this host key?").assertIsDisplayed()
        compose.onNodeWithText("SHA256:presented").assertIsDisplayed()
        compose.onNodeWithText("Trust and connect").performClick()
        compose.runOnIdle { assertEquals(listOf("approve:SHA256:presented"), calls.log) }
        show(prompt, calls = calls)
        compose.onNodeWithText("Reject").performClick()
        compose.runOnIdle { assertEquals("reject", calls.log.last()) }
    }

    @Test
    fun aConnectedHostOffersShellTmuxSessionsAndHerdrSessions() {
        val calls = Calls()
        show(HostState.Connected(1u), calls = calls)
        compose.onNodeWithText("Using address 2: b.invalid:2222").assertIsDisplayed()
        compose.onNodeWithTag("host-shell").performClick()
        compose.onNodeWithTag("tmux-attach:build").performScrollTo().performClick()
        compose.onNodeWithText("3 windows · 1 attached").performScrollTo().assertIsDisplayed()
        // The default session opens without a name; a named one by name; a stopped one not at all.
        compose.onNodeWithTag("herdr-open:default").performScrollTo().performClick()
        compose.onNodeWithTag("herdr-open:work").performScrollTo().performClick()
        compose.onNodeWithTag("herdr-open:old").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("host-refresh").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(listOf("shell", "tmux:build", "herdr:null", "herdr:work", "refresh"), calls.log) }
        compose.onNodeWithTag("host-disconnect").performScrollTo().assertIsDisplayed()
    }

    @Test
    fun newTmuxSessionNamesAreValidatedBeforeCreating() {
        val calls = Calls()
        show(HostState.Connected(0u), calls = calls)
        compose.onNodeWithTag("tmux-new").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("a:b")
        compose.onNodeWithText("Remove backslash, colon and dot characters.").assertIsDisplayed()
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        compose.onNodeWithTag("tmux-new-name").performTextInput("x") // "a:bx" is still invalid.
        compose.onNodeWithTag("tmux-new").assertIsNotEnabled()
        show(HostState.Connected(0u), calls = calls)
        compose.onNodeWithTag("tmux-new-name").performScrollTo().performTextInput("my work")
        compose.onNodeWithTag("tmux-new").assertIsEnabled().performClick()
        compose.runOnIdle { assertEquals(listOf("tmux:my work"), calls.log) }
    }

    @Test
    fun missingTmuxHerdrAndFailedListingsAreExplainedNotHidden() {
        show(HostState.Connected(0u), caps = caps.copy(tmux = null, herdr = null))
        compose.onNodeWithTag("tmux-missing").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("herdr-missing").performScrollTo().assertIsDisplayed()
        show(HostState.Connected(0u), tmux = TmuxList.Failed("The connection has closed. Reconnect to continue."))
        compose.onNodeWithText("The connection has closed. Reconnect to continue.").performScrollTo().assertIsDisplayed()
        show(HostState.Connected(0u), caps = null, tmux = TmuxList.Loading)
        compose.onNodeWithText("Checking the host…", substring = true).assertExists()
        show(HostState.Connected(0u), capsError = "probe failed")
        compose.onNodeWithText("Could not query the host. Try Refresh.").performScrollTo().assertIsDisplayed()
    }
}

package ai.astrid.app

import ai.astrid.app.ui.ApprovalHost
import ai.astrid.app.ui.derivePendingApproval
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.core.app.ApplicationProvider
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Phase 5 approval tests on the real UI (Robolectric): a risky command shows
 * the popup; Deny blocks; Approve runs; read-only commands run without
 * asking; a path outside the sandbox folder is refused with a clear error.
 * Every decision lands in the Live trace (asserted through recentEvents).
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w412dp-h915dp")
class ApprovalFlowTest {

    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    private val bridge get() = AstridBridge.bridge

    @Before
    fun hermeticLadder() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        AstridBridge.init(context.filesDir)
        runCatching { bridge.setLadder(listOf("offline")) }
    }

    private fun tagExists(tag: String): Boolean =
        runCatching { compose.onNodeWithTag(tag).assertExists(); true }.getOrDefault(false)

    private fun sandboxFile(name: String): File =
        File(bridge.sandboxDir()).resolve(name)

    @Test
    fun readOnlyRunsFreeAndNothingPending() {
        val rec = bridge.shellCommand("echo approval-free-ok")
        assertEquals("ran", rec.outcome)
        assertTrue(rec.output.contains("approval-free-ok"))
        assertNull("read-only must not open the popup", derivePendingApproval(bridge))
        compose.waitForIdle()
        assertTrue("no popup for read-only", !tagExists("approval/deny"))
    }

    @Test
    fun riskyCommandShowsTheApprovalPopup() {
        val rec = bridge.shellCommand("rm -rf scratch-dir")
        assertEquals("needs_approval", rec.outcome)
        assertTrue(rec.reason.contains("rm -rf"))
        compose.waitForIdle()
        compose.waitUntil(5_000) { tagExists("approval/approve") }
        compose.onNodeWithTag("approval/deny").assertIsDisplayed()
        compose.onNodeWithTag("approval/approve").assertIsDisplayed()
        compose.onNodeWithText("Approval needed").assertIsDisplayed()
        // The popup carries the command, the sandbox folder and the why.
        compose.onNodeWithTag("approval/command").assertIsDisplayed()
        compose.onNodeWithTag("approval/folder").assertIsDisplayed()
        compose.onNodeWithTag("approval/why").assertIsDisplayed()
    }

    @Test
    fun denyBlocksTheCommand() {
        sandboxFile("denied-marker").parentFile.mkdirs()
        sandboxFile("denied-marker").writeText("should never appear")
        val rec = bridge.shellCommand("rm -rf denied-marker")
        assertEquals("needs_approval", rec.outcome)
        compose.waitUntil(5_000) { tagExists("approval/deny") }
        compose.onNodeWithTag("approval/deny").performClick()
        compose.waitForIdle()
        // The file still exists: Deny stopped the command.
        assertTrue("Deny must leave the target alone", sandboxFile("denied-marker").exists())
        val rows = bridge.recentEvents(30u)
        assertTrue(
            "the denial must reach the Live trace",
            rows.any { it.kind == "shell.denied" }
        )
    }

    @Test
    fun approveRunsTheCommand() {
        sandboxFile("approved-marker").writeText("x")
        val rec = bridge.shellCommand("rm -rf approved-marker")
        assertEquals("needs_approval", rec.outcome)
        compose.waitUntil(5_000) { tagExists("approval/approve") }
        compose.onNodeWithTag("approval/approve").performClick()
        compose.waitForIdle()
        compose.waitUntil(5_000) { !tagExists("approval/approve") }
        // The command really ran: the marker is gone.
        assertTrue("Approve must run the command", !sandboxFile("approved-marker").exists())
        val rows = bridge.recentEvents(30u)
        assertTrue(rows.any { it.kind == "shell.approved" })
        assertTrue(rows.any { it.kind == "shell.ran" })
    }

    @Test
    fun aPathOutsideTheSandboxIsRefusedWithAClearError() {
        val rec = bridge.shellCommand("cat /etc/passwd")
        assertEquals("refused", rec.outcome)
        assertTrue(
            "the error must name the sandbox",
            rec.reason.contains("outside the sandbox folder")
        )
        assertNull("a refused command must not open the popup", derivePendingApproval(bridge))
    }
}

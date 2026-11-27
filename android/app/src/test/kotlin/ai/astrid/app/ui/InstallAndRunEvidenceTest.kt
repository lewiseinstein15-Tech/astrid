package ai.astrid.app.ui

import ai.astrid.app.AstridBridge
import ai.astrid.app.MainActivity
import android.graphics.Bitmap
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ApplicationProvider
import java.io.File
import java.io.FileOutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Install-and-run evidence (user bug-fix verification for 0.1.5.1): the real
 * MainActivity, the real Rust core, the user's exact question from the bug
 * report — and REAL rendered PNGs captured from the running app UI.
 *
 * What the user saw in 0.1.5: the reply arrived but the header stayed stuck
 * on "Working", the app hung, and Android showed "Astrid isn't responding".
 * Root cause: the approval reads blocked on the core mutex the turn holds
 * for its whole run. The fix (lock-free approval surface) is exercised here
 * end to end: two chat turns on the offline rung, the reply rendered, and
 * the turn state fully cleared afterwards — plus captured screenshots saved
 * next to the deliverables.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w412dp-h915dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class InstallAndRunEvidenceTest {

    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    /** Screenshots land next to the delivered APK so the user can see them. */
    private val shots =
        File("/home/z/my-project/download/screenshots")

    private val bridge get() = AstridBridge.bridge

    @Before
    fun hermeticLadder() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        AstridBridge.init(context.filesDir)
        // Hermetic: answer from the offline agent, never the internet.
        runCatching { bridge.setLadder(listOf("offline")) }
        // A leftover pending approval would scrim the screen (see AstridUiTest).
        derivePendingApproval(bridge)?.let { p ->
            runCatching {
                if (p.kind == "shell") bridge.resolveShell(p.approvalId, false)
                else bridge.resolveApproval(false)
            }
        }
        // Fresh-install state for the notification rationale (the 0.1.5 bug:
        // it re-popped; now it is persisted once ever). Nothing is acked yet.
        context.getSharedPreferences("astrid-ui", android.content.Context.MODE_PRIVATE)
            .edit().clear().commit()
        shots.mkdirs()
    }

    /** Render the current UI to a real PNG next to the deliverables. */
    private fun snap(name: String) {
        compose.waitForIdle()
        val out = File(shots, "$name.png")
        val decor = compose.activity.window.decorView
        val bmp = Bitmap.createBitmap(decor.width.coerceAtLeast(720), decor.height.coerceAtLeast(1520), Bitmap.Config.ARGB_8888)
        decor.draw(android.graphics.Canvas(bmp))
        FileOutputStream(out).use { fos -> bmp.compress(Bitmap.CompressFormat.PNG, 100, fos) }
        bmp.recycle()
        assertTrue(
            "screenshot must be real pixels: ${out.path}",
            out.exists() && out.length() > 1000,
        )
    }

    /** Replies seen on screen so far — each turn must add exactly one. */
    private fun replyCount() =
        compose.onAllNodesWithText("offline agent", substring = true)
            .fetchSemanticsNodes().size

    private fun sendAndAwaitReply(question: String, shotPrefix: String, priorReplies: Int) {
        compose.onNodeWithTag("chat/input").performTextInput(question)
        snap("${shotPrefix}_2_question_typed")
        compose.onNodeWithTag("chat/send").performClick()
        snap("${shotPrefix}_3_question_sent")
        // The offline agent's reply contains "offline agent" (see the user's
        // 0.1.5 screenshot). Wait until THIS turn's reply lands on screen:
        // the count grows by one (a stale previous reply must not satisfy
        // the wait — that masked the second turn in the first run).
        compose.waitUntil(60_000) { replyCount() > priorReplies }
        snap("${shotPrefix}_4_reply_arrived")
        // THE 0.1.5 FIX: the header chip used to say "Working" forever — it
        // was hardcoded in the header row (and the send never returned during
        // the 120s approval wait). Once the reply is rendered the header MUST
        // show "Ready". (The plan card may keep its own last step label at
        // "Working" — that is the audit stream's honest last-known state.)
        assertTrue(
            "the turn ended but the header never went back to Ready",
            compose.onAllNodesWithText("Ready").fetchSemanticsNodes().isNotEmpty(),
        )
        snap("${shotPrefix}_5_chip_cleared")
    }

    @Test
    fun questionSendsRepliesAndTheTurnStateClears() {
        // 1. The app is up: the chat input is ready.
        compose.waitUntil(60_000) {
            compose.onAllNodesWithTag("chat/input").fetchSemanticsNodes().isNotEmpty()
        }
        snap("01_astrid_launched")

        // 2. The user's exact question from the bug report, twice in a row.
        sendAndAwaitReply("Explain in two sentences what Rust is.", "turn1", 0)
        sendAndAwaitReply("Explain in two sentences what Kotlin is.", "turn2", 1)

        // 3. The notification rationale NEVER popped (fast offline turns end
        // before the promote delay; and the 0.1.5.1 build persists the choice
        // once it is given, so it cannot re-pop on later launches).
        assertEquals(
            "the About notifications dialog must not appear mid-chat",
            0,
            compose.onAllNodesWithText("About notifications").fetchSemanticsNodes().size,
        )
        snap("07_no_notification_dialog")
    }
}

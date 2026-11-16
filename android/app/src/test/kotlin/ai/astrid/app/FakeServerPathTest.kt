package ai.astrid.app

import android.content.Intent
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import com.sun.net.httpserver.HttpServer
import java.net.InetSocketAddress
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * THE app-path proof (Phase 4.1): a local fake OpenAI-compatible HTTP server
 * on 127.0.0.1 stands in for a provider rung; the send goes through the REAL
 * stack — Compose send → bridge.sendMessage → Rust core → ladder → reqwest →
 * HTTP → parsed reply → SQLite → bubble. When this test is green, the model
 * path works end to end; the same reqwest/rustls client then talks to the
 * built-in keyless rungs (kilo-auto/free, Pollinations) on a real device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w412dp-h915dp")
class FakeServerPathTest {

    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    /** OpenAI-compatible fake: one canned completion on the chat endpoint. */
    private fun startFakeServer(): HttpServer {
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext("/v1/chat/completions") { exchange ->
            val body = """
                {"id":"fake-1","object":"chat.completion","created":1,"model":"fake-model",
                 "choices":[{"index":0,"finish_reason":"stop",
                   "message":{"role":"assistant","content":"FAKE-REPLY-OK"}}],
                 "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}
            """.trimIndent().toByteArray()
            exchange.responseHeaders.add("Content-Type", "application/json")
            exchange.sendResponseHeaders(200, body.size.toLong())
            exchange.responseBody.use { it.write(body) }
        }
        server.start()
        return server
    }

    @Test
    fun aSendRunsTheRealHttpPathAndTheBubbleShowsTheAnswer() {
        val server = startFakeServer()
        try {
            val bridge = AstridBridge.bridge
            // Hermetic by construction: the ONLY remote rung is the fake on
            // loopback — no test traffic ever reaches the internet.
            bridge.setLadder(listOf("fake"))
            bridge.setProvider(
                name = "fake",
                baseUrl = "http://127.0.0.1:${server.address.port}/v1",
                apiKey = "",
                model = "fake-model",
            )

            // The real app path: type into the composer and tap send.
            compose.onNodeWithTag("chat/input").performTextInput("ping the fake rung")
            compose.onNodeWithTag("chat/send").performClick()

            // The turn is async (Rust core + HTTP): wait for the answer.
            compose.waitUntil(timeoutMillis = 20_000) {
                compose.onAllNodesWithText("FAKE-REPLY-OK").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onAllNodesWithText("FAKE-REPLY-OK").fetchSemanticsNodes().isNotEmpty()
        } finally {
            server.stop(0)
        }
    }

    /** Same proof one level down: the bridge alone, no Compose. */
    @Test
    fun bridgeSendMessageHitsTheFakeHttpServer() {
        val server = startFakeServer()
        try {
            val dir = java.io.File(
                System.getProperty("java.io.tmpdir"),
                "astrid-fake-http-${System.nanoTime()}"
            ).apply { mkdirs() }
            val bridge = Bridge.withCore(
                dbPath = dir.resolve("astrid.db").absolutePath,
                dataDir = dir.resolve("core").absolutePath,
            )
            bridge.setLadder(listOf("fake"))
            bridge.setProvider(
                name = "fake",
                baseUrl = "http://127.0.0.1:${server.address.port}/v1",
                apiKey = "",
                model = "fake-model",
            )
            val reply = bridge.sendMessage("ping the fake rung")
            check(reply.reply.contains("FAKE-REPLY-OK")) {
                "expected the fake server's answer, got: ${reply.provider} / ${reply.reply.take(200)}"
            }
            check(reply.provider == "fake") { "the serving rung must be labeled 'fake'" }
            dir.deleteRecursively()
        } finally {
            server.stop(0)
        }
    }
}

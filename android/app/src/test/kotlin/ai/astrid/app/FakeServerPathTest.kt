package ai.astrid.app

import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.astrid_ffi.Bridge

/**
 * THE app-path proof (Phase 4.1): a fake OpenAI-compatible HTTP server on
 * 127.0.0.1 (pure java.net ServerSocket — no JDK-only packages, which are
 * absent from the android.jar compile classpath) stands in for a provider
 * rung; the send goes through the REAL stack — Compose send → bridge
 * sendMessage → Rust core → ladder → reqwest → HTTP → parsed reply →
 * SQLite → bubble. When this test is green, the model path works end to
 * end; the same reqwest/rustls client then talks to the built-in keyless
 * rungs (kilo-auto/free, Pollinations) on a real device.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w412dp-h915dp")
class FakeServerPathTest {

    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    /** One canned completion, served by a raw HTTP/1.1 thread. */
    private class FakeOpenAiServer(private val server: ServerSocket) {
        val port: Int get() = server.localPort

        fun start() {
            Thread {
                while (!server.isClosed) {
                    val socket = try {
                        server.accept()
                    } catch (e: Exception) {
                        return@Thread
                    }
                    Thread { serve(socket) }.apply { isDaemon = true }.start()
                }
            }.apply { isDaemon = true }.start()
        }

        private fun serve(socket: Socket) {
            socket.use { s ->
                // Content-Length counts BYTES, not chars: the core's system
                // prompt carries multi-byte UTF-8 (em dashes, ≤ in tool
                // descriptions), so a char-based read via InputStreamReader
                // blocks forever waiting for chars that never arrive (the
                // request body is complete; the client is awaiting our
                // response). Read the head as raw bytes too — a
                // BufferedReader would prefetch body bytes into its buffer.
                val input = s.getInputStream()
                val head = ArrayList<Byte>()
                fun headEnds(): Boolean {
                    val n = head.size
                    return n >= 4 && head[n - 4] == 13.toByte() && head[n - 3] == 10.toByte() &&
                        head[n - 2] == 13.toByte() && head[n - 1] == 10.toByte()
                }
                while (!headEnds()) {
                    val b = input.read()
                    if (b < 0) return
                    head.add(b.toByte())
                }
                var contentLength = 0
                for (line in String(head.toByteArray(), Charsets.ISO_8859_1).split("\r\n").drop(1)) {
                    if (line.startsWith("Content-Length:", ignoreCase = true)) {
                        contentLength = line.substringAfter(':').trim().toInt()
                    }
                }

                val out = s.getOutputStream()
                var requestBody = ""
                if (contentLength > 0) {
                    val buf = ByteArray(contentLength)
                    var read = 0
                    while (read < contentLength) {
                        val n = input.read(buf, read, contentLength - read)
                        if (n < 0) break
                        read += n
                    }
                    requestBody = String(buf, 0, read, Charsets.UTF_8)

                    if (requestBody.contains("\"stream\":true")) {
                        // The core's agent turn streams (SSE). Answer the way
                        // OpenAI-compatible servers do: content delta, finish
                        // reason, then [DONE].
                        val sse = buildString {
                            append(
                                "data: {\"choices\":[{\"delta\":{\"content\":\"FAKE-REPLY-OK\"}," +
                                    "\"finish_reason\":null}]}\n\n"
                            )
                            append("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n")
                            append("data: [DONE]\n\n")
                        }.toByteArray()
                        val head = "HTTP/1.1 200 OK\r\n" +
                            "Content-Type: text/event-stream\r\n" +
                            "Connection: close\r\n\r\n"
                        out.write(head.toByteArray())
                        out.write(sse)
                        out.flush()
                        return
                    }

                    val body = """
                        {"id":"fake-1","object":"chat.completion","created":1,"model":"fake-model",
                         "choices":[{"index":0,"finish_reason":"stop",
                           "message":{"role":"assistant","content":"FAKE-REPLY-OK"}}],
                         "usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}
                    """.trimIndent().toByteArray()
                    val head = "HTTP/1.1 200 OK\r\n" +
                        "Content-Type: application/json\r\n" +
                        "Content-Length: ${body.size}\r\n" +
                        "Connection: close\r\n\r\n"
                    out.write(head.toByteArray())
                    out.write(body)
                    out.flush()
                }
            }
        }

        fun stop() {
            server.close()
        }
    }

    private fun startFakeServer(): FakeOpenAiServer {
        val server = FakeOpenAiServer(ServerSocket(0, 50, InetAddress.getByName("127.0.0.1")))
        server.start()
        return server
    }

    private fun configureFakeRung(bridge: Bridge, server: FakeOpenAiServer) {
        // Hermetic by construction: the ONLY remote rung is the fake on
        // loopback — no test traffic ever reaches the internet.
        bridge.setLadder(listOf("fake"))
        bridge.setProvider(
            name = "fake",
            baseUrl = "http://127.0.0.1:${server.port}/v1",
            apiKey = "",
            model = "fake-model",
        )
    }

    @Test
    fun aSendRunsTheRealHttpPathAndTheBubbleShowsTheAnswer() {
        val server = startFakeServer()
        try {
            configureFakeRung(AstridBridge.bridge, server)

            // The real app path: type into the composer and tap send.
            compose.onNodeWithTag("chat/input").performTextInput("ping the fake rung")
            compose.onNodeWithTag("chat/send").performClick()

            // The turn is async (Rust core + HTTP): wait for the answer.
            compose.waitUntil(timeoutMillis = 20_000) {
                compose.onAllNodesWithText("FAKE-REPLY-OK").fetchSemanticsNodes().isNotEmpty()
            }
            check(compose.onAllNodesWithText("FAKE-REPLY-OK").fetchSemanticsNodes().isNotEmpty())
        } finally {
            server.stop()
        }
    }

    /** Same proof one level down: the bridge alone, no Compose. */
    @Test
    fun bridgeSendMessageHitsTheFakeHttpServer() {
        val server = startFakeServer()
        val dir = java.io.File(
            System.getProperty("java.io.tmpdir"),
            "astrid-fake-http-${System.nanoTime()}"
        ).apply { mkdirs() }
        try {
            val bridge = Bridge.withCore(
                dbPath = dir.resolve("astrid.db").absolutePath,
                dataDir = dir.resolve("core").absolutePath,
            )
            configureFakeRung(bridge, server)
            val reply = bridge.sendMessage("ping the fake rung")
            check(reply.reply.contains("FAKE-REPLY-OK")) {
                "expected the fake server's answer, got provider=${reply.provider}, " +
                    "fallback=${reply.fallbackReason ?: "none"}, reply=${reply.reply.take(200)}"
            }
            check(reply.provider == "fake") { "the serving rung must be labeled 'fake'" }
        } finally {
            server.stop()
            dir.deleteRecursively()
        }
    }
}

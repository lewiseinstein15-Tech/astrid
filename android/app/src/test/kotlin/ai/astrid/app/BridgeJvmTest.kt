package ai.astrid.app

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import uniffi.astrid_ffi.Bridge

/**
 * Bridge smoke tests on the desktop JVM: the SAME generated Kotlin bindings
 * the APK ships, driven against a host build of libastrid_ffi.so through
 * JNA. Includes a real chat turn with an attachment — computed by the core.
 */
class BridgeJvmTest {
    /**
     * The version gradle parsed from the VERSION file is handed to tests as
     * the astrid.version system property; the Rust core must report the same
     * value, or the single-source chain (VERSION file → gradle → build.rs)
     * is broken.
     */
    private val version: String =
        requireNotNull(System.getProperty("astrid.version")) { "astrid.version property not set by gradle" }

    @Test
    fun coreAnswersThroughGeneratedBindings() {
        val bridge = Bridge()
        val info = bridge.appInfo()
        assertEquals("Astrid", info.name)
        assertEquals(version, info.version, "core version must equal the VERSION file")
        assertTrue(info.tagline.isNotBlank())
        assertTrue(
            bridge.greeting().contains(info.version),
            "greeting must carry the core version"
        )
    }

    @Test
    fun uptimeIncreasesAcrossCalls() {
        val bridge = Bridge()
        val before = bridge.uptimeMs()
        Thread.sleep(25)
        assertTrue(bridge.uptimeMs() >= before, "uptime must be monotonic")
    }

    @Test
    fun aChatTurnThroughTheFullCoreProducesAReply() {
        val dir = File(System.getProperty("java.io.tmpdir"), "astrid-jvm-chat-${System.nanoTime()}")
        dir.mkdirs()
        try {
            val bridge = Bridge.withCore(
                dbPath = dir.resolve("astrid.db").absolutePath,
                dataDir = dir.resolve("core").absolutePath,
            )
            // Hermetic: the built-in keyless rungs are real network endpoints;
            // a JVM unit test must answer from the local kernel, never the net.
            bridge.setLadder(listOf("offline"))
            val reply = bridge.sendMessage("hi")
            assertTrue(
                reply.reply.contains("Astrid"),
                "the kernel gate must answer small talk from the core, got: ${reply.reply}"
            )
            assertEquals(
                "local (no model)",
                reply.provider,
                "small talk is answered by the kernel gate, never by a provider"
            )

            val messages = bridge.recentMessages(10u)
            assertEquals(2, messages.size, "user + assistant persisted")
            assertEquals("user", messages[0].role)
            assertEquals("hi", messages[0].text)
            assertEquals("assistant", messages[1].role)
            assertTrue(messages[1].text == reply.reply)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun sendMessageWithFilesIngestsLinksAndDedupes() {
        val dir = File(System.getProperty("java.io.tmpdir"), "astrid-jvm-files-${System.nanoTime()}")
        dir.mkdirs()
        try {
            val bridge = Bridge.withCore(
                dbPath = dir.resolve("astrid.db").absolutePath,
                dataDir = dir.resolve("core").absolutePath,
            )
            // Hermetic: "see attached" is a Task — keep the ladder offline.
            bridge.setLadder(listOf("offline"))
            val src = dir.resolve("brief.md")
            src.writeText("attachment body\n")
            val path = src.absolutePath
            bridge.sendMessageWithFiles("see attached", listOf(path, path))

            // same sha256 -> ONE upload row, ONE file on the bubble
            assertEquals(1, bridge.uploads().size)
            val messages = bridge.recentMessages(10u)
            assertEquals(2, messages.size)
            assertEquals(listOf("brief.md"), messages[0].files)
            assertTrue(messages[1].files.isEmpty())
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun uploadsIngestThroughTheCoreAndPersist() {
        val dir = File(System.getProperty("java.io.tmpdir"), "astrid-jvm-upload-${System.nanoTime()}")
        dir.mkdirs()
        try {
            val bridge = Bridge.withCore(
                dbPath = dir.resolve("astrid.db").absolutePath,
                dataDir = dir.resolve("core").absolutePath,
            )
            val src = dir.resolve("note.md")
            src.writeText("# real file\nfrom the JVM test\n")
            val rec = bridge.ingestUpload("note.md", src.absolutePath)
            assertTrue(rec.bytes > 0uL, "ingested file must have bytes")
            assertEquals(64, rec.sha256.length, "sha256 recorded")
            val listed = bridge.uploads()
            assertTrue(listed.any { it.name == "note.md" && it.sha256 == rec.sha256 })
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun eventLogPersistsAcrossBridgeReopen() {
        val dir = File(System.getProperty("java.io.tmpdir"), "astrid-jvm-test-${System.nanoTime()}")
        dir.mkdirs()
        try {
            val db = File(dir, "astrid.db").absolutePath
            val first = Bridge.withStore(db)
            val written = first.appendEvent("test.kind", "hello from jvm")
            assertTrue(written.id > 0uL, "append must return the stored row id")

            val reopened = Bridge.withStore(db)
            val events = reopened.recentEvents(10u)
            assertTrue(
                events.any { it.kind == "test.kind" && it.text == "hello from jvm" },
                "row written before reopen must still be there"
            )
            assertTrue(
                events.count { it.kind == "core.boot" } >= 2,
                "every bridge open logs a core.boot event"
            )
        } finally {
            dir.deleteRecursively()
        }
    }
}

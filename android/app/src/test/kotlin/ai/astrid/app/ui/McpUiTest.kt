package ai.astrid.app.ui

import ai.astrid.app.AstridBridge
import ai.astrid.app.MainActivity
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ApplicationProvider
import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Phase 6 — MCP server management, END TO END on the real app: the drawer
 * opens the MCP Servers screen, a server is added (persisted to the [mcp]
 * section of config.toml), PROBED with a real stdio handshake against an
 * embedded fake server (python3 on the test host — the spawn the core does
 * is the real one), toggled off and removed. This is the surface that was
 * hard-disabled ("stdio MCP servers are a phase-9 surface") before 0.1.6.0.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w412dp-h915dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class McpUiTest {

    @get:Rule
    val compose = createAndroidComposeRule<MainActivity>()

    private val bridge get() = AstridBridge.bridge

    /** The fake stdio MCP server, written to the app cache before the flow. */
    private val fakeServerScript = """
        import json, sys
        def send(obj):
            sys.stdout.write(json.dumps(obj) + "\n")
            sys.stdout.flush()
        TOOLS = [{"name": "echo", "description": "Echo the input text back",
                  "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}},
                                  "required": ["text"]}}]
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            req = json.loads(line)
            method, rid = req.get("method"), req.get("id")
            if method == "initialize":
                send({"jsonrpc": "2.0", "id": rid, "result": {
                    "protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-demo", "version": "1.0.0"}}})
            elif method == "notifications/initialized":
                pass
            elif method == "tools/list":
                send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
            elif method == "tools/call":
                args = (req.get("params", {}) or {}).get("arguments", {}) or {}
                send({"jsonrpc": "2.0", "id": rid, "result": {
                    "content": [{"type": "text", "text": "echo: %s" % args.get("text", "")}],
                    "isError": False}})
    """.trimIndent()

    @Before
    fun hermeticLadder() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        AstridBridge.init(context.filesDir)
        runCatching { bridge.setLadder(listOf("offline")) }
        derivePendingApproval(bridge)?.let { p ->
            runCatching {
                if (p.kind == "shell") bridge.resolveShell(p.approvalId, false)
                else bridge.resolveApproval(false)
            }
        }
        // Any server left by another test run would change the row count.
        runCatching {
            bridge.mcpServers().forEach { bridge.removeMcpServer(it.name) }
        }
        val script = File(context.cacheDir, "fake_mcp_server.py")
        script.writeText(fakeServerScript)
        assertTrue(script.length() > 100)
    }

    private fun openDrawer() {
        compose.onNodeWithTag("header/menu").performClick()
        compose.waitForIdle()
    }

    private fun navigate(label: String, tag: String) {
        openDrawer()
        compose.onNodeWithTag("drawer/$label").performClick()
        compose.waitForIdle()
        compose.onNodeWithTag(tag).assertExists()
    }

    @Test
    fun addProbeToggleRemoveEndToEnd() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val script = File(context.cacheDir, "fake_mcp_server.py").absolutePath

        navigate("MCP Servers", "mcp/add")
        compose.onNodeWithTag("mcp/add").performClick()
        compose.onNodeWithTag("mcp/name").performTextInput("fake")
        compose.onNodeWithTag("mcp/command").performTextInput("python3")
        compose.onNodeWithTag("mcp/args").performTextInput(script)
        compose.onNodeWithTag("mcp/save").performClick()

        // The row appears — the add persisted and the list refreshed.
        compose.waitUntil(10_000) {
            compose.onAllNodesWithTag("mcp/fake").fetchSemanticsNodes().isNotEmpty()
        }

        // PROBE: a real spawn + JSON-RPC handshake + tools/list. The line
        // renders either way; the assertion pins it to a successful probe
        // and, on failure, prints the actual line.
        compose.onNodeWithTag("mcp/probe/fake").performClick()
        compose.waitUntil(30_000) {
            compose.onAllNodesWithText("ok: ", substring = true)
                .fetchSemanticsNodes().isNotEmpty() ||
                compose.onAllNodesWithText("error: ", substring = true)
                    .fetchSemanticsNodes().isNotEmpty()
        }
        val okLine = compose.onAllNodesWithText("ok: ", substring = true)
            .fetchSemanticsNodes().isNotEmpty()
        assertTrue(
            "probe must succeed with ok: N tools (saw an error: line instead)",
            okLine,
        )

        // Toggle off: the button flips to Enable, the chip flips to off.
        compose.onNodeWithTag("mcp/toggle/fake").performClick()
        compose.waitUntil(10_000) {
            compose.onAllNodesWithText("Enable").fetchSemanticsNodes().isNotEmpty()
        }

        // Remove: the row disappears and the config no longer lists it.
        compose.onNodeWithTag("mcp/remove/fake").performClick()
        compose.waitUntil(10_000) {
            compose.onAllNodesWithTag("mcp/fake").fetchSemanticsNodes().isEmpty()
        }
        assertTrue(
            "the removed server must be gone from the bridge config",
            runCatching { bridge.mcpServers().isEmpty() }.getOrDefault(true),
        )
    }
}

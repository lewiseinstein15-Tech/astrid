package ai.astrid.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import uniffi.astrid_ffi.Bridge

/**
 * The list screens, all REAL — every row comes from the bridge:
 * Live trace renders the core's audit/receipt stream (this is where the
 * Events card lives — it moved here from Chat), Agents the compiled-in
 * roster, Sessions the persisted chat sessions, Memory the five-verb
 * markdown store, Skills the bundled catalog, Knowledge the ingested uploads
 * and photos, Plugins the manifest scan, MCP the session's connection report.
 */

private fun timeLabel(ms: Long): String =
    SimpleDateFormat("MMM d, HH:mm", Locale.US).format(Date(ms))

/** 06 — Live trace: the core's real receipt stream, newest first. */
@Composable
fun LiveTraceScreen(bridge: Bridge, onMenu: () -> Unit) {
    var events by remember { mutableStateOf(runCatching { bridge.recentEvents(100u) }.getOrDefault(emptyList())) }
    ScreenScaffold("Live trace", AccentAmber, onMenu) {
        SectionLabel("RECEIPT STREAM · RUST AUDIT LOG", AccentAmber)
        if (events.isEmpty()) {
            Card { KitText("No events yet — send a chat message to see real receipts.", TextMuted, 13, FontWeight.Medium) }
        }
        events.forEach { event ->
            Card {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Dot(AccentAmber)
                    Spacer(Modifier.width(8.dp))
                    KitText(event.kind, AccentAmber, 12, FontWeight.Bold)
                    Spacer(Modifier.weight(1f))
                    KitText(timeLabel((event.createdAt * 1000u).toLong()), TextMuted, 11, FontWeight.Medium)
                }
                KitText(event.text, TextMuted, 12, FontWeight.Medium)
            }
        }
    }
}

/** 07 — Agents: the compiled-in curated roster. */
@Composable
fun AgentsScreen(bridge: Bridge, onMenu: () -> Unit) {
    val agents = remember { runCatching { bridge.agents() }.getOrDefault(emptyList()) }
    ScreenScaffold("Agents", AccentCoral, onMenu) {
        SectionLabel("STAFF · ${agents.size} AGENTS FROM THE ROSTER", AccentCoral)
        if (agents.isEmpty()) {
            Card { KitText("Roster unavailable (core not attached).", TextMuted, 13, FontWeight.Medium) }
        }
        agents.forEach { agent ->
            ListRow(
                letter = agent.name.firstOrNull()?.uppercase() ?: "?",
                accent = AccentCoral,
                title = agent.name,
                subtitle = "${agent.role} · division ${agent.division}",
                tag = "agent/${agent.id}"
            )
            Card {
                KitText("Capabilities", TextPrimary, 12, FontWeight.Bold)
                KitText(agent.capabilities.joinToString(", "), TextMuted, 12, FontWeight.Medium)
            }
        }
    }
}

/** 08 — Sessions: the persisted chats. Tap one to reopen it. */
@Composable
fun SessionsScreen(bridge: Bridge, onMenu: () -> Unit, onOpenSession: (Long) -> Unit) {
    val sessions = remember { runCatching { bridge.chatSessions() }.getOrDefault(emptyList()) }
    ScreenScaffold("Sessions", AccentBlue, onMenu) {
        SectionLabel("CHATS · PERSISTED IN SQLITE", AccentBlue)
        if (sessions.isEmpty()) {
            Card { KitText("No chats yet — send a message and one appears here.", TextMuted, 13, FontWeight.Medium) }
        }
        sessions.forEach { session ->
            ListRow(
                letter = session.title.firstOrNull()?.uppercase() ?: "·",
                accent = AccentBlue,
                title = session.title,
                subtitle = "${session.messageCount} messages · ${timeLabel((session.updatedAt * 1000u).toLong())}",
                tag = "session/${session.id}",
                onClick = { onOpenSession(session.id) }
            )
        }
    }
}

/** 09 — Memory: the five-verb markdown store, read and written through the core. */
@Composable
fun MemoryScreen(bridge: Bridge, onMenu: () -> Unit) {
    var pages by remember { mutableStateOf(runCatching { bridge.memories() }.getOrDefault(emptyList())) }
    var adding by remember { mutableStateOf(false) }
    var topic by remember { mutableStateOf("") }
    var body by remember { mutableStateOf("") }

    ScreenScaffold("Memory", AccentPurple, onMenu) {
        SectionLabel("PAGES · ${pages.size} · MARKDOWN STORE, SHA-256 MANIFEST", AccentPurple)
        if (pages.isEmpty()) {
            Card { KitText("No memory yet — add a fact and it is stored by the core.", TextMuted, 13, FontWeight.Medium) }
        }
        pages.forEach { page ->
            ListRow(
                letter = page.title.firstOrNull()?.uppercase() ?: "·",
                accent = AccentPurple,
                title = page.title,
                subtitle = "${page.kind} · ${page.updatedAt}",
                chipLabel = if (page.forgotten) "forgotten" else null,
                tag = "memory/${page.id}"
            )
            OutlineButton("Forget", AccentPurple, tag = "memory/forget/${page.id}") {
                runCatching { bridge.forgetMemory(page.id) }
                pages = runCatching { bridge.memories() }.getOrDefault(pages)
            }
        }
        if (adding) {
            Card {
                FormField("Topic", topic, tag = "memory/topic") { topic = it }
                FormField("Body", body, tag = "memory/body") { body = it }
                FilledButton("Save to memory", AccentPurple, tag = "memory/save") {
                    if (topic.isNotBlank() && body.isNotBlank()) {
                        runCatching { bridge.remember(topic.trim(), body.trim()) }
                        topic = ""
                        body = ""
                        adding = false
                        pages = runCatching { bridge.memories() }.getOrDefault(pages)
                    }
                }
            }
        } else {
            FilledButton("Add memory", AccentPurple, tag = "memory/add") { adding = true }
        }
    }
}

/** 10 — Skills: the bundled catalog with progressive disclosure metadata. */
@Composable
fun SkillsScreen(bridge: Bridge, onMenu: () -> Unit) {
    val skills = remember { runCatching { bridge.skills() }.getOrDefault(emptyList()) }
    ScreenScaffold("Skills", AccentGreen, onMenu) {
        SectionLabel("BUNDLED · ${skills.size} SKILLS FROM THE CATALOG", AccentGreen)
        if (skills.isEmpty()) {
            Card { KitText("Catalog unavailable (core not attached).", TextMuted, 13, FontWeight.Medium) }
        }
        skills.forEach { skill ->
            ListRow(
                letter = skill.name.firstOrNull()?.uppercase() ?: "?",
                accent = AccentGreen,
                title = skill.name,
                subtitle = skill.description,
                tag = "skill/${skill.name}"
            )
            skill.whenToUse?.let { whenToUse ->
                Card { KitText("When to use: $whenToUse", TextMuted, 12, FontWeight.Medium) }
            }
        }
    }
}

/** 11 — Knowledge: what actually came in through Upload and Camera. */
@Composable
fun KnowledgeScreen(bridge: Bridge, onMenu: () -> Unit) {
    val uploads = remember { runCatching { bridge.uploads() }.getOrDefault(emptyList()) }
    val photos = remember { runCatching { bridge.photos() }.getOrDefault(emptyList()) }
    ScreenScaffold("Knowledge", AccentTeal, onMenu) {
        SectionLabel("UPLOADS · ${uploads.size}", AccentTeal)
        if (uploads.isEmpty()) {
            Card { KitText("No files yet — use + → Upload file in the chat.", TextMuted, 13, FontWeight.Medium) }
        }
        uploads.forEach { upload ->
            ListRow(
                letter = upload.name.firstOrNull()?.uppercase() ?: "?",
                accent = AccentTeal,
                title = upload.name,
                subtitle = "${upload.bytes} bytes · sha256 ${upload.sha256.take(12)}…",
                tag = "knowledge/upload/${upload.id}"
            )
        }
        SectionLabel("PHOTOS · ${photos.size}", AccentTeal)
        if (photos.isEmpty()) {
            Card { KitText("No photos yet — use + → Take photo in the chat.", TextMuted, 13, FontWeight.Medium) }
        }
        photos.forEach { photo ->
            ListRow(
                letter = "P",
                accent = AccentTeal,
                title = photo.path.substringAfterLast('/'),
                subtitle = "${photo.bytes} bytes · ${timeLabel((photo.createdAt * 1000u).toLong())}",
                tag = "knowledge/photo/${photo.id}"
            )
        }
    }
}

/** 13 — Plugins: the manifest scan (real pipeline, honest empty state). */
@Composable
fun PluginsScreen(bridge: Bridge, onMenu: () -> Unit) {
    val plugins = remember { runCatching { bridge.plugins() }.getOrDefault(emptyList()) }
    ScreenScaffold("Plugins", AccentYellow, onMenu) {
        SectionLabel("MANIFEST SCAN · plugins/*/plugin.toml", AccentYellow)
        if (plugins.isEmpty()) {
            Card {
                KitText("No plugins installed.", TextPrimary, 13, FontWeight.Bold)
                KitText(
                    "The scanner runs against the app's plugins directory at every core start. " +
                        "An installer UI lands with the release polish phase.",
                    TextMuted, 12, FontWeight.Medium
                )
            }
        }
        plugins.forEach { plugin ->
            ListRow(
                letter = plugin.name.firstOrNull()?.uppercase() ?: "?",
                accent = AccentYellow,
                title = plugin.name,
                subtitle = plugin.problem ?: "Loaded as a first-class tool",
                chipLabel = if (plugin.problem != null) "broken" else "On",
                chipAccent = if (plugin.problem != null) Danger else AccentYellow
            )
        }
    }
}

/** 14 — MCP Servers: configure stdio servers, probe them, manage the list.
 *
 * The list lives in the [mcp] section of the app's config.toml (phase 6):
 * add / toggle / remove are quick config writes; the PROBE spawns the server,
 * handshakes, and counts its tools — it owns a process, so it runs on
 * Dispatchers.IO, never the UI thread (the 0.1.5 ANR lesson).
 */
@Composable
fun McpServersScreen(bridge: Bridge, onMenu: () -> Unit) {
    var servers by remember { mutableStateOf(runCatching { bridge.mcpServers() }.getOrDefault(emptyList())) }
    var report by remember { mutableStateOf(runCatching { bridge.mcpReport() }.getOrDefault(emptyList())) }
    var adding by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var command by remember { mutableStateOf("") }
    var args by remember { mutableStateOf("") }
    var risk by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    /** One probe result line per server name, shown under its row. */
    var probeLines by remember { mutableStateOf(emptyMap<String, String>()) }
    val scope = rememberCoroutineScope()

    fun refresh() {
        servers = runCatching { bridge.mcpServers() }.getOrDefault(servers)
        report = runCatching { bridge.mcpReport() }.getOrDefault(report)
    }

    ScreenScaffold("MCP Servers", AccentIndigo, onMenu) {
        SectionLabel("CONFIGURED SERVERS · ${servers.size}", AccentIndigo)
        if (servers.isEmpty()) {
            Card {
                KitText("No MCP servers yet.", TextPrimary, 13, FontWeight.Bold)
                KitText(
                    "Add one below. At the next app start its process spawns, its " +
                        "tools join the registry (mcp_<server>_<tool>), and every call " +
                        "is approval-gated like the built-in tools. Probe connects " +
                        "right now and counts the tools without attaching anything.",
                    TextMuted, 12, FontWeight.Medium
                )
            }
        }
        servers.forEach { server ->
            val subtitle = buildString {
                append(server.command)
                server.args.forEach { append(" ").append(it) }
                server.defaultRisk?.let { append("  ·  risk: ").append(it) }
            }
            ListRow(
                letter = server.name.firstOrNull()?.uppercase() ?: "·",
                accent = AccentIndigo,
                title = server.name,
                subtitle = subtitle,
                chipLabel = if (server.enabled) "on" else "off",
                chipAccent = if (server.enabled) AccentGreen else AccentSlate,
                tag = "mcp/${server.name}"
            )
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlineButton("Probe", AccentIndigo, tag = "mcp/probe/${server.name}") {
                    scope.launch(Dispatchers.IO) {
                        val line = runCatching { bridge.mcpProbe(server.name) }
                            .getOrElse { "error: ${it.message}" }
                        probeLines = probeLines + (server.name to line)
                    }
                }
                OutlineButton(if (server.enabled) "Disable" else "Enable", AccentSlate, tag = "mcp/toggle/${server.name}") {
                    runCatching { bridge.setMcpServerEnabled(server.name, !server.enabled) }
                        .onFailure { error = it.message }
                    refresh()
                }
                OutlineButton("Remove", Danger, tag = "mcp/remove/${server.name}") {
                    runCatching { bridge.removeMcpServer(server.name) }
                        .onFailure { error = it.message }
                    refresh()
                }
            }
            probeLines[server.name]?.let { line ->
                Card { KitText(line, TextMuted, 12, FontWeight.Medium) }
            }
        }
        if (adding) {
            Card {
                FormField("Name (a-z 0-9 - _)", name, tag = "mcp/name") { name = it }
                FormField("Command (executable path)", command, tag = "mcp/command") { command = it }
                FormField("Arguments (space-separated)", args, tag = "mcp/args") { args = it }
                FormField("Risk class — optional: safe|medium|high|external", risk, tag = "mcp/risk") { risk = it }
                FilledButton("Save server", AccentIndigo, tag = "mcp/save") {
                    val parsed = if (args.isBlank()) emptyList() else args.trim().split(Regex("\\s+"))
                    runCatching {
                        bridge.addMcpServer(
                            name.trim(), command.trim(), parsed,
                            risk.trim().takeIf { it.isNotEmpty() },
                        )
                    }
                        .onSuccess {
                            name = ""; command = ""; args = ""; risk = ""
                            adding = false; error = null
                            refresh()
                        }
                        .onFailure { error = it.message }
                }
            }
        } else {
            FilledButton("Add server", AccentIndigo, tag = "mcp/add") { adding = true }
        }
        error?.let { msg ->
            Card { KitText(msg, Danger, 12, FontWeight.Medium) }
        }

        SectionLabel("CONNECTION REPORT · SESSION START", AccentIndigo)
        if (report.isEmpty()) {
            Card {
                KitText(
                    "Nothing connected at session start — the report fills in once a " +
                        "configured server spawns.",
                    TextMuted, 12, FontWeight.Medium
                )
            }
        }
        report.forEach { line ->
            Card { KitText(line, TextMuted, 12, FontWeight.Medium) }
        }
    }
}

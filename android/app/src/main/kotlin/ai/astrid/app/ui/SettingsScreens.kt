package ai.astrid.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import android.content.pm.PackageManager
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.astrid_ffi.Bridge
import uniffi.astrid_ffi.LadderRungRecord
import uniffi.astrid_ffi.NetSelfTestCheck
import uniffi.astrid_ffi.NetSelfTestReport

/**
 * The settings cluster. Settings reads the live version and permissions
 * profile from the core; Providers (was Custom provider) persists the rung
 * and the ladder is rebuilt; Approval rules are stored CRUD (enforcement
 * lands in phase 9); About shows only what is true — no fake update rows.
 */

/** 16 — Settings: GENERAL (approvals, about, profile) + PROVIDERS groups. */
@Composable
fun SettingsScreen(bridge: Bridge, onMenu: () -> Unit, onOpen: (Screen) -> Unit) {
    val version = remember { bridge.appInfo().version }
    val profile = remember { runCatching { bridge.permissionsProfile() }.getOrDefault("unknown") }
    val provider = remember { runCatching { bridge.provider() }.getOrNull() }

    ScreenScaffold("Settings", AccentSlate, onMenu) {
        SectionLabel("GENERAL", AccentSlate)
        ListRow("A", AccentSlate, "Approvals", "Rules stored; enforced from phase 9", tag = "settings/approvals") {
            onOpen(Screen.ApprovalRules)
        }
        ListRow("A", AccentSlate, "About and updates", "Version and build info", chipLabel = "v$version", tag = "settings/about") {
            onOpen(Screen.About)
        }
        ListRow(
            "P", AccentSlate, "Permission profile",
            "Safe tools run; risky calls are denied headlessly",
            chipLabel = profile
        )
        SectionLabel("PROVIDERS", AccentSlate)
        val configured = provider != null && provider.baseUrl.isNotEmpty()
        ListRow(
            letter = if (configured) (provider?.name?.firstOrNull()?.uppercase() ?: "P") else "O",
            accent = AccentSlate,
            title = if (configured) provider!!.name else "built-in free rungs",
            subtitle = if (configured) {
                "${provider!!.baseUrl} · ${provider!!.model.ifEmpty { "default model" }}"
            } else {
                // Honest: two keyless built-ins answer first, offline last —
                // the real order lives in Settings > Providers.
                "kilo-auto/free and Pollinations answer first; offline last"
            },
            chipLabel = if (configured) "Configured" else "2 built-in",
            tag = "settings/provider",
            onClick = { onOpen(Screen.Providers) }
        )
        FilledButton("Add custom provider", AccentSlate, tag = "settings/add-provider") {
            onOpen(Screen.Providers)
        }
    }
}

/** 17 — Providers: the form persists into the ladder (real rebuild). The
 * ladder order is editable and every rung carries the network self-test
 * (INTERNET permission, DNS, HTTPS handshake, a real chat request) with a
 * Copy report button — raw errors, never swallowed. */
@Composable
fun ProvidersScreen(bridge: Bridge, onMenu: () -> Unit) {
    val saved = remember { runCatching { bridge.provider() }.getOrNull() }
    var name by remember { mutableStateOf(saved?.name ?: "") }
    var baseUrl by remember { mutableStateOf(saved?.baseUrl ?: "") }
    var apiKey by remember { mutableStateOf("") }
    var modelId by remember { mutableStateOf(saved?.model ?: "") }
    var result by remember { mutableStateOf<String?>(null) }

    // Ladder order + network self-test state.
    var rungs by remember {
        mutableStateOf(runCatching { bridge.ladderRungs() }.getOrDefault(emptyList()))
    }
    var reports by remember { mutableStateOf(mapOf<String, NetSelfTestReport>()) }
    var testing by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val hasInternet = remember {
        ContextCompat.checkSelfPermission(context, "android.permission.INTERNET") ==
            PackageManager.PERMISSION_GRANTED
    }

    fun testRung(id: String) {
        if (testing != null) return
        scope.launch {
            testing = id
            reports = reports + (id to runCatching {
                withContext(Dispatchers.IO) { bridge.netSelftest(id) }
            }.getOrElse { e ->
                NetSelfTestReport(
                    id,
                    listOf(NetSelfTestCheck("error", false, "${e::class.simpleName}: ${e.message}", 0uL))
                )
            })
            testing = null
        }
    }

    fun moveRung(id: String, delta: Int) {
        val ordered = rungs.filter { it.id != "offline" }
        val idx = ordered.indexOfFirst { it.id == id }
        if (idx == -1) return
        val newIdx = idx + delta
        if (newIdx < 0 || newIdx >= ordered.size) return
        val mutable = ordered.toMutableList()
        val item = mutable.removeAt(idx)
        mutable.add(newIdx, item)
        runCatching { bridge.setLadder(mutable.map { it.id }) }
            .onSuccess { rungs = it }
            .onFailure { e -> result = "Reorder failed: ${e.message}" }
    }

    fun reportText(): String = buildString {
        appendLine(
            "Astrid network self-test — v${ai.astrid.app.BuildConfig.VERSION_NAME} " +
                "(code ${ai.astrid.app.BuildConfig.VERSION_CODE})"
        )
        appendLine("INTERNET permission: ${if (hasInternet) "granted" else "DENIED"}")
        rungs.forEach { rung ->
            appendLine("— rung ${rung.id} —")
            val rep = reports[rung.id]
            if (rep == null) appendLine("(not tested)") else rep.checks.forEach { c ->
                appendLine("${c.name}: ${if (c.ok) "ok" else "ERROR"} in ${c.ms} ms — ${c.detail}")
            }
        }
    }

    ScreenScaffold("Providers", AccentSlate, onMenu) {
        SectionLabel("YOUR OWN OPENAI-COMPATIBLE RUNG", AccentSlate)
        FormField("Provider name", name, tag = "provider/name") { name = it }
        FormField("Base URL", baseUrl, tag = "provider/url") { baseUrl = it }
        FormField("API key (stored app-private, injected as an env var)", apiKey, tag = "provider/key") { apiKey = it }
        FormField("Model ID", modelId, tag = "provider/model") { modelId = it }
        if (saved != null && saved.baseUrl.isNotEmpty()) {
            Card {
                KitText("Saved: ${saved.name} → ${saved.baseUrl}", TextMuted, 12, FontWeight.Medium)
                KitText(
                    if (saved.hasKey) "API key stored (app-private)" else "No API key stored",
                    TextMuted, 12, FontWeight.Medium
                )
            }
        }
        FilledButton("Save provider", AccentSlate, tag = "provider/save") {
            runCatching {
                bridge.setProvider(
                    name = name.trim(),
                    baseUrl = baseUrl.trim(),
                    apiKey = apiKey.trim(),
                    model = modelId.trim(),
                )
            }.onSuccess {
                result = "Saved — your rung is on the ladder (real order below); " +
                    "the built-in free rungs answer first unless you reorder."
                runCatching { bridge.appendEvent("provider.saved", name.trim()) }
                rungs = runCatching { bridge.ladderRungs() }.getOrDefault(rungs)
            }.onFailure { e ->
                result = "Save failed: ${e.message}"
            }
        }
        result?.let {
            Card { KitText(it, if (it.startsWith("Save failed")) Danger else AccentGreen, 13, FontWeight.Bold) }
        }

        SectionLabel("LADDER ORDER · TRY TOP TO BOTTOM · OFFLINE LAST", AccentSlate)
        rungs.forEach { rung ->
            ListRow(
                letter = if (rung.builtin) "B" else rung.id.firstOrNull()?.uppercase() ?: "R",
                accent = if (rung.id == "offline") TextMuted else AccentGreen,
                title = rung.id,
                subtitle = rung.desc,
                tag = "rung/${rung.id}"
            )
            if (rung.id != "offline") {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    OutlineButton("Up", AccentSlate, tag = "rung/up/${rung.id}") { moveRung(rung.id, -1) }
                    OutlineButton("Down", AccentSlate, tag = "rung/down/${rung.id}") { moveRung(rung.id, +1) }
                    OutlineButton(
                        if (testing == rung.id) "Testing…" else "Test",
                        AccentOrange,
                        tag = "rung/test/${rung.id}"
                    ) { testRung(rung.id) }
                }
            }
            reports[rung.id]?.checks?.forEach { check ->
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                    verticalAlignment = Alignment.CenterVertically
                ) {
                    Dot(if (check.ok) AccentGreen else Danger)
                    Spacer(Modifier.size(8.dp))
                    KitText(
                        "${check.name} (${check.ms} ms): ${check.detail}",
                        if (check.ok) AccentGreen else Danger,
                        11,
                        FontWeight.Medium
                    )
                }
            }
        }
        FilledButton("Test every rung", AccentOrange, tag = "nettest/run-all") {
            rungs.forEach { testRung(it.id) }
        }
        Card {
            KitText(
                "INTERNET permission: ${if (hasInternet) "granted" else "DENIED — no network call can succeed"}",
                if (hasInternet) AccentGreen else Danger,
                13,
                FontWeight.Bold
            )
        }
        FilledButton("Copy report", AccentSlate, tag = "nettest/copy") {
            clipboard.setText(AnnotatedString(reportText()))
        }
    }
}

/** 18 — Approval rules: real CRUD; the core stores, phase 9 enforces. */
@Composable
fun ApprovalRulesScreen(bridge: Bridge, onMenu: () -> Unit) {
    var rules by remember { mutableStateOf(runCatching { bridge.rules() }.getOrDefault(emptyList())) }
    var adding by remember { mutableStateOf(false) }
    var pattern by remember { mutableStateOf("") }
    var decision by remember { mutableStateOf("deny") }

    ScreenScaffold("Approvals", AccentSlate, onMenu) {
        SectionLabel("RULES · ${rules.size} · STORED NOW, ENFORCED FROM PHASE 9", AccentSlate)
        if (rules.isEmpty()) {
            Card { KitText("No rules yet.", TextMuted, 13, FontWeight.Medium) }
        }
        rules.forEach { rule ->
            ListRow(
                letter = rule.decision.first().uppercase(),
                accent = if (rule.decision == "deny") Danger else AccentGreen,
                title = rule.pattern,
                subtitle = "decision: ${rule.decision}",
                tag = "rule/${rule.id}"
            )
            OutlineButton("Delete", Danger, tag = "rule/delete/${rule.id}") {
                runCatching { bridge.deleteRule(rule.id) }
                rules = runCatching { bridge.rules() }.getOrDefault(rules)
            }
        }
        if (adding) {
            Card(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                FormField("Pattern (e.g. rm -rf*)", pattern, tag = "rule/pattern") { pattern = it }
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    DecisionToggle(Modifier.weight(1f), "allow", decision == "allow", AccentGreen, "rule/decision/allow") { decision = "allow" }
                    DecisionToggle(Modifier.weight(1f), "deny", decision == "deny", Danger, "rule/decision/deny") { decision = "deny" }
                }
                FilledButton("Add rule", AccentSlate, tag = "rule/save") {
                    if (pattern.isNotBlank()) {
                        runCatching { bridge.addRule(pattern.trim(), decision) }
                        pattern = ""
                        adding = false
                        rules = runCatching { bridge.rules() }.getOrDefault(rules)
                    }
                }
            }
        } else {
            FilledButton("Add rule", AccentSlate, tag = "rule/add") { adding = true }
        }
    }
}

/** One half-width allow/deny choice chip. */
@Composable
private fun DecisionToggle(modifier: Modifier, label: String, selected: Boolean, accent: Color, tag: String, onClick: () -> Unit) {
    Box(
        modifier
            .height(48.dp)
            .clip(androidx.compose.foundation.shape.RoundedCornerShape(14.dp))
            .background(if (selected) tinted(accent) else Surface)
            .border(1.dp, if (selected) accent else Line, androidx.compose.foundation.shape.RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .testTag(tag),
        contentAlignment = Alignment.Center
    ) {
        KitText(label, if (selected) accent else TextMuted, 14, FontWeight.Bold)
    }
}

/** 19 — About: only true statements. */
@Composable
fun AboutScreen(bridge: Bridge, onMenu: () -> Unit) {
    val info = remember { bridge.appInfo() }
    val profile = remember { runCatching { bridge.permissionsProfile() }.getOrDefault("unknown") }
    val events = remember { runCatching { bridge.eventCount() }.getOrDefault(0u) }

    ScreenScaffold("About", AccentSlate, onMenu) {
        Card(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            KitText("${info.name} ${info.version}", TextPrimary, 16, FontWeight.Bold)
            KitText(info.tagline, TextMuted, 13, FontWeight.Medium)
            KitText("versionCode ${ai.astrid.app.BuildConfig.VERSION_CODE} (phase × 100 + fix)", TextMuted, 12, FontWeight.Medium)
            KitText("Core version: ${info.version} (same source as this screen)", TextMuted, 12, FontWeight.Medium)
            KitText("Permission profile: $profile", TextMuted, 12, FontWeight.Medium)
            KitText("Audit log: $events events", TextMuted, 12, FontWeight.Medium)
        }
        SectionLabel("UPDATES", AccentSlate)
        Card {
            KitText("No update service is configured in this build.", TextPrimary, 13, FontWeight.Bold)
            KitText(
                "When an update channel exists, checking it will be an audit-logged action " +
                    "like every other one here.",
                TextMuted, 12, FontWeight.Medium
            )
        }
    }
}

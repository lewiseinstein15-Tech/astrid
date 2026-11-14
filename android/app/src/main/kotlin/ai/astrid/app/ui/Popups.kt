package ai.astrid.app.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import ai.astrid.app.R
import uniffi.astrid_ffi.Bridge

/**
 * Overlays: side menu drawer (02), and the model picker sheet (04) — the
 * honest picker: the built-in kilo-auto/free rung always last, any
 * configured provider row from the bridge, and the plain-language guidance
 * when nothing is configured. Panels consume taps so a tap on a gap never
 * falls through to the scrim.
 */

private val scrimLight = Bg.copy(alpha = 0.45f)
private val scrimMedium = Bg.copy(alpha = 0.55f)

/** Consume taps with no ripple or state change: for panels. */
@Composable
private fun blockClicks(): Modifier = Modifier.clickable(
    interactionSource = remember { MutableInteractionSource() },
    indication = null
) { }

/** One 46dp drawer row with colored dot and a tinted background when active. */
@Composable
private fun DrawerRow(label: String, accent: Color, active: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(46.dp)
            .background(if (active) tinted(accent) else Color.Transparent)
            .clickable(onClick = onClick)
            .testTag("drawer/$label"),
        verticalAlignment = Alignment.CenterVertically
    ) {
        Box(Modifier.width(3.dp).height(46.dp).background(if (active) accent else Color.Transparent))
        Row(
            Modifier.padding(horizontal = 13.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp)
        ) {
            Dot(accent)
            KitText(label, TextPrimary, 15, FontWeight.SemiBold)
        }
    }
}

/** 02 — side menu: 300dp drawer from the left over a 55% scrim. */
@Composable
fun SideMenuDrawer(current: Screen, onOpen: (Screen) -> Unit, onClose: () -> Unit) {
    val panelClicks = blockClicks()
    Box(
        Modifier.fillMaxSize().clickable(onClick = onClose).background(scrimMedium),
        contentAlignment = Alignment.CenterStart
    ) {
        Column(
            Modifier
                .width(300.dp)
                .fillMaxSize()
                .background(Surface)
                .border(1.dp, Line)
                .then(panelClicks)
                .statusBarsPadding()
                .padding(bottom = 16.dp)
        ) {
            Row(
                Modifier.padding(horizontal = 10.dp, vertical = 6.dp).height(56.dp),
                verticalAlignment = Alignment.CenterVertically
            ) {
                Image(
                    painter = painterResource(R.drawable.logo_mark),
                    contentDescription = null,
                    modifier = Modifier.size(24.dp)
                )
                Spacer(Modifier.width(6.dp))
                KitText("Astrid", TextPrimary, 18, FontWeight.Bold)
                Spacer(Modifier.weight(1f))
                Box(
                    Modifier.size(44.dp).clickable(onClick = onClose),
                    contentAlignment = Alignment.Center
                ) { IconClose() }
            }
            menuItems.forEach { (label, screen) ->
                DrawerRow(label, screen.accent, screen == current) { onOpen(screen) }
            }
            Spacer(Modifier.height(8.dp))
            Box(Modifier.fillMaxWidth().height(1.dp).background(Line))
            Spacer(Modifier.height(8.dp))
            DrawerRow("Settings", Screen.Settings.accent, current == Screen.Settings) {
                onOpen(Screen.Settings)
            }
        }
    }
}

/** Drawer order (accent per section from DESIGN.md). */
val menuItems: List<Pair<String, Screen>> = listOf(
    "Chat" to Screen.Chat,
    "Live trace" to Screen.LiveTrace,
    "Agents" to Screen.Agents,
    "Sessions" to Screen.Sessions,
    "Memory" to Screen.Memory,
    "Skills" to Screen.Skills,
    "Knowledge" to Screen.Knowledge,
    "Connectors" to Screen.Connectors,
    "Plugins" to Screen.Plugins,
    "MCP Servers" to Screen.McpServers,
    "Scheduler" to Screen.Scheduler,
)

/**
 * 04 — model picker sheet. Rows: the built-in kilo-auto/free automatic rung
 * (served by the offline agent until a provider is added — honest), any
 * configured provider from the bridge, and the plain-language guidance.
 */
@Composable
fun ModelPickerSheet(
    bridge: Bridge,
    onManageProviders: () -> Unit,
    onClose: () -> Unit
) {
    val sheetClicks = blockClicks()
    val provider = runCatching { bridge.provider() }.getOrNull()
    val hasProvider = provider != null && provider.baseUrl.isNotEmpty()
    Box(Modifier.fillMaxSize().clickable(onClick = onClose).background(scrimMedium)) {
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
                .background(Surface)
                .border(1.dp, Line)
                .then(sheetClicks)
                .navigationBarsPadding()
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                KitText("Choose model", TextPrimary, 18, FontWeight.Bold)
                Spacer(Modifier.weight(1f))
                Box(Modifier.size(44.dp).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                    IconClose()
                }
            }

            SectionLabel("BUILT-IN · ALWAYS LAST ON THE LADDER", AccentOrange)
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(56.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(tinted(AccentOrange))
                    .border(1.dp, AccentOrange, RoundedCornerShape(12.dp))
                    .clickable(onClick = onClose)
                    .testTag("picker/kilo-auto-free")
                    .padding(horizontal = 12.dp),
                contentAlignment = Alignment.CenterStart
            ) {
                Column {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        KitText("kilo-auto/free", TextPrimary, 15, FontWeight.SemiBold)
                        Spacer(Modifier.width(8.dp))
                        Chip("Current", AccentOrange)
                    }
                    KitText(
                        if (hasProvider) "Automatic rung; the provider above is tried first"
                        else "Picks the best free path — served by the built-in offline agent",
                        TextMuted, 12, FontWeight.Medium
                    )
                }
            }

            if (hasProvider) {
                SectionLabel("YOUR PROVIDERS", AccentBlue)
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(56.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(Surface)
                        .border(1.dp, AccentBlue, RoundedCornerShape(12.dp))
                        .testTag("picker/provider")
                        .padding(horizontal = 12.dp),
                    contentAlignment = Alignment.CenterStart
                ) {
                    Column {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            KitText(provider!!.name, TextPrimary, 15, FontWeight.SemiBold)
                            Spacer(Modifier.width(8.dp))
                            Chip("First", AccentBlue)
                        }
                        val model = provider.model.ifEmpty { "default model" }
                        KitText("${provider.baseUrl} · $model", TextMuted, 12, FontWeight.Medium)
                    }
                }
            } else {
                Card(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    KitText("No provider yet. Open Settings > Providers.", TextPrimary, 14, FontWeight.Bold)
                    KitText(
                        "Until then every turn is served by the built-in offline agent — " +
                            "deterministic answers, capability questions, and approved tools.",
                        TextMuted, 12, FontWeight.Medium
                    )
                }
            }
            OutlineButton("Open Settings > Providers", AccentSlate, tag = "provider/manage", onClick = onManageProviders)
        }
    }
}

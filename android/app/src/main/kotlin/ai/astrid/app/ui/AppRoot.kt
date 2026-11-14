package ai.astrid.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import uniffi.astrid_ffi.Bridge

/**
 * Screen registry and app root (Phase 3 fix 3). The drawer items and the
 * provider sheet live here; the attach menu and the SAF/camera flows live in
 * the Chat composer (uploads and photos are message attachments now). Every
 * screen renders REAL data from the bridge — no sample data anywhere. The
 * approval popup returns with the approvals phase (9) when the core actually
 * requests one.
 */
enum class Screen(val title: String, val accent: Color) {
    Chat("Astrid", AccentOrange),
    LiveTrace("Live trace", AccentAmber),
    Agents("Agents", AccentCoral),
    Sessions("Sessions", AccentBlue),
    Memory("Memory", AccentPurple),
    Skills("Skills", AccentGreen),
    Knowledge("Knowledge", AccentTeal),
    Connectors("Connectors", AccentPink),
    Plugins("Plugins", AccentYellow),
    McpServers("MCP Servers", AccentIndigo),
    Scheduler("Scheduler", AccentSky),
    Settings("Settings", AccentSlate),
    Providers("Providers", AccentSlate),
    ApprovalRules("Approvals", AccentSlate),
    About("About", AccentSlate),
}

@Composable
fun AstridApp(bridge: Bridge) {
    var screen by remember { mutableStateOf(Screen.Chat) }
    var drawerOpen by remember { mutableStateOf(false) }
    var providerOpen by remember { mutableStateOf(false) }
    // Bumps when a new chat starts or a session is opened; ChatScreen
    // reloads the transcript from the core on change.
    var chatEpoch by remember { mutableIntStateOf(0) }

    fun open(s: Screen) {
        screen = s
        drawerOpen = false
        runCatching { bridge.appendEvent("nav.open", s.title) }
    }

    BackHandler(
        enabled = drawerOpen || providerOpen || screen != Screen.Chat
    ) {
        when {
            providerOpen -> providerOpen = false
            drawerOpen -> drawerOpen = false
            else -> open(Screen.Chat)
        }
    }

    Box(Modifier.fillMaxSize().background(Bg)) {
        when (screen) {
            Screen.Chat -> ChatScreen(
                bridge = bridge,
                chatEpoch = chatEpoch,
                onMenu = { drawerOpen = true; runCatching { bridge.appendEvent("menu.open", "side menu") } },
                onModelClick = { providerOpen = true; runCatching { bridge.appendEvent("provider.open", "model picker") } },
                onNewChat = {
                    runCatching { bridge.newChat() }
                    chatEpoch++
                    runCatching { bridge.appendEvent("chat.new", "new chat started") }
                }
            )
            Screen.LiveTrace -> LiveTraceScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Agents -> AgentsScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Sessions -> SessionsScreen(
                bridge,
                onMenu = { drawerOpen = true },
                onOpenSession = { id ->
                    runCatching { bridge.openSession(id) }
                    chatEpoch++
                    open(Screen.Chat)
                }
            )
            Screen.Memory -> MemoryScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Skills -> SkillsScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Knowledge -> KnowledgeScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Connectors -> ConnectorsScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Plugins -> PluginsScreen(bridge, onMenu = { drawerOpen = true })
            Screen.McpServers -> McpServersScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Scheduler -> SchedulerScreen(bridge, onMenu = { drawerOpen = true })
            Screen.Settings -> SettingsScreen(
                bridge = bridge,
                onMenu = { drawerOpen = true },
                onOpen = ::open
            )
            Screen.Providers -> ProvidersScreen(bridge, onMenu = { drawerOpen = true })
            Screen.ApprovalRules -> ApprovalRulesScreen(bridge, onMenu = { drawerOpen = true })
            Screen.About -> AboutScreen(bridge, onMenu = { drawerOpen = true })
        }

        if (drawerOpen) {
            SideMenuDrawer(current = screen, onOpen = ::open, onClose = { drawerOpen = false })
        }
        if (providerOpen) {
            ModelPickerSheet(
                bridge = bridge,
                onManageProviders = { providerOpen = false; open(Screen.Providers) },
                onClose = { providerOpen = false }
            )
        }
    }
}

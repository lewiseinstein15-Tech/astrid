package ai.astrid.app.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import ai.astrid.app.TaskWorker
import java.text.SimpleDateFormat
import java.util.Locale
import java.util.concurrent.TimeUnit
import uniffi.astrid_ffi.Bridge

/**
 * The remaining work screens — Connectors and Scheduler. Every button calls
 * an Android API or the Rust core. Uploads and photos are message
 * attachments now (Chat composer); their records surface in Knowledge.
 */

/** Connectors: real Android intents; the state row is persisted by the core. */
@Composable
fun ConnectorsScreen(bridge: Bridge, onMenu: () -> Unit) {
    val context = LocalContext.current
    var connectors by remember { mutableStateOf(runCatching { bridge.connectors() }.getOrDefault(emptyList())) }

    fun openConnector(id: String, intent: Intent) {
        runCatching {
            context.startActivity(intent)
            bridge.setConnector(id, "connected")
        }.onSuccess {
            connectors = runCatching { bridge.connectors() }.getOrDefault(connectors)
        }
    }

    ScreenScaffold("Connectors", AccentPink, onMenu) {
        SectionLabel("OPENED VIA ANDROID INTENTS · STATE PERSISTED BY THE CORE", AccentPink)
        connectors.forEach { connector ->
            val state = connector.state
            val lastOpened = if (connector.lastOpened > 0uL) {
                SimpleDateFormat("MMM d, HH:mm", Locale.US)
                    .format(java.util.Date((connector.lastOpened * 1000u).toLong()))
            } else "never"
            ListRow(
                letter = connector.label.first().toString(),
                accent = AccentPink,
                title = connector.label,
                subtitle = "${connector.subtitle} · last opened $lastOpened",
                chipLabel = state,
                chipAccent = if (state == "connected") AccentGreen else AccentSlate,
                tag = "connector/${connector.id}"
            )
            OutlineButton(
                if (state == "connected") "Open again" else "Connect",
                AccentPink,
                tag = "connector/open/${connector.id}"
            ) {
                when (connector.id) {
                    "whatsapp" -> openConnector(
                        connector.id,
                        Intent(Intent.ACTION_VIEW, Uri.parse("https://wa.me/"))
                    )
                    "email" -> openConnector(
                        connector.id,
                        Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:"))
                    )
                    "github" -> openConnector(
                        connector.id,
                        Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com"))
                    )
                }
            }
        }
    }
}

/** Scheduler: core-persisted rows + real WorkManager periodic work. */
@Composable
fun SchedulerScreen(bridge: Bridge, onMenu: () -> Unit) {
    val context = LocalContext.current
    var tasks by remember { mutableStateOf(runCatching { bridge.tasks() }.getOrDefault(emptyList())) }
    var adding by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var minutes by remember { mutableStateOf("15") }

    fun enqueue(task: uniffi.astrid_ffi.TaskRecord) {
        val request = PeriodicWorkRequestBuilder<TaskWorker>(task.everyMinutes.toLong(), TimeUnit.MINUTES)
            .setInputData(androidx.work.workDataOf(TaskWorker.KEY_TASK_NAME to task.name))
            .build()
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            TaskWorker.workName(task.id),
            ExistingPeriodicWorkPolicy.UPDATE,
            request
        )
    }

    ScreenScaffold("Scheduler", AccentSky, onMenu) {
        SectionLabel("TASKS · WORKMANAGER PERIODIC WORK FIRES INTO THE AUDIT LOG", AccentSky)
        if (tasks.isEmpty()) {
            Card { KitText("No tasks yet — add one and WorkManager runs it on-device.", TextMuted, 13, FontWeight.Medium) }
        }
        tasks.forEach { task ->
            ListRow(
                letter = task.name.firstOrNull()?.uppercase() ?: "?",
                accent = AccentSky,
                title = task.name,
                subtitle = "every ${task.everyMinutes} minutes",
                chipLabel = if (task.enabled) "On" else "Off",
                chipAccent = if (task.enabled) AccentGreen else AccentSlate,
                tag = "task/${task.id}"
            )
            OutlineButton(
                if (task.enabled) "Pause" else "Resume",
                AccentSky,
                tag = "task/toggle/${task.id}"
            ) {
                val enable = !task.enabled
                runCatching { bridge.setTaskEnabled(task.id, enable) }
                val wm = WorkManager.getInstance(context)
                if (enable) enqueue(task) else wm.cancelUniqueWork(TaskWorker.workName(task.id))
                tasks = runCatching { bridge.tasks() }.getOrDefault(tasks)
            }
        }
        if (adding) {
            Card {
                FormField("Task name", name, tag = "task/name") { name = it }
                FormField("Interval in minutes (min 15)", minutes, tag = "task/minutes") { minutes = it }
                FilledButton("Schedule task", AccentSky, tag = "task/save") {
                    val mins = minutes.toIntOrNull() ?: 0
                    if (name.isNotBlank() && mins >= 15) {
                        val task = runCatching { bridge.addTask(name.trim(), mins.toUInt()) }.getOrNull()
                        if (task != null) enqueue(task)
                        name = ""
                        minutes = "15"
                        adding = false
                        tasks = runCatching { bridge.tasks() }.getOrDefault(tasks)
                    }
                }
            }
        } else {
            FilledButton("New task", AccentSky, tag = "task/add") { adding = true }
        }
    }
}

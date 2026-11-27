package ai.astrid.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.testTag
import kotlinx.coroutines.delay
import uniffi.astrid_ffi.Bridge
import uniffi.astrid_ffi.EventRecord

/** One waiting approval, ready to show. kind: "turn" | "shell". */
data class ApprovalPending(
    val kind: String,
    val command: String,
    val why: String,
    val approvalId: String,
    val folder: String,
)

/** Row kinds that close a waiting approval. */
private val RESOLVED_KINDS =
    setOf("approval.resolved", "shell.approved", "shell.denied", "shell.ran", "shell.refused")

private fun commandFromDetails(details: String): String =
    Regex("\"command\"\\s*:\\s*\"(.*?)\"").find(details)?.groupValues?.get(1) ?: details

private fun reasonFromDetails(details: String): String =
    Regex("\"reason\"\\s*:\\s*\"(.*?)\"").find(details)?.groupValues?.get(1) ?: ""

private fun shellIdFromRow(text: String): String =
    Regex("^shell_run\\[([^]]+)]:").find(text)?.groupValues?.get(1) ?: ""

private fun shellRowCommand(text: String): String {
    val afterId = text.substringAfter("]: ", text)
    return afterId.substringBefore(" — ").trim()
}

private fun shellRowWhy(text: String): String = text.substringAfter(" — ", "").trim()

/**
 * Derive the current pending approval from the bridge: the core's turn loop
 * signals through approvalPending(); the direct shell surface through an
 * unanswered shell.requested row. Returns null when nothing is waiting.
 */
fun derivePendingApproval(bridge: Bridge): ApprovalPending? {
    val events: List<EventRecord> =
        runCatching { bridge.recentEvents(30u) }.getOrDefault(emptyList())
    val folder = runCatching { bridge.sandboxDir() }.getOrDefault("")
    if (runCatching { bridge.approvalPending() }.getOrDefault(false)) {
        val ask = events.firstOrNull { it.kind == "approval.requested" }
            ?: return ApprovalPending("turn", "", "the agent is waiting for your decision", "", folder)
        val details = ask.text.substringAfter(": ", "")
        return ApprovalPending(
            kind = "turn",
            command = commandFromDetails(details),
            why = reasonFromDetails(details).ifEmpty { "the agent is waiting for your decision" },
            approvalId = "",
            folder = folder,
        )
    }
    val newestResolvedIdx = events.indexOfFirst { it.kind in RESOLVED_KINDS }
    val ask = events.firstOrNull { it.kind == "shell.requested" } ?: return null
    // newest-first: if the ask is older than the newest resolution, it is done.
    val askIdx = events.indexOf(ask)
    if (newestResolvedIdx in 0 until askIdx) return null
    return ApprovalPending(
        kind = "shell",
        command = shellRowCommand(ask.text),
        why = shellRowWhy(ask.text).ifEmpty { "this command needs your approval" },
        approvalId = shellIdFromRow(ask.text),
        folder = folder,
    )
}

/**
 * Phase 5 — the live approval popup. While a risky command waits, this
 * overlay shows the command, the sandbox folder, why it is risky, and the
 * Deny / Approve buttons; the agent waits and Deny stops the command.
 * When the app goes to the background the decision moves to a notification
 * with the same two buttons (ApprovalNotifier).
 */
@Composable
fun ApprovalHost(bridge: Bridge) {
    var pending by remember { mutableStateOf<ApprovalPending?>(null) }

    LaunchedEffect(Unit) {
        while (true) {
            pending = runCatching { derivePendingApproval(bridge) }.getOrNull()
            delay(400)
        }
    }

    val state = pending ?: return
    Box(
        Modifier
            .fillMaxSize()
            .background(Bg.copy(alpha = 0.55f))
            .clickable(enabled = false) { /* the decision is required */ },
        contentAlignment = Alignment.Center
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .padding(24.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(Surface)
                .border(1.dp, AccentOrange, RoundedCornerShape(16.dp))
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)
        ) {
            KitText("Approval needed", TextPrimary, 18, FontWeight.Bold)
            if (state.command.isNotEmpty()) {
                KitText(state.command, TextPrimary, 14, FontWeight.SemiBold,
                    modifier = Modifier.testTag("approval/command"))
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                KitText("folder:", TextMuted, 12, FontWeight.Medium)
                KitText(state.folder, TextMuted, 12, FontWeight.Medium,
                    modifier = Modifier.testTag("approval/folder"))
            }
            KitText(state.why, AccentOrange, 12, FontWeight.Medium,
                modifier = Modifier.testTag("approval/why"))
            Row(Modifier.fillMaxWidth().height(52.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) {
                    OutlineButton("Deny", Danger, tag = "approval/deny") {
                        resolve(state, false)
                        pending = null
                    }
                }
                Box(Modifier.weight(1f)) {
                    FilledButton("Approve", AccentGreen, tag = "approval/approve") {
                        resolve(state, true)
                        pending = null
                    }
                }
            }
            KitText(
                "Deny stops the command. Read-only commands never ask.",
                TextMuted, 11, FontWeight.Medium
            )
        }
    }
}

private fun resolve(state: ApprovalPending, approved: Boolean) {
    // Resolved through the right surface; the FFI appends the audit rows
    // (shell.approved / shell.denied / approval.resolved) for the Live trace.
    runCatching {
        ai.astrid.app.AstridBridge.bridge.let { bridge ->
            if (state.kind == "shell") bridge.resolveShell(state.approvalId, approved)
            else bridge.resolveApproval(approved)
        }
    }
}

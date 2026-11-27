package ai.astrid.app

import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import ai.astrid.app.R

/**
 * Phase 5 — when the app is in the background and a risky command waits,
 * the decision moves here: a notification with Approve and Deny buttons.
 * Tapping either answers the exact pending approval (turn-loop or the
 * direct shell surface) through the same resolve paths as the popup.
 */
object ApprovalNotifier {
    const val CHANNEL_ID = "astrid.approvals"
    const val ACTION_DECIDE = "ai.astrid.app.APPROVAL_DECIDE"
    const val EXTRA_APPROVED = "approved"
    const val EXTRA_KIND = "kind"
    const val EXTRA_ID = "approval_id"
    private const val NOTIFICATION_ID = 4102

    fun ensureChannel(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.createNotificationChannel(
                android.app.NotificationChannel(
                    CHANNEL_ID,
                    "Astrid approvals",
                    android.app.NotificationManager.IMPORTANCE_HIGH,
                )
            )
        }
    }

    fun maybeNotify(context: Context) {
        val bridge = if (::AstridBridge.bridge.isInitialized) AstridBridge.bridge else return
        val pending = runCatching { ai.astrid.app.ui.derivePendingApproval(bridge) }.getOrNull() ?: return
        ensureChannel(context)
        fun decideIntent(approved: Boolean): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                if (approved) 1 else 0,
                Intent(context, ApprovalReceiver::class.java)
                    .setAction(ACTION_DECIDE)
                    .putExtra(EXTRA_APPROVED, approved)
                    .putExtra(EXTRA_KIND, pending.kind)
                    .putExtra(EXTRA_ID, pending.approvalId),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        val notification = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Astrid needs your approval")
            .setContentText(
                if (pending.command.length > 60) pending.command.take(60) + "…"
                else pending.command.ifEmpty { "An agent action is waiting for your decision" }
            )
            .addAction(0, "Deny", decideIntent(false))
            .addAction(0, "Approve", decideIntent(true))
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java)?.notify(NOTIFICATION_ID, notification)
    }

    fun cancel(context: Context) {
        context.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION_ID)
    }
}

/** Answers the notification buttons exactly like the popup does. */
class ApprovalReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ApprovalNotifier.ACTION_DECIDE) return
        val bridge = if (::AstridBridge.bridge.isInitialized) AstridBridge.bridge else return
        val approved = intent.getBooleanExtra(ApprovalNotifier.EXTRA_APPROVED, false)
        val kind = intent.getStringExtra(ApprovalNotifier.EXTRA_KIND) ?: "turn"
        val id = intent.getStringExtra(ApprovalNotifier.EXTRA_ID) ?: ""
        runCatching {
            if (kind == "shell") bridge.resolveShell(id, approved)
            else bridge.resolveApproval(approved)
        }
        ApprovalNotifier.cancel(context)
    }
}

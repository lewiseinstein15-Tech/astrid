package ai.astrid.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Foreground service for LONG agent turns (Phase 4).
 *
 * Why it exists: Android can kill a backgrounded app mid-turn, which cuts an
 * agent off between tool calls. While a turn still runs after the promote
 * delay, Astrid promotes it to the foreground — a persistent notification
 * says exactly that, and nothing is hidden from the user.
 *
 * The `specialUse` type is the honest Android policy choice: an agent turn is
 * not media, not a file transfer, not a call — it is Astrid's own long work,
 * declared through PROPERTY_SPECIAL_USE_FGS_SUBTYPE in the manifest, and the
 * app explains WHY before the first promotion (`rationaleText`, shown by the
 * rationale dialog in ChatScreen). Short turns — and every test turn — finish
 * before the delay fires and never promote.
 */
class TurnService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                active = false
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
            else -> { // ACTION_START, and the null-intent restart path
                ensureChannel(this)
                startForeground(
                    NOTIFICATION_ID,
                    buildNotification(this),
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
                )
                active = true
            }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        active = false
        super.onDestroy()
    }

    companion object {
        const val CHANNEL_ID = "astrid-turns"
        const val NOTIFICATION_ID = 4001
        const val ACTION_START = "ai.astrid.app.TurnService.START"
        const val ACTION_STOP = "ai.astrid.app.TurnService.STOP"

        /** The manifest property value: the honest subtype declaration. */
        const val FGS_SUBTYPE = "long-running agent turns"

        /** A turn still working this long is promoted to the foreground. */
        const val PROMOTE_DELAY_MS = 3_000L

        /** Observable service state; the UI never reads it, the tests do. */
        @Volatile
        var active: Boolean = false
            private set

        /** Promote a running turn to the foreground. */
        fun start(context: Context) {
            context.startForegroundService(
                Intent(context, TurnService::class.java).setAction(ACTION_START)
            )
        }

        /** Demote once the turn ended. */
        fun stop(context: Context) {
            context.startService(
                Intent(context, TurnService::class.java).setAction(ACTION_STOP)
            )
        }

        fun ensureChannel(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            if (manager.getNotificationChannel(CHANNEL_ID) == null) {
                manager.createNotificationChannel(
                    NotificationChannel(
                        CHANNEL_ID,
                        "Astrid turns",
                        NotificationManager.IMPORTANCE_LOW,
                    )
                )
            }
        }

        fun buildNotification(context: Context): Notification =
            NotificationCompat.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle("Astrid is working")
                .setContentText(
                    "An agent turn is running in the foreground so it is not killed mid-task."
                )
                .setOngoing(true)
                .build()

        /** The copy the rationale dialog shows before the first promotion. */
        fun rationaleText(): String =
            "For long turns Astrid runs a foreground service: a persistent " +
                "notification keeps Android from killing an agent mid-task. " +
                "It appears only while a turn is still working, it reads " +
                "\"Astrid is working\", and it goes away when the turn ends."
    }
}

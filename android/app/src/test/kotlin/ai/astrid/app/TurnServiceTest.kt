package ai.astrid.app

import android.app.Notification
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Phase 4: the foreground turn service on the desktop JVM (Robolectric).
 * The service object is the real thing — created, promoted with
 * startForeground, demoted by the stop action — and the manifest
 * declaration is checked as Android will see it: specialUse type, the
 * honest subtype property, and the two permissions promotion needs.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class TurnServiceTest {

    private val context: Context = ApplicationProvider.getApplicationContext()

    /** Create the service for real and hand it the start command. */
    private fun startService(): TurnService {
        val intent = Intent(context, TurnService::class.java).setAction(TurnService.ACTION_START)
        val service = Robolectric.buildService(TurnService::class.java, intent).create().get()
        service.onStartCommand(intent, 0, 1)
        return service
    }

    @Test
    fun longTurnPromotesToForegroundWithNamedNotification() {
        val service = startService()
        assertTrue("a started turn must report active", TurnService.active)

        // The channel is created before promotion, IMPORTANCE_LOW (quiet).
        val manager = context.getSystemService(NotificationManager::class.java)
        val channel = manager.getNotificationChannel(TurnService.CHANNEL_ID)
        assertNotNull("channel astrid-turns must exist", channel)
        assertEquals(NotificationManager.IMPORTANCE_LOW, channel.importance)

        // The foreground notification is named — nothing silent, ever.
        val notification = shadowOf(service).lastForegroundNotification
        assertEquals("Astrid is working", notification.extras.getString(Notification.EXTRA_TITLE))
        assertEquals(
            "An agent turn is running in the foreground so it is not killed mid-task.",
            notification.extras.getString(Notification.EXTRA_TEXT)
        )
    }

    @Test
    fun stopActionDemotesAndClearsState() {
        val service = startService()
        assertTrue(TurnService.active)

        val stopIntent = Intent(context, TurnService::class.java).setAction(TurnService.ACTION_STOP)
        service.onStartCommand(stopIntent, 0, 2)
        assertFalse("the stop action must demote the service", TurnService.active)
        assertTrue(
            "the stop action must call stopSelf",
            shadowOf(service).isStoppedBySelf()
        )

        // Teardown clears the flag too (restart-after-kill safety).
        service.onDestroy()
        assertFalse(TurnService.active)
    }

    @Test
    fun manifestDeclaresSpecialUseServiceWithSubtype() {
        val pm = context.packageManager
        val info = pm.getServiceInfo(
            ComponentName(context, TurnService::class.java),
            PackageManager.GET_META_DATA
        )
        assertEquals("ai.astrid.app.TurnService", info.name)
        assertFalse("the turn service must not be exported", info.exported)
        assertEquals(
            "foreground service type must be specialUse",
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE.toLong(),
            info.foregroundServiceType.toLong()
        )

        // The honest subtype: what the service does, declared to Android.
        // Robolectric does not implement PackageManager.getProperty (it fails
        // with NameNotFound "unsupported"), so the property is verified
        // against the merged manifest AGP produces on disk.
        val manifestDirs = listOf(
            java.io.File("build/intermediates/merged_manifests"),
            java.io.File("app/build/intermediates/merged_manifests"),
        ).filter { it.isDirectory }
        val manifestText = manifestDirs
            .asSequence()
            .flatMap { it.walkTopDown().filter { f -> f.name == "AndroidManifest.xml" } }
            .map { it.readText() }
            .firstOrNull { it.contains("PROPERTY_SPECIAL_USE_FGS_SUBTYPE") }
        assertNotNull(
            "PROPERTY_SPECIAL_USE_FGS_SUBTYPE must be declared in the merged manifest " +
                "(scanned: ${manifestDirs.map { it.path }})",
            manifestText
        )
        assertTrue(
            "the subtype must be a <property> element",
            manifestText!!.contains("<property")
        )
        assertTrue(
            "the subtype value must equal TurnService.FGS_SUBTYPE (${TurnService.FGS_SUBTYPE})",
            manifestText.contains("android:value=\"${TurnService.FGS_SUBTYPE}\"")
        )

        // The permissions promotion needs, requested by the app.
        val requested = pm
            .getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS)
            .requestedPermissions
            ?.toList()
            .orEmpty()
        assertTrue(
            "FOREGROUND_SERVICE must be requested",
            requested.contains("android.permission.FOREGROUND_SERVICE")
        )
        assertTrue(
            "FOREGROUND_SERVICE_SPECIAL_USE must be requested",
            requested.contains("android.permission.FOREGROUND_SERVICE_SPECIAL_USE")
        )
    }

    @Test
    fun rationaleCopyExplainsNotificationAndLifecycle() {
        val text = TurnService.rationaleText()
        assertTrue(text.contains("foreground service"))
        assertTrue(text.contains("notification"))
        assertTrue(text.contains("\"Astrid is working\""))
        assertTrue(text.contains("turn ends"))
        assertTrue(
            "the rationale is a short plain-language paragraph",
            text.length in 120..600
        )
    }
}

package ai.astrid.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import ai.astrid.app.ui.AstridApp
import uniffi.astrid_ffi.Bridge

/**
 * Process-wide bridge. The UI, the scheduler worker, and the tests all use
 * this one instance; the core (session turn loop + SQLite store) lives in
 * app-private storage behind it.
 */
object AstridBridge {
    lateinit var bridge: Bridge
        private set

    fun init(filesDir: java.io.File) {
        if (::bridge.isInitialized) return
        // The event log + chat store live in app-private storage; the paths
        // are handed to Rust, which owns the SQLite connection and the turn
        // loop. Every process start logs a core.boot event.
        bridge = Bridge.withCore(
            dbPath = filesDir.resolve("astrid.db").absolutePath,
            dataDir = filesDir.resolve("core").absolutePath,
        )
    }
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        AstridBridge.init(filesDir)
        setContent {
            AstridApp(bridge = AstridBridge.bridge)
        }
    }
}

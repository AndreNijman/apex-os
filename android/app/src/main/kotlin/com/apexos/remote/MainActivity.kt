package com.apexos.remote

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.fragment.app.FragmentActivity
import com.apexos.remote.core.Device
import com.apexos.remote.core.Pairing
import com.apexos.remote.ui.ApexRemoteApp

/**
 * The one activity.
 *
 * A [FragmentActivity] and not a `ComponentActivity`, which is not a style
 * choice: `androidx.biometric`'s `BiometricPrompt` requires one, and every
 * unlock in this app goes through it.
 */
class MainActivity : FragmentActivity() {
    private var payload by mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        payload = pairingPayload(intent)
        setContent {
            ApexRemoteApp(
                activity = this,
                deviceName = defaultDeviceName(),
                launchPayload = payload,
                // Cleared once a screen has it. Otherwise locking and
                // unlocking again would navigate back to a code that has since
                // expired, and the user would be looking at a failure they did
                // nothing to cause.
                onPayloadConsumed = { payload = null },
            )
        }
    }

    /**
     * A second `apex-remote:` link while the app is already open.
     *
     * The activity is `singleTask`, so this arrives here instead of stacking a
     * second copy of the app on top of the first — which would mean two
     * view models, two stores and a race over the same file.
     */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        payload = pairingPayload(intent)
    }

    /**
     * The payload out of a VIEW intent, if it is one.
     *
     * Checked for the scheme rather than trusted: this filter is exported, so
     * anything on the phone can send an intent to it. Nothing here acts on the
     * payload — it opens the screen that explains what accepting it would mean
     * — and that ordering is the point. A link that paired a phone by being
     * opened would be a link worth sending somebody.
     */
    private fun pairingPayload(intent: Intent?): String? {
        val data = intent?.data?.toString() ?: return null
        return data.takeIf { it.startsWith(Pairing.SCHEME) }
    }

    /**
     * What this phone calls itself to the desktop.
     *
     * `Build.MODEL` because it is what the owner will recognise in
     * `apex remote devices`, run through the protocol's own name check so a
     * device whose model string is hostile — a newline would put a second
     * request into `apex-agentd`'s control protocol — never reaches the wire.
     * A model that fails the check falls back rather than refusing to pair.
     */
    private fun defaultDeviceName(): String {
        val candidate = listOf(Build.MODEL, Build.DEVICE, "android")
        for (name in candidate) {
            if (name.isNullOrBlank()) continue
            runCatching { Device.checkName(name) }.onSuccess { return it }
        }
        return "android"
    }
}

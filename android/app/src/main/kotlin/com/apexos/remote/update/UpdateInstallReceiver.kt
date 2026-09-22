package com.apexos.remote.update

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build

/**
 * Where `PackageInstaller` reports back, and where the user's install prompt
 * is actually raised.
 *
 * `session.commit()` does not show anything. It answers on this receiver with
 * [PackageInstaller.STATUS_PENDING_USER_ACTION] and an intent *inside* the
 * broadcast, and it is starting THAT intent which draws the system's "do you
 * want to update this app?" screen. An app that commits a session and never
 * reads this receiver has an update that silently never happens — and a
 * pairing screen that spins forever is exactly the shape of failure this unit
 * was asked to eliminate.
 *
 * Declared in the manifest rather than registered at runtime, and not as a
 * style choice: the status can arrive after this process has gone, and a
 * receiver that only exists while the app is in the foreground would miss it.
 * `android:exported="false"`, so only the system's own broadcast reaches it.
 */
class UpdateInstallReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, -1)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = confirmation(intent) ?: return
                // NEW_TASK because a receiver has no task of its own to start
                // an activity into. Without it this throws and the prompt the
                // whole flow exists to show is never drawn.
                confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                runCatching { context.startActivity(confirm) }
            }
            // Everything else — success, the user saying no, a failure inside
            // the installer — is deliberately not surfaced from here. On
            // success this process is replaced, so there is nobody to tell. On
            // refusal the user is the one who refused and does not need to be
            // told what they just did. A toast fired from a receiver that may
            // run with no app on screen is noise at best.
            else -> Unit
        }
    }

    /**
     * The confirmation intent out of the broadcast.
     *
     * Read through the typed getter on API 33 and later. The untyped
     * `getParcelableExtra` is deprecated there, and `:app` is not compiled
     * with warnings as errors — but this is an intent that will be handed
     * straight to `startActivity`, and taking the checked route where the
     * platform offers one is the cheaper habit.
     */
    @Suppress("DEPRECATION")
    private fun confirmation(intent: Intent): Intent? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_INTENT)
        }

    companion object {
        /**
         * Namespaced under this app's own package and sent with
         * `setPackage()`, so it is an explicit broadcast to one component and
         * not an announcement any app on the phone could listen to.
         */
        const val ACTION = "com.apexos.remote.UPDATE_STATUS"
    }
}

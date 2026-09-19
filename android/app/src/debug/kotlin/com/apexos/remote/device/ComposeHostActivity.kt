package com.apexos.remote.device

import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity

/**
 * The activity every Compose test in this module composes into.
 *
 * Its own class rather than `ui-test-manifest`'s `ComponentActivity`, for one
 * reason: the manifest entry beside this file declares `showWhenLocked` and
 * `turnScreenOn`, and without them nothing composes on a phone with a screen
 * lock. See that manifest for the measurement.
 *
 * The API-27 calls are made as well as the manifest attributes, because a
 * `singleTask`-style relaunch of an already-created activity does not re-read
 * the manifest flags.
 */
class ComposeHostActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
    }
}

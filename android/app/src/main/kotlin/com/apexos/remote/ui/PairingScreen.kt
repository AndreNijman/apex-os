package com.apexos.remote.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.apexos.remote.pairing.QrScanner
import com.apexos.remote.ui.theme.MachineText
import java.util.concurrent.Executors

/**
 * Onboarding, and the screen that has to explain trust rather than assert it.
 *
 * P1-053 asks for "QR onboarding that explains what trust is being established".
 * The explanation below is written from the protocol rather than from
 * reassurance, because the protocol is what is actually true:
 *
 * * The code carries the machine's **public key**, and this phone pins it. A
 *   machine that does not hold the matching private half cannot complete the
 *   handshake — there is no warning to click past, because there is no way for
 *   the wrong machine to get far enough to produce one.
 * * The code also carries a **token**, which proves whoever scanned it was
 *   looking at that screen. It is good once and for three minutes.
 * * The addresses in the code authenticate nothing and do not need to. Reaching
 *   the wrong address produces a handshake that fails, not a connection to
 *   somewhere unexpected.
 * * This phone generates a **fresh key for this machine**, used with it and
 *   nothing else, wrapped by the device keystore behind the screen lock.
 *
 * Every one of those sentences is a property something in `:core` tests. That
 * is the standard for what this screen is allowed to claim.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingScreen(
    state: UiState,
    onPayload: (String) -> Unit,
    onBack: () -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    var typing by remember { mutableStateOf(false) }
    var typed by remember { mutableStateOf("") }
    val asker = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted = it }

    // Asked at the moment the camera is wanted, which is the only moment it
    // means anything to the person answering.
    LaunchedEffect(Unit) {
        if (!granted) asker.launch(Manifest.permission.CAMERA)
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Add a computer", style = MaterialTheme.typography.titleMedium) },
                navigationIcon = { TextButton(onClick = onBack) { Text("Back") } },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .padding(padding)
                .verticalScroll(rememberScrollState()),
        ) {
            state.busy?.let { Notice(it, alarming = false) }
            state.failure?.let { Notice(it, alarming = true, onDismiss = onDismiss) }

            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(20.dp)
                    .aspectRatio(1f)
                    .clip(RoundedCornerShape(12.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center,
            ) {
                if (granted && !typing) {
                    CameraScanner(onText = onPayload)
                } else {
                    Column(
                        Modifier.padding(24.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.Center,
                    ) {
                        Text(
                            if (typing) {
                                "Paste the line that `apex remote pair --text` printed."
                            } else {
                                "APEX Remote needs the camera to read the pairing code. " +
                                    "You can paste the code instead."
                            },
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (typing) {
                            Spacer(Modifier.height(12.dp))
                            BasicTextField(
                                value = typed,
                                onValueChange = { typed = it },
                                textStyle = MachineText.copy(
                                    color = MaterialTheme.colorScheme.onSurface,
                                ),
                                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .background(MaterialTheme.colorScheme.background)
                                    .padding(10.dp),
                            )
                            Spacer(Modifier.height(12.dp))
                            Button(
                                onClick = { onPayload(typed.trim()) },
                                enabled = typed.isNotBlank(),
                                shape = RoundedCornerShape(6.dp),
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = MaterialTheme.colorScheme.primary,
                                    contentColor = MaterialTheme.colorScheme.onPrimary,
                                ),
                            ) { Text("Pair") }
                        }
                    }
                }
            }

            TextButton(
                onClick = { typing = !typing },
                modifier = Modifier.padding(horizontal = 12.dp),
            ) { Text(if (typing) "Use the camera instead" else "Paste a code instead") }

            TrustExplanation()
        }
    }
}

/**
 * The four sentences, on the screen rather than only in a comment.
 *
 * Written as what happens rather than as what is safe. "Your connection is
 * secure" is a claim the reader can only take on faith; "this phone pins that
 * machine's key, and a machine that does not hold the matching half cannot
 * finish the handshake" is a claim they can check.
 */
@Composable
private fun TrustExplanation() {
    Column(Modifier.padding(20.dp)) {
        Text("What scanning the code does", style = MaterialTheme.typography.titleMedium)
        Spacer(Modifier.height(12.dp))
        for ((heading, body) in POINTS) {
            Text(heading, style = MaterialTheme.typography.labelLarge)
            Text(
                body,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(14.dp))
        }
        Text(
            "The code is good once, and for three minutes. If it expires, run " +
                "`apex remote pair` again.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

private val POINTS = listOf(
    "This phone pins that computer's key" to
        "The code carries the machine's public key. From now on only a machine holding the " +
        "matching private half can complete a handshake with this phone — a different one " +
        "cannot get far enough to ask you to trust it.",
    "The code proves you saw the screen" to
        "It carries a one-time token that the machine redeems. Somebody who guesses your " +
        "machine's address still has nothing to send.",
    "A key of its own, for this computer only" to
        "This phone makes a fresh key for this machine and uses it nowhere else, so revoking " +
        "this phone on one computer tells nobody anything about the others.",
    "Kept where this phone cannot read it" to
        "The new key is wrapped by the device keystore behind your screen lock or fingerprint. " +
        "The app can ask the keystore to use it and can never get the key itself.",
)

/**
 * The camera, and the decoder behind it.
 *
 * The analyser runs on its own single thread, not on the main one and not on a
 * pool: ZXing's `MultiFormatReader` keeps per-frame state, so exactly one frame
 * may be inside it at a time. `STRATEGY_KEEP_ONLY_LATEST` is what keeps that
 * from turning into a queue of stale frames when decoding is slower than the
 * camera — a scanner that falls behind reads a code that is no longer on screen.
 */
@Composable
private fun CameraScanner(onText: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val scanner = remember { QrScanner() }
    val executor = remember { Executors.newSingleThreadExecutor() }
    // `rememberUpdatedState` and not the parameter: the analyser is installed
    // once and outlives several recompositions, and capturing the first lambda
    // would keep calling into a screen that has moved on.
    val deliver by rememberUpdatedState(onText)
    // One payload per visit. A QR code stays in frame for many frames, and a
    // scanner that fired on each of them would start a second pairing while the
    // first was still on the socket.
    val seen = remember { java.util.concurrent.atomic.AtomicBoolean(false) }

    DisposableEffect(Unit) {
        onDispose {
            executor.shutdown()
            ProcessCameraProvider.getInstance(context).get().unbindAll()
        }
    }

    AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { ctx ->
            val view = PreviewView(ctx)
            val future = ProcessCameraProvider.getInstance(ctx)
            future.addListener(
                {
                    val provider = future.get()
                    val preview = Preview.Builder().build()
                    preview.setSurfaceProvider(view.surfaceProvider)
                    val analysis = ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                    analysis.setAnalyzer(executor) { image ->
                        try {
                            val text = scanner.decode(image)
                            if (text != null && seen.compareAndSet(false, true)) {
                                ContextCompat.getMainExecutor(ctx).execute { deliver(text) }
                            }
                        } catch (_: Exception) {
                            // A frame that will not decode is the ordinary
                            // case, and an analyser that threw would take the
                            // camera down with it.
                        } finally {
                            image.close()
                        }
                    }
                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner,
                        CameraSelector.DEFAULT_BACK_CAMERA,
                        preview,
                        analysis,
                    )
                },
                ContextCompat.getMainExecutor(ctx),
            )
            view
        },
    )
}

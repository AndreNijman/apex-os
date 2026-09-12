plugins {
    // `org.jetbrains.kotlin.android` is deliberately absent: AGP 9 builds
    // Kotlin itself and refuses the standalone plugin outright. `:core` still
    // applies `kotlin.jvm`, because it is not an Android module and AGP is not
    // in it at all.
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "com.apexos.remote"
    // 36, which is what AGP 9 wants and what is actually installed. An
    // earlier attempt at 35 failed with "Build properties not found": the
    // command-line tools' unzip of `platforms;android-35` crashed with a
    // `DirectoryNotEmptyException` and left a directory holding nothing but
    // `package.xml`, which every tool then read as "installed".
    compileSdk = 36

    defaultConfig {
        applicationId = "com.apexos.remote"
        // 28, and the number is a security decision rather than a reach
        // decision. It is the first release with `BiometricPrompt` in the
        // platform and with a keystore that can hold an AES key marked
        // `setUserAuthenticationRequired`, which is what P1-053's app lock and
        // its "no private keys in insecure app storage" both rest on. Going
        // lower would mean shipping a build where the device key is protected
        // by nothing, on exactly the phones least able to defend it.
        minSdk = 28
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    // ── Release signing (P1-060) ────────────────────────────────────────────
    //
    // Every value comes from the environment and NOTHING is committed. Not a
    // `keystore.properties`, not a debug-shaped fallback keystore, not a
    // default password. A signing key in a repository is a signing key for
    // everyone who can clone it, and for an app that unseals a device key and
    // shows which machine is about to run something as root, a forged build is
    // the whole game.
    //
    // The config is created only when the environment carries all four values.
    // That is the important half: with them absent the release build is
    // UNSIGNED and says so, rather than falling back to the debug key — which
    // every Android project ships by default and which would produce an
    // installable artefact signed by a key whose password is the string
    // "android". `verifyReleaseSigning` below is what refuses to call that
    // outcome success.
    val signingEnv = listOf(
        "APEX_KEYSTORE",
        "APEX_KEYSTORE_PASSWORD",
        "APEX_KEY_ALIAS",
        "APEX_KEY_PASSWORD",
    ).associateWith { System.getenv(it) }
    val signingReady = signingEnv.values.all { !it.isNullOrBlank() } &&
        file(signingEnv["APEX_KEYSTORE"]!!).isFile

    signingConfigs {
        if (signingReady) {
            create("release") {
                storeFile = file(signingEnv["APEX_KEYSTORE"]!!)
                storePassword = signingEnv["APEX_KEYSTORE_PASSWORD"]
                keyAlias = signingEnv["APEX_KEY_ALIAS"]
                keyPassword = signingEnv["APEX_KEY_PASSWORD"]
                // v1 off, v2 and v3 on. minSdk is 28, so every phone this app
                // supports verifies v2; v1 is the JAR-signature scheme whose
                // Janus and Master Key families of bugs are the reason v2
                // exists, and leaving it on means shipping that attack surface
                // to nobody's benefit.
                enableV1Signing = false
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        release {
            // Off for now, and deliberately: nothing is published yet, and a
            // shrinker configured before there is anything to shrink produces
            // keep-rules nobody can justify. It goes on with the first release
            // build, together with the rules `kotlinx.serialization` needs.
            //
            // Turning it on unverified would be worse than leaving it off. R8
            // strips the generated serializers unless it is told not to, and
            // the failure is at RUNTIME on a device — which is exactly the
            // thing no test in this repository can reach.
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources {
            // BouncyCastle ships signature files that two copies of the jar
            // would collide on.
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    // The protocol, which knows nothing about Android and is tested without it.
    implementation(project(":core"))

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.core)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.biometric)
    // Not imported anywhere. It is here to raise the floor: see the note on
    // `fragment` in libs.versions.toml — biometric 1.1.0 pulls a FragmentActivity
    // that mishandles every activity result this app takes.
    implementation(libs.androidx.fragment)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.zxing.core)
    implementation(libs.kotlinx.coroutines.core)

    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

tasks.withType<Test>().configureEach {
    useJUnitPlatform()
}

// ── Is the release artefact actually signed? ─────────────────────────────────
//
// A task rather than a comment, because "we sign it in CI" is a claim and this
// is a check. It reads the APK's own signature blocks with `apksigner verify`
// and refuses to report success for an unsigned build.
//
// It is deliberately NOT wired into `assembleRelease`. A developer building a
// release APK locally to look at it has no keystore and should not need one;
// what must never happen is a PIPELINE reporting a green release having
// produced something nobody can install. So CI runs this task by name, and a
// missing signature fails there.
tasks.register("verifyReleaseSigning") {
    group = "verification"
    description = "Fail unless the release APK carries a v2/v3 signature."
    dependsOn("assembleRelease", "bundleRelease")
    doLast {
        val dir = layout.buildDirectory.dir("outputs/apk/release").get().asFile
        val apks = (dir.listFiles()?.filter { it.name.endsWith(".apk") } ?: emptyList())
        if (apks.isEmpty()) {
            throw GradleException(
                "no release APK was produced under $dir, so there is nothing to check. This " +
                    "task is about a signature; a missing artefact is a different failure and " +
                    "is reported as one.",
            )
        }
        val sdk = System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT")
            ?: throw GradleException("ANDROID_HOME is unset, so apksigner cannot be found")
        val signer = File(sdk, "build-tools").listFiles()
            ?.sortedBy { it.name }
            ?.mapNotNull { File(it, "apksigner").takeIf(File::canExecute) }
            ?.lastOrNull()
            ?: throw GradleException(
                "apksigner was not found under $sdk/build-tools. A run that could not look " +
                    "must fail rather than report that it found no problem.",
            )
        for (apk in apks) {
            val result = providers.exec {
                commandLine(signer.absolutePath, "verify", "--print-certs", apk.absolutePath)
                isIgnoreExitValue = true
            }
            val code = result.result.get().exitValue
            val text = result.standardOutput.asText.get() + result.standardError.asText.get()
            if (code != 0) {
                throw GradleException(
                    "${apk.name} is not signed, so it cannot be installed and must not be " +
                        "reported as a release build.\n" +
                        "Set APEX_KEYSTORE, APEX_KEYSTORE_PASSWORD, APEX_KEY_ALIAS and " +
                        "APEX_KEY_PASSWORD.\n" + text,
                )
            }
            logger.lifecycle("signed: ${apk.name}")
            text.lines().filter { it.startsWith("Signer") }.forEach { logger.lifecycle("  $it") }
        }

        // The AAB is the artefact a store actually takes, and it is signed by a
        // different scheme: a bundle is a jar, so apksigner does not read it
        // and jarsigner does. Checking only the APK would leave the thing that
        // ships unchecked.
        val bundleDir = layout.buildDirectory.dir("outputs/bundle/release").get().asFile
        val bundles = bundleDir.listFiles()?.filter { it.name.endsWith(".aab") } ?: emptyList()
        if (bundles.isEmpty()) {
            throw GradleException("no release bundle was produced under $bundleDir")
        }
        val jarsigner = File(System.getProperty("java.home"), "bin/jarsigner")
        if (!jarsigner.canExecute()) {
            throw GradleException(
                "jarsigner was not found at $jarsigner, so the bundle's signature could not " +
                    "be read. A run that could not look must fail rather than report that it " +
                    "found no problem.",
            )
        }
        for (aab in bundles) {
            val r = providers.exec {
                commandLine(jarsigner.absolutePath, "-verify", aab.absolutePath)
                isIgnoreExitValue = true
            }
            val outText = r.standardOutput.asText.get() + r.standardError.asText.get()
            // MEASURED, not assumed: jarsigner -verify on the unsigned bundle
            // this project produces without a keystore prints "no manifest."
            // and EXITS 0. Reading the exit code alone would therefore pass
            // every unsigned bundle, which is the exact outcome this task
            // exists to prevent. A signed one prints "jar verified.", so that
            // string is the condition and the exit code is only a second
            // opinion.
            if (r.result.get().exitValue != 0 || !outText.contains("jar verified")) {
                throw GradleException(
                    "${aab.name} is not signed, so a store would reject it.\n$outText",
                )
            }
            logger.lifecycle("signed: ${aab.name}")
        }
    }
}

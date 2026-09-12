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

    buildTypes {
        release {
            // Off for now, and deliberately: nothing is published yet, and a
            // shrinker configured before there is anything to shrink produces
            // keep-rules nobody can justify. It goes on with the first release
            // build, together with the rules `kotlinx.serialization` needs.
            isMinifyEnabled = false
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

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.blre.launcher"
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.blre.launcher"
        minSdk = 23
        targetSdk = 34
        versionCode = 3
        versionName = "0.3.0"

        // Only the ABIs the Rust core is actually built for. Declaring an ABI the
        // jniLibs directory does not contain produces an APK that installs and
        // then fails at System.loadLibrary on that device — the worst outcome,
        // because the failure happens after the install "succeeded".
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            // Nothing is shrunk yet: R8 on a Compose app with a reflective-free
            // JNI edge is safe, but there is no reason to pay for it before the
            // launcher does anything worth protecting.
            isMinifyEnabled = false
        }
    }

    // A signing key whose location *this project* decides, instead of AGP's
    // default `~/.android/debug.keystore`.
    //
    // That default is regenerated with a random key whenever it is missing, and
    // on a CI runner it was missing every time - so every build was signed by a
    // different identity and Android refused to install one over the other
    // ("app not installed"). CI caches this directory and creates the key on a
    // miss, which makes the identity stable. Locally, with no key present, the
    // build falls back to AGP's default so a fresh clone still works.
    signingConfigs {
        getByName("debug") {
            val key = rootProject.file(".keystore/debug.keystore")
            if (key.exists()) {
                storeFile = key
                storePassword = "android"
                keyAlias = "androiddebugkey"
                keyPassword = "android"
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")

    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
}

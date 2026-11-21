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
        versionCode = 1
        versionName = "0.1.0"

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

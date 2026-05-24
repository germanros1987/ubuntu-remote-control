plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.urc.android"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.urc.android"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        debug {
            // WebView remote debugging is enabled at runtime ONLY in debug builds
            // (see MainActivity); release builds never call
            // setWebContentsDebuggingEnabled(true).
            isMinifyEnabled = false
        }
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Unsigned by default; CI signs with the release keystore when secrets
            // are present, otherwise produces an unsigned APK for manual signing.
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
        viewBinding = true
        // BuildConfig.DEBUG gates WebView remote debugging (MainActivity); AGP 8
        // does not generate BuildConfig unless asked.
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.constraintlayout:constraintlayout:2.1.4")
    implementation("androidx.recyclerview:recyclerview:1.3.2")
    implementation("androidx.activity:activity-ktx:1.9.2")

    // Coroutines for the proxy accept loop / byte pumps.
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    // Persisted host list.
    implementation("androidx.datastore:datastore-preferences:1.1.1")

    // QR scanning: CameraX preview + ML Kit barcode (bundled model, offline).
    implementation("androidx.camera:camera-core:1.3.4")
    implementation("androidx.camera:camera-camera2:1.3.4")
    implementation("androidx.camera:camera-lifecycle:1.3.4")
    implementation("androidx.camera:camera-view:1.3.4")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")

    // HostStore persistence uses org.json (bundled in the Android SDK) — no extra
    // serialization plugin needed.
}

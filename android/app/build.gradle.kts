plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.recall.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.recall.app"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    // Compose
    val composeBom = platform("androidx.compose:compose-bom:2026.06.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    // Icons.Filled.{Mic,Stop,Storage,...} live outside the small default icon set.
    implementation("androidx.compose.material:material-icons-extended")
    debugImplementation("androidx.compose.ui:ui-tooling")

    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.navigation:navigation-compose:2.9.8")

    // View-based Material3 theme used as the manifest/launch theme host for the Compose UI.
    implementation("com.google.android.material:material:1.14.0")

    // UniFFI-generated bindings (Task 3) load the native library via JNA at runtime.
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    // Coroutines for bridging suspend/Flow-based bindings and app-side async work.
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")

    // ModelManager: extracting the parakeet .tar.bz2 model bundle on-device.
    implementation("org.apache.commons:commons-compress:1.28.0")
}

plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    // namespace = the code/R package (where MainActivity lives), kept as the
    // flutter-create default so the manifest's ".MainActivity" resolves. This
    // is intentionally NOT the applicationId — the on-device identity below is
    // com.incubtek.jeliya to match the macOS bundle id.
    namespace = "com.incubtek.jeliya_app"
    compileSdk = flutter.compileSdkVersion
    // The libjeliya_ffi.so in jniLibs are linked with NDK r29 (scripts/
    // build-android-libs.mjs); keep the packaging toolchain coherent.
    ndkVersion = "29.0.14206865"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        applicationId = "com.incubtek.jeliya"
        // minSdk 26 (Android 8) — settled floor; the runtime-proof .so link
        // against API 26 and the in-market low-end target devices.
        minSdk = 26
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
        // Ship all three in-market ABIs. armeabi-v7a is REQUIRED: real target
        // devices (e.g. moto g play 2023) run 32-bit-only Android builds.
        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

flutter {
    source = "../.."
}

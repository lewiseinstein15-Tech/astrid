import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// ---------------------------------------------------------------------------
// Version: ONE source of truth — the VERSION file at the repo root.
// Format major.minor.phase[.fix], e.g. "0.1.3.3" (phase 3, third reject).
//   versionName = the file content, verbatim
//   versionCode = phase * 100 + fix        (0.1.3.3 -> 303)
// The Rust core bakes the same file in at compile time (build.rs -> ASTRID_VERSION),
// so the Chat card, the About screen, and the APK manifest always agree.
// ---------------------------------------------------------------------------
val astridVersion = rootProject.file("../VERSION").readText().trim()
val astridVersionParts = astridVersion.split('.')
check(astridVersionParts.size >= 3) { "VERSION must be major.minor.phase[.fix], got '$astridVersion'" }
val astridPhase = astridVersionParts[2].toInt()
val astridFix = astridVersionParts.getOrNull(3)?.toInt() ?: 0

val debugKeystore = rootProject.file("app/debug.keystore")

android {
    namespace = "ai.astrid.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "ai.astrid.app"
        minSdk = 35
        targetSdk = 36
        versionCode = astridPhase * 100 + astridFix
        versionName = astridVersion
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    signingConfigs {
        getByName("debug") {
            if (debugKeystore.exists()) {
                storeFile = debugKeystore
                storePassword = "astrid"
                keyAlias = "astriddebug"
                keyPassword = "astrid"
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug") // replaced in phase 10
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    testOptions {
        unitTests {
            isIncludeAndroidResources = true
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.foundation)
    // Real scheduling: the Scheduler screen mirrors its rows in WorkManager.
    implementation(libs.androidx.work.runtime)
    implementation(libs.jna) {
        artifact {
            type = "aar"
        }
    }

    testImplementation(libs.kotlin.test)
    testImplementation(libs.jna.jvm)
    testImplementation(libs.androidx.compose.ui.test.junit4)
    testImplementation(libs.androidx.compose.ui.test.manifest)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.ext.junit)
    testImplementation(libs.androidx.test.core)
    testImplementation(libs.junit4)
    testImplementation(libs.androidx.work.testing)
    testRuntimeOnly(libs.junit.vintage)
}

tasks.withType<Test>().configureEach {
    useJUnitPlatform()
    systemProperty("jna.library.path", layout.projectDirectory.dir("src/test/natives").asFile.absolutePath)
    systemProperty("astrid.version", astridVersion)
    // Maven Central rate-limits the android-all fetch; the mirror is the
    // same artifact set.
    systemProperty("robolectric.dependency.repo.url", "https://maven.aliyun.com/repository/public")
    systemProperty("robolectric.dependency.repo.id", "aliyun")
    maxHeapSize = "2g"
}

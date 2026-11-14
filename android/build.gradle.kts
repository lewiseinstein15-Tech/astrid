// Root build: plugin versions come from gradle/libs.versions.toml; no
// repositories here (settings.gradle.kts owns them).
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

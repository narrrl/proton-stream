import org.gradle.api.tasks.Exec
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

val repositoryRoot = rootProject.projectDir.parentFile
val workspaceVersion = Regex("""(?m)^version\s*=\s*\"([^\"]+)\"""")
    .find(repositoryRoot.resolve("Cargo.toml").readText())
    ?.groupValues?.get(1)
    ?: error("workspace version is missing from Cargo.toml")

fun semverVersionCode(version: String): Int {
    val match = Regex("""^(\d+)\.(\d+)\.(\d+)(?:[-+][0-9A-Za-z.-]+)?$""").matchEntire(version)
        ?: error("Android version must start with major.minor.patch: $version")
    val (major, minor, patch) = match.destructured.toList().take(3).map(String::toLong)
    require(minor in 0..999 && patch in 0..999) { "minor and patch must fit three digits" }
    val code = major * 1_000_000 + minor * 1_000 + patch
    require(code in 1..2_100_000_000) { "semantic version is outside Android's version-code range" }
    return code.toInt()
}

// The visible Android version is the workspace/release-tag version. Only the
// monotonically increasing Play version code may be overridden by release CI.
val releaseVersionName = workspaceVersion
val versionCodeOverride = providers.environmentVariable("ANDROID_VERSION_CODE").orNull
val releaseVersionCode = versionCodeOverride?.toIntOrNull()
    ?: if (versionCodeOverride == null) semverVersionCode(releaseVersionName)
    else error("ANDROID_VERSION_CODE must be an integer")
require(releaseVersionCode in 1..2_100_000_000) { "ANDROID_VERSION_CODE must be a positive Play-compatible integer" }

tasks.register("printAndroidVersion") {
    group = "help"
    description = "Prints the resolved Android version name and code for release validation."
    doLast {
        println("versionName=$releaseVersionName")
        println("versionCode=$releaseVersionCode")
    }
}

android {
    namespace = "io.narl.protonstream"
    compileSdk = 36
    buildToolsVersion = "37.0.0"
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "io.narl.protonstream"
        minSdk = 31
        targetSdk = 36
        versionCode = releaseVersionCode
        versionName = releaseVersionName

        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
        externalNativeBuild {
            cmake {
                arguments += "-DPSTR_MPV_ROOT=${layout.buildDirectory.dir("generated/mpv").get().asFile.absolutePath}"
                arguments += "-DPSTR_RUST_JNI_ROOT=${layout.buildDirectory.dir("generated/jniLibs").get().asFile.absolutePath}"
                cppFlags += listOf("-std=c++20")
            }
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    sourceSets.named("main") {
        java.srcDir(layout.buildDirectory.dir("generated/source/uniffi"))
        jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
        jniLibs.srcDir(layout.buildDirectory.dir("generated/mpv/jniLibs"))
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    val releaseStore = System.getenv("ANDROID_RELEASE_STORE_FILE")
    if (releaseStore != null) {
        signingConfigs.create("release") {
            storeFile = file(releaseStore)
            storePassword = System.getenv("ANDROID_RELEASE_STORE_PASSWORD")
            keyAlias = System.getenv("ANDROID_RELEASE_KEY_ALIAS")
            keyPassword = System.getenv("ANDROID_RELEASE_KEY_PASSWORD")
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            if (releaseStore != null) signingConfig = signingConfigs.getByName("release")
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    packaging {
        jniLibs.useLegacyPackaging = false
        resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

val rustJniDir = layout.buildDirectory.dir("generated/jniLibs")
val uniffiSourceDir = layout.buildDirectory.dir("generated/source/uniffi")
val hostRustLibrary = repositoryRoot.resolve(
    "target/debug/${System.mapLibraryName("pstr_android")}",
)
val stagedMpvDir = layout.buildDirectory.dir("generated/mpv")

val buildLibmpvAndroid by tasks.registering(Exec::class) {
    group = "native"
    description = "Builds the pinned GPL libmpv and stages both Android ABIs."
    workingDir(repositoryRoot)
    commandLine("bash", repositoryRoot.resolve("scripts/build-libmpv-android.sh").absolutePath)
    inputs.file(repositoryRoot.resolve("scripts/build-libmpv-android.sh"))
    outputs.files(
        stagedMpvDir.map { it.file("jniLibs/arm64-v8a/libmpv.so") },
        stagedMpvDir.map { it.file("jniLibs/x86_64/libmpv.so") },
        stagedMpvDir.map { it.file("include/mpv/client.h") },
        stagedMpvDir.map { it.file("REVISION") },
    )
}

val buildRustHost by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds the host cdylib used by UniFFI binding generation."
    workingDir(repositoryRoot)
    commandLine("cargo", "build", "-p", "pstr-android", "--lib")
    inputs.files(fileTree(repositoryRoot.resolve("crates")) { include("**/*.rs", "**/Cargo.toml") })
    outputs.file(hostRustLibrary)
}

val generateUniFfiBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generates Kotlin bindings from the pstr-android cdylib."
    dependsOn(buildRustHost)
    workingDir(repositoryRoot)
    doFirst { uniffiSourceDir.get().asFile.mkdirs() }
    commandLine(
        "cargo", "run", "-p", "pstr-android", "--features", "bindgen", "--bin", "uniffi-bindgen", "--", "generate",
        "--library", hostRustLibrary.absolutePath,
        "--language", "kotlin",
        "--no-format",
        "--out-dir", uniffiSourceDir.get().asFile.absolutePath,
    )
    inputs.file(hostRustLibrary)
    outputs.dir(uniffiSourceDir)
}

val buildRustAndroid by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds pstr-android for every supported ABI with cargo-ndk."
    workingDir(repositoryRoot)
    doFirst { rustJniDir.get().asFile.mkdirs() }
    commandLine(
        "cargo", "ndk",
        "--target", "arm64-v8a",
        "--target", "x86_64",
        "--platform", "31",
        "--output-dir", rustJniDir.get().asFile.absolutePath,
        "build", "--release", "-p", "pstr-android", "--lib",
    )
    environment("CARGO_TARGET_DIR", repositoryRoot.resolve("target/android").absolutePath)
    inputs.files(fileTree(repositoryRoot.resolve("crates")) { include("**/*.rs", "**/Cargo.toml") })
    outputs.dir(rustJniDir)
}

tasks.named("preBuild") {
    dependsOn(generateUniFfiBindings, buildRustAndroid, buildLibmpvAndroid)
}

tasks.configureEach {
    if (name.contains("CMake", ignoreCase = true)) {
        dependsOn(buildRustAndroid, buildLibmpvAndroid)
    }
}

dependencies {
    implementation(libs.androidx.core)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.androidx.work.runtime)
    implementation(libs.jna) { artifact { type = "aar" } }


    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.runtime.livedata)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.androidx.compose.material3.adaptive)
    implementation(libs.androidx.compose.material3.navigation.suite)
    debugImplementation(libs.androidx.compose.ui.tooling)
    testImplementation(libs.junit)
}

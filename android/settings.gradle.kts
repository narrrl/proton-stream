import java.io.File

// Google's mirror of Maven Central, asked first. Central answers bursts from
// CI runners' shared addresses with 429, which failed builds at plugin
// resolution before a line of the app compiled. The mirror serves the same
// artifacts, and Gradle checks them against Central's checksums all the same.
val centralMirror = "https://maven-central.storage-download.googleapis.com/maven2/"

pluginManagement {
    repositories {
        google()
        maven("https://maven-central.storage-download.googleapis.com/maven2/") { name = "MavenCentralMirror" }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        maven(centralMirror) { name = "MavenCentralMirror" }
        mavenCentral()
    }
}

rootProject.name = "proton-stream-android"
include(":app")

-keep class uniffi.pstr_android.** { *; }
# rustls-platform-verifier loads these classes from Rust through JNI, so R8
# cannot infer that they are reachable from the Kotlin call graph.
-keep class org.rustls.platformverifier.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
-keepclasseswithmembernames class * {
    native <methods>;
}

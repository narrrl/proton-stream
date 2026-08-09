-keep class uniffi.pstr_android.** { *; }
# rustls-platform-verifier loads these classes from Rust through JNI, so R8
# cannot infer that they are reachable from the Kotlin call graph.
-keep class org.rustls.platformverifier.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
-keepclasseswithmembernames class * {
    native <methods>;
}

# UniFFI records are Kotlin data classes whose generic signatures and
# annotations survive into the runtime; stripping them changes how the bindings
# lift and lower structured types.
-keepattributes Signature,*Annotation*,InnerClasses,EnclosingMethod

# JNA dispatches into callbacks reflectively by method name.
-keepclassmembers class * implements com.sun.jna.Callback {
    <methods>;
}

# The bridge calls back into these from Rust. Nothing in the Kotlin call graph
# reaches their overrides, so R8 is free to rename or drop them.
-keep class io.narl.protonstream.native.KeystoreSecretStore { *; }
-keep class * implements uniffi.pstr_android.AndroidSecretStore { *; }
-keep class * implements uniffi.pstr_android.DownloadObserver { *; }

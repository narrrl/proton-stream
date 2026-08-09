# Applied only to the instrumentation APK when androidTest runs against a
# minified build (ANDROID_TEST_BUILD_TYPE=release). The tests exist to prove the
# *app's* R8 configuration is right, so nothing here may relax the app's rules —
# it only keeps the test harness itself intact.
-dontobfuscate
-keep class io.narl.protonstream.** { *; }
-keep class androidx.test.** { *; }
-dontwarn androidx.test.**
-keepclasseswithmembers class * {
    @org.junit.Test <methods>;
}

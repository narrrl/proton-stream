# Keeps that exist so the instrumentation suite can run against the *release*
# build. They are applied to the shipping APK, which is a real trade-off worth
# stating plainly: the tested artifact is not byte-identical to an artifact
# built without them.
#
# It is the better side of the trade. The alternative is to run instrumentation
# against `debug` only, which leaves R8 — the step that has already broken this
# app twice, in ways no debug run can reproduce — entirely uncovered. What is
# kept here is a few kilobytes of framework plumbing containing no application
# logic and revealing nothing about the app.
#
# Why these keeps are needed at all: the instrumentation APK is minified in a
# separate R8 run that consumes the app's mapping file, and links against the
# app's classes at runtime. That arrangement fails in two ways.
#
# 1. The mapping file carries renames but not R8's parameter-permutation
#    optimisation, so a permuted method resolves to the wrong descriptor from
#    the test APK.
# 2. A class that only the test APK reaches is unreachable from the app's call
#    graph, so R8 removes it from the app APK — and the test APK does not
#    bundle it, because it is supposed to come from the app.

# The Kotlin runtime is where both failures land, because androidx.test is
# itself Kotlin and shares the app's copy of the stdlib. Keeping it wholesale
# rather than class by class is deliberate: each individual keep only moves the
# crash to the next facade the runner happens to touch, which is a list nobody
# can enumerate ahead of time. Three were hit before this rule replaced them:
#
#   NoSuchMethodError: No static method f(Ljava/lang/Object;Ljava/lang/String;)V
#     in class Lo6/k;                     (o6.k = kotlin.jvm.internal.Intrinsics,
#                                          whose parameters R8 had permuted —
#                                          every Kotlin class calls it on entry,
#                                          so nothing ran at all)
#   NoClassDefFoundError: Lkotlin/LazyKt;  (androidx.test.platform.io.TestDirCalculator)
#
# Cost is roughly a megabyte of dex against a ~100 MB APK that is almost
# entirely native libraries.
-keep class kotlin.** { *; }

# `AndroidJUnitRunner.onCreate` traces its own startup. Nothing in the app
# reaches androidx.tracing, so R8 dropped it and the runner could not start:
#
#   java.lang.NoClassDefFoundError: Failed resolution of: Landroidx/tracing/Trace;
#   at androidx.test.runner.AndroidJUnitRunner.onCreate(AndroidJUnitRunner.java:307)
-keep class androidx.tracing.** { *; }

# A third shape of the same problem, and the one worth understanding: the app
# only ever touches WorkManager through its implementation, so R8 vertically
# merged the abstract `androidx.work.WorkManager` into `WorkManagerImpl` and the
# abstract type stopped existing. It is absent from mapping.txt entirely rather
# than renamed. The download tests declare fields of the public types, so JUnit's
# field scan could not even construct the test class:
#
#   java.lang.NoClassDefFoundError: Failed resolution of: Landroidx/work/WorkManager;
#   at org.junit.runners.model.TestClass.getSortedDeclaredFields(TestClass.java:77)
#
# Kept package-wide because the same merge can happen to any of the public API
# types the tests name (WorkInfo, Data, Constraints), and finding out one crash
# at a time costs a four-minute device round trip each.
-keep class androidx.work.** { *; }

# Everything above is framework plumbing. The last group is app code, and it is
# the one keep in this file that costs real fidelity: R8 no longer optimises
# these classes in the shipping build, so the suite no longer proves R8 handles
# them correctly. It is a narrow loss — the surfaces R8 has actually
# broken here are the FFI ones, and those are kept for functional reasons in
# proguard-rules.pro and stay under test.
#
# The trigger was `DownloadStateStore.clear()`. It is not dead code — AppViewModel
# calls it — but it has exactly one call site, so R8 inlined it and dropped the
# method. Only a caller that resolves it by name, which is to say a test, notices:
#
#   java.lang.NoSuchMethodError: No virtual method clear()V
#   in class Lio/narl/protonstream/download/c;
#
# Package-wide rather than class-by-class: Kotlin's top-level functions live in
# a synthesised `...Kt` facade that no source file names, so keeping the classes
# a test imports still leaves it missing `DownloadCoordinatorKt`.
-keep class io.narl.protonstream.download.** { *; }

# Same inlining, one layer out: the `wifiOnly` setter behind the download
# constraint.
#   java.lang.NoSuchMethodError: No virtual method setWifiOnly(Z)V in class Li5/a;
-keep class io.narl.protonstream.settings.SettingsStore { *; }

# The live cases drive the player directly, so the same inlining applies to the
# playback classes they name.
-keep class io.narl.protonstream.playback.** { *; }

# `runBlocking` — the live cases drive suspending bridge calls from a JUnit
# thread. The app itself never blocks a thread on a coroutine, so R8 drops the
# facade that holds it:
#
#   java.lang.NoClassDefFoundError: Failed resolution of: Lkotlinx/coroutines/BuildersKt;
#
# Kept narrowly rather than as `kotlinx.coroutines.**`: that library is large and
# on the app's hot paths, and pinning all of it would cost real optimisation.
-keep class kotlinx.coroutines.BuildersKt** { *; }

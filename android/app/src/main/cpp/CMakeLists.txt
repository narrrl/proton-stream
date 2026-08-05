cmake_minimum_required(VERSION 3.22.1)
project(pstr_mpv LANGUAGES CXX)

if(NOT DEFINED PSTR_MPV_ROOT)
    message(FATAL_ERROR "PSTR_MPV_ROOT must point at the staged Android libmpv tree")
endif()
if(NOT DEFINED PSTR_RUST_JNI_ROOT)
    message(FATAL_ERROR "PSTR_RUST_JNI_ROOT must point at cargo-ndk output")
endif()

add_library(pstr_mpv SHARED pstr_mpv.cpp)
add_library(mpv SHARED IMPORTED)
set_target_properties(mpv PROPERTIES
    IMPORTED_LOCATION "${PSTR_MPV_ROOT}/jniLibs/${ANDROID_ABI}/libmpv.so")

target_compile_features(pstr_mpv PRIVATE cxx_std_20)
target_compile_options(pstr_mpv PRIVATE -Wall -Wextra -Werror)
target_include_directories(pstr_mpv PRIVATE "${PSTR_MPV_ROOT}/include")
# Link by soname rather than by absolute build path. Both libraries are
# packaged into the same ABI directory in the APK.
target_link_directories(pstr_mpv PRIVATE "${PSTR_RUST_JNI_ROOT}/${ANDROID_ABI}")
target_link_options(pstr_mpv PRIVATE "-Wl,-l:libpstr_android.so")
target_link_libraries(pstr_mpv PRIVATE mpv android EGL log)

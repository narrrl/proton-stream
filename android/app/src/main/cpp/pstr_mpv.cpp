#include "pstr_mpv.h"

#include <EGL/egl.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <jni.h>
#include <mpv/client.h>
#include <mpv/render_gl.h>
#include <mpv/stream_cb.h>

#include <atomic>
#include <algorithm>
#include <charconv>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <cstdint>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <thread>
#include <utility>

namespace {

constexpr const char *kProtocol = "pstr";

struct StreamCookie {
    uint64_t handle;
    int64_t size;
    int64_t position = 0;
    std::atomic<bool> cancelled{false};
};

template <typename Result, typename Function>
Result c_boundary(Result failure, Function &&function) noexcept {
    try {
        return std::forward<Function>(function)();
    } catch (...) {
        return failure;
    }
}

template <typename Function>
void c_boundary(Function &&function) noexcept {
    try {
        std::forward<Function>(function)();
    } catch (...) {
        // C and JNI callers cannot receive C++ exceptions.
    }
}

int64_t stream_read(void *opaque, char *buffer, uint64_t length) noexcept {
    return c_boundary<int64_t>(-1, [=] {
        auto *stream = static_cast<StreamCookie *>(opaque);
        if (!stream || !buffer) return int64_t{-1};
        if (stream->cancelled.load(std::memory_order_relaxed)) return int64_t{-1};
        const auto remaining = stream->size - stream->position;
        if (remaining <= 0) return int64_t{0};
        const auto requested = static_cast<size_t>(std::min<uint64_t>(length, remaining));
        const int64_t read = pstr_android_stream_read(
            stream->handle, static_cast<uint64_t>(stream->position), buffer, requested);
        if (read > 0) stream->position += read;
        return read;
    });
}

int64_t stream_seek(void *opaque, int64_t offset) noexcept {
    return c_boundary<int64_t>(MPV_ERROR_GENERIC, [=] {
        auto *stream = static_cast<StreamCookie *>(opaque);
        if (!stream || offset < 0 || offset > stream->size) {
            return int64_t{MPV_ERROR_GENERIC};
        }
        stream->position = offset;
        stream->cancelled.store(false, std::memory_order_relaxed);
        return offset;
    });
}

int64_t stream_size(void *opaque) noexcept {
    return c_boundary<int64_t>(MPV_ERROR_GENERIC, [=] {
        auto *stream = static_cast<StreamCookie *>(opaque);
        return stream ? stream->size : int64_t{MPV_ERROR_GENERIC};
    });
}

void stream_cancel(void *opaque) noexcept {
    c_boundary([=] {
        if (auto *stream = static_cast<StreamCookie *>(opaque)) {
            stream->cancelled.store(true, std::memory_order_relaxed);
        }
    });
}

void stream_close(void *opaque) noexcept {
    c_boundary([=] { delete static_cast<StreamCookie *>(opaque); });
}

int stream_open(void *, char *uri, mpv_stream_cb_info *info) noexcept {
    return c_boundary<int>(MPV_ERROR_LOADING_FAILED, [=]() -> int {
        if (!info) return MPV_ERROR_LOADING_FAILED;
        constexpr std::string_view prefix = "pstr://";
        const std::string_view value(uri == nullptr ? "" : uri);
        if (!value.starts_with(prefix)) return MPV_ERROR_LOADING_FAILED;
        uint64_t handle = 0;
        const auto token = value.substr(prefix.size());
        const auto result = std::from_chars(token.data(), token.data() + token.size(), handle);
        if (result.ec != std::errc{} || result.ptr != token.data() + token.size()) {
            return MPV_ERROR_LOADING_FAILED;
        }
        const int64_t size = pstr_android_stream_size(handle);
        if (size < 0) return MPV_ERROR_LOADING_FAILED;
        std::unique_ptr<StreamCookie> cookie(new StreamCookie{handle, size});
        info->cookie = cookie.get();
        info->read_fn = stream_read;
        info->seek_fn = stream_seek;
        info->size_fn = stream_size;
        info->close_fn = stream_close;
        info->cancel_fn = stream_cancel;
        cookie.release();
        return 0;
    });
}

void *resolve_gl(void *, const char *name) noexcept {
    return c_boundary<void *>(nullptr, [=] {
        return reinterpret_cast<void *>(eglGetProcAddress(name));
    });
}

struct PlaybackState {
    double position = 0.0;
    double duration = 0.0;
    double volume = 100.0;
    bool paused = true;
    bool muted = false;
    bool ended = false;
};

class Player {
  public:
    Player() {
        mpv_ = mpv_create();
        if (!mpv_) return;
        option("config", "no");
        option("vo", "libmpv");
        option("force-window", "no");
        option("video-timing-offset", "0");
        option("hwdec", "auto-safe");
        option("cache", "yes");
        option("cache-secs", "30");
        option("demuxer-readahead-secs", "30");
        option("demuxer-max-bytes", "50331648");
        option("audio-focus", "no"); // Android AudioManager owns focus.
        if (mpv_stream_cb_add_ro(mpv_, kProtocol, nullptr, stream_open) < 0 ||
            mpv_initialize(mpv_) < 0) {
            mpv_terminate_destroy(mpv_);
            mpv_ = nullptr;
            return;
        }
        observe("time-pos", 1, MPV_FORMAT_DOUBLE);
        observe("duration", 2, MPV_FORMAT_DOUBLE);
        observe("pause", 3, MPV_FORMAT_FLAG);
        observe("volume", 4, MPV_FORMAT_DOUBLE);
        observe("mute", 5, MPV_FORMAT_FLAG);
        events_ = std::thread([this] { event_loop(); });
        renderer_ = std::thread([this] { render_loop(); });
    }

    ~Player() noexcept {
        // Destruction runs from JNI. Keep failures from std::thread primitives
        // inside C++, even on runtimes that report a join error.
        c_boundary([this] {
            running_.store(false);
            render_cv_.notify_all();
            if (mpv_) mpv_wakeup(mpv_);
        });
        c_boundary([this] { if (events_.joinable()) events_.join(); });
        c_boundary([this] { if (renderer_.joinable()) renderer_.join(); });
        c_boundary([this] { if (mpv_) mpv_terminate_destroy(mpv_); });
    }

    bool valid() const { return mpv_ != nullptr; }

    bool attach(JNIEnv *env, jobject surface) {
        if (!env || !surface) return false;
        ANativeWindow *window = ANativeWindow_fromSurface(env, surface);
        if (!window) return false;
        {
            std::lock_guard lock(render_mutex_);
            if (pending_window_) ANativeWindow_release(pending_window_);
            pending_window_ = window;
            surface_changed_ = true;
        }
        render_cv_.notify_one();
        return true;
    }

    void detach() {
        {
            std::lock_guard lock(render_mutex_);
            if (pending_window_) {
                ANativeWindow_release(pending_window_);
                pending_window_ = nullptr;
            }
            surface_changed_ = true;
        }
        render_cv_.notify_one();
    }

    bool load(uint64_t handle, double start, const std::string &audio,
              const std::string &subtitle, bool subtitles) {
        if (!mpv_) return false;
        {
            std::unique_lock lock(render_mutex_);
            if (!render_cv_.wait_for(lock, std::chrono::seconds(5), [this] { return render_ != nullptr || !running_.load(); })) return false;
        }
        if (!audio.empty()) set_string("alang", audio);
        if (!subtitle.empty()) set_string("slang", subtitle);
        set_string("sid", subtitles ? "auto" : "no");
        pending_start_.store(std::max(0.0, start));
        const std::string url = "pstr://" + std::to_string(handle);
        const char *command[] = {"loadfile", url.c_str(), nullptr};
        if (mpv_command(mpv_, command) < 0) return false;
        return true;
    }

    void pause(bool value) { set_flag("pause", value); }
    void seek(double seconds) { set_double("time-pos", std::max(0.0, seconds)); }
    void volume(double value) { set_double("volume", std::clamp(value, 0.0, 100.0)); }
    void mute(bool value) { set_flag("mute", value); }
    void select_track(bool audio, int64_t id) {
        const char *property = audio ? "aid" : "sid";
        if (id <= 0) set_string(property, "no"); else mpv_set_property(mpv_, property, MPV_FORMAT_INT64, &id);
    }
    void stop() {
        if (!mpv_) return;
        const char *command[] = {"stop", nullptr};
        mpv_command(mpv_, command);
    }

    PlaybackState state() const {
        std::lock_guard lock(state_mutex_);
        return state_;
    }

    std::string tracks_json() const {
        if (!mpv_) return "[]";
        int64_t count = 0;
        if (mpv_get_property(mpv_, "track-list/count", MPV_FORMAT_INT64, &count) < 0) return "[]";
        std::string json = "[";
        for (int64_t i = 0; i < count; ++i) {
            const std::string base = "track-list/" + std::to_string(i) + "/";
            int64_t id = 0;
            char *type_raw = nullptr;
            char *lang_raw = nullptr;
            char *title_raw = nullptr;
            int selected = 0;
            mpv_get_property(mpv_, (base + "id").c_str(), MPV_FORMAT_INT64, &id);
            mpv_get_property(mpv_, (base + "type").c_str(), MPV_FORMAT_STRING, &type_raw);
            MpvString type(type_raw);
            mpv_get_property(mpv_, (base + "lang").c_str(), MPV_FORMAT_STRING, &lang_raw);
            MpvString lang(lang_raw);
            mpv_get_property(mpv_, (base + "title").c_str(), MPV_FORMAT_STRING, &title_raw);
            MpvString title(title_raw);
            mpv_get_property(mpv_, (base + "selected").c_str(), MPV_FORMAT_FLAG, &selected);
            if (i) json += ',';
            json += "{\"id\":" + std::to_string(id) + ",\"type\":\"" + escape(type.get()) +
                    "\",\"language\":\"" + escape(lang.get()) + "\",\"title\":\"" +
                    escape(title.get()) + "\",\"selected\":" + (selected ? "true" : "false") + "}";
        }
        return json + ']';
    }

  private:
    struct MpvFree {
        void operator()(char *value) const noexcept { if (value) mpv_free(value); }
    };
    using MpvString = std::unique_ptr<char, MpvFree>;

    static std::string escape(const char *text) {
        std::string out;
        if (!text) return out;
        for (const unsigned char c : std::string_view(text)) {
            if (c == '"' || c == '\\') out += '\\';
            if (c >= 0x20) out += static_cast<char>(c);
        }
        return out;
    }

    void option(const char *name, const char *value) { mpv_set_option_string(mpv_, name, value); }
    void observe(const char *name, uint64_t id, mpv_format format) { mpv_observe_property(mpv_, id, name, format); }
    void set_string(const char *name, const std::string &value) { mpv_set_property_string(mpv_, name, value.c_str()); }
    void set_double(const char *name, double value) { if (mpv_) mpv_set_property(mpv_, name, MPV_FORMAT_DOUBLE, &value); }
    void set_flag(const char *name, bool value) { int flag = value; if (mpv_) mpv_set_property(mpv_, name, MPV_FORMAT_FLAG, &flag); }

    void event_loop() {
        while (running_.load()) {
            mpv_event *event = mpv_wait_event(mpv_, 0.25);
            if (event->event_id == MPV_EVENT_NONE) continue;
            if (event->event_id == MPV_EVENT_SHUTDOWN) break;
            if (event->event_id == MPV_EVENT_FILE_LOADED) {
                const double start = pending_start_.exchange(0.0);
                if (start > 0.0) set_double("time-pos", start);
            }
            if (event->event_id == MPV_EVENT_END_FILE) {
                std::lock_guard lock(state_mutex_);
                state_.ended = true;
            } else if (event->event_id == MPV_EVENT_START_FILE) {
                std::lock_guard lock(state_mutex_);
                state_.ended = false;
            } else if (event->event_id == MPV_EVENT_PROPERTY_CHANGE) {
                update_property(*static_cast<mpv_event_property *>(event->data));
            }
        }
    }

    void update_property(const mpv_event_property &property) {
        if (!property.data) return;
        std::lock_guard lock(state_mutex_);
        if (!std::strcmp(property.name, "time-pos")) state_.position = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "duration")) state_.duration = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "volume")) state_.volume = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "pause")) state_.paused = *static_cast<int *>(property.data);
        else if (!std::strcmp(property.name, "mute")) state_.muted = *static_cast<int *>(property.data);
    }

    static void render_update(void *opaque) noexcept {
        c_boundary([=] {
            auto *self = static_cast<Player *>(opaque);
            if (!self) return;
            self->frame_ready_.store(true);
            self->render_cv_.notify_one();
        });
    }

    void render_loop() {
        EGLDisplay display = EGL_NO_DISPLAY;
        EGLContext context = EGL_NO_CONTEXT;
        EGLSurface surface = EGL_NO_SURFACE;
        EGLSurface pbuffer = EGL_NO_SURFACE;
        EGLConfig config = nullptr;
        ANativeWindow *window = nullptr;
        while (running_.load()) {
            std::unique_lock lock(render_mutex_);
            render_cv_.wait(lock, [this] { return !running_.load() || surface_changed_ || frame_ready_.load(); });
            if (!running_.load()) break;
            if (surface_changed_) {
                surface_changed_ = false;
                if (surface != EGL_NO_SURFACE) { eglMakeCurrent(display, pbuffer, pbuffer, context); eglDestroySurface(display, surface); surface = EGL_NO_SURFACE; }
                if (window) ANativeWindow_release(window);
                window = pending_window_;
                pending_window_ = nullptr;
                if (window && display == EGL_NO_DISPLAY && !initialize_egl(display, context, config, pbuffer)) {
                    ANativeWindow_release(window);
                    window = nullptr;
                }
                if (window) {
                    surface = eglCreateWindowSurface(display, config, window, nullptr);
                    eglMakeCurrent(display, surface, surface, context);
                    initialize_renderer();
                }
            }
            const bool draw = frame_ready_.exchange(false);
            lock.unlock();
            if (draw && surface != EGL_NO_SURFACE && render_) {
                eglMakeCurrent(display, surface, surface, context);
                if (mpv_render_context_update(render_) & MPV_RENDER_UPDATE_FRAME) {
                    EGLint width = 0, height = 0;
                    eglQuerySurface(display, surface, EGL_WIDTH, &width);
                    eglQuerySurface(display, surface, EGL_HEIGHT, &height);
                    mpv_opengl_fbo fbo{0, width, height, 0};
                    int flip = 1;
                    mpv_render_param params[] = {{MPV_RENDER_PARAM_OPENGL_FBO, &fbo}, {MPV_RENDER_PARAM_FLIP_Y, &flip}, {MPV_RENDER_PARAM_INVALID, nullptr}};
                    mpv_render_context_render(render_, params);
                    eglSwapBuffers(display, surface);
                }
            }
        }
        if (render_) {
            eglMakeCurrent(display, surface != EGL_NO_SURFACE ? surface : pbuffer, surface != EGL_NO_SURFACE ? surface : pbuffer, context);
            mpv_render_context_set_update_callback(render_, nullptr, nullptr);
            mpv_render_context_free(render_);
            render_ = nullptr;
        }
        if (surface != EGL_NO_SURFACE) eglDestroySurface(display, surface);
        if (pbuffer != EGL_NO_SURFACE) eglDestroySurface(display, pbuffer);
        if (context != EGL_NO_CONTEXT) eglDestroyContext(display, context);
        if (display != EGL_NO_DISPLAY) eglTerminate(display);
        if (window) ANativeWindow_release(window);
        std::lock_guard lock(render_mutex_);
        if (pending_window_) { ANativeWindow_release(pending_window_); pending_window_ = nullptr; }
    }

    bool initialize_egl(EGLDisplay &display, EGLContext &context, EGLConfig &config, EGLSurface &pbuffer) {
        display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        if (display == EGL_NO_DISPLAY || !eglInitialize(display, nullptr, nullptr)) return false;
        const EGLint attributes[] = {EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, EGL_WINDOW_BIT | EGL_PBUFFER_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE};
        EGLint count = 0;
        if (!eglChooseConfig(display, attributes, &config, 1, &count) || count == 0) return false;
        const EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
        context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
        if (context == EGL_NO_CONTEXT) return false;
        const EGLint pbuffer_attributes[] = {EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE};
        pbuffer = eglCreatePbufferSurface(display, config, pbuffer_attributes);
        return pbuffer != EGL_NO_SURFACE && eglMakeCurrent(display, pbuffer, pbuffer, context);
    }

    void initialize_renderer() {
        if (render_ || !mpv_) return;
        mpv_opengl_init_params gl{resolve_gl, nullptr};
        const char *api = MPV_RENDER_API_TYPE_OPENGL;
        mpv_render_param params[] = {{MPV_RENDER_PARAM_API_TYPE, const_cast<char *>(api)}, {MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &gl}, {MPV_RENDER_PARAM_INVALID, nullptr}};
        if (mpv_render_context_create(&render_, mpv_, params) >= 0) {
            mpv_render_context_set_update_callback(render_, render_update, this);
            render_cv_.notify_all();
        }
    }

    mpv_handle *mpv_ = nullptr;
    mpv_render_context *render_ = nullptr;
    std::atomic<bool> running_{true};
    std::thread events_;
    std::thread renderer_;
    mutable std::mutex state_mutex_;
    PlaybackState state_;
    std::mutex render_mutex_;
    std::condition_variable render_cv_;
    ANativeWindow *pending_window_ = nullptr;
    bool surface_changed_ = false;
    std::atomic<bool> frame_ready_{false};
    std::atomic<double> pending_start_{0.0};
};

Player *from(jlong handle) { return reinterpret_cast<Player *>(handle); }
jstring string(JNIEnv *env, const std::string &value) { return env->NewStringUTF(value.c_str()); }

class UtfChars {
  public:
    UtfChars(JNIEnv *env, jstring value) noexcept
        : env_(env), value_(value), characters_(env->GetStringUTFChars(value, nullptr)) {}
    ~UtfChars() noexcept {
        if (characters_) env_->ReleaseStringUTFChars(value_, characters_);
    }
    UtfChars(const UtfChars &) = delete;
    UtfChars &operator=(const UtfChars &) = delete;
    const char *get() const noexcept { return characters_; }

  private:
    JNIEnv *env_;
    jstring value_;
    const char *characters_;
};

bool utf8(JNIEnv *env, jstring value, std::string &result) {
    if (!value) return true;
    UtfChars characters(env, value);
    if (!characters.get()) return false; // An exception, normally OOM, is pending.
    result.assign(characters.get());
    return !env->ExceptionCheck();
}

void reject_null_surface(JNIEnv *env) noexcept {
    c_boundary([=] {
        jclass type = env->FindClass("java/lang/IllegalArgumentException");
        if (type) env->ThrowNew(type, "surface must not be null");
    });
}

} // namespace

extern "C" JNIEXPORT jlong JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeCreate(JNIEnv *, jobject) noexcept {
    return c_boundary<jlong>(0, [] {
        auto player = std::make_unique<Player>();
        return player->valid() ? reinterpret_cast<jlong>(player.release()) : 0;
    });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeDestroy(JNIEnv *, jobject, jlong handle) noexcept {
    c_boundary([=] { delete from(handle); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeAttachSurface(JNIEnv *env, jobject, jlong handle, jobject surface) noexcept {
    c_boundary([=] {
        if (!surface) { reject_null_surface(env); return; }
        if (auto *p = from(handle)) p->attach(env, surface);
    });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeDetachSurface(JNIEnv *, jobject, jlong handle) noexcept {
    c_boundary([=] { if (auto *p = from(handle)) p->detach(); });
}
extern "C" JNIEXPORT jboolean JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeLoad(JNIEnv *env, jobject, jlong handle, jlong stream, jdouble start, jstring audio, jstring subtitle, jboolean subtitles) noexcept {
    return c_boundary<jboolean>(JNI_FALSE, [=] {
        std::string audio_text;
        std::string subtitle_text;
        if (!utf8(env, audio, audio_text) || !utf8(env, subtitle, subtitle_text)) {
            return jboolean{JNI_FALSE};
        }
        auto *p = from(handle);
        return static_cast<jboolean>(
            p && p->load(static_cast<uint64_t>(stream), start, audio_text,
                         subtitle_text, subtitles == JNI_TRUE)
                ? JNI_TRUE
                : JNI_FALSE);
    });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativePause(JNIEnv *, jobject, jlong h, jboolean value) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->pause(value == JNI_TRUE); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeSeek(JNIEnv *, jobject, jlong h, jdouble value) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->seek(value); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeVolume(JNIEnv *, jobject, jlong h, jdouble value) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->volume(value); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeMute(JNIEnv *, jobject, jlong h, jboolean value) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->mute(value == JNI_TRUE); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeSelectTrack(JNIEnv *, jobject, jlong h, jboolean audio, jlong id) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->select_track(audio == JNI_TRUE, id); });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeStop(JNIEnv *, jobject, jlong h) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->stop(); });
}
extern "C" JNIEXPORT jdoubleArray JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeState(JNIEnv *env, jobject, jlong h) noexcept {
    return c_boundary<jdoubleArray>(nullptr, [=] {
        const auto state = from(h) ? from(h)->state() : PlaybackState{};
        const jdouble values[] = {state.position, state.duration, state.volume,
                                  state.paused ? 1.0 : 0.0, state.muted ? 1.0 : 0.0,
                                  state.ended ? 1.0 : 0.0};
        jdoubleArray result = env->NewDoubleArray(6);
        if (!result) return static_cast<jdoubleArray>(nullptr);
        env->SetDoubleArrayRegion(result, 0, 6, values);
        if (env->ExceptionCheck()) return static_cast<jdoubleArray>(nullptr);
        return result;
    });
}
extern "C" JNIEXPORT jstring JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeTracks(JNIEnv *env, jobject, jlong h) noexcept {
    return c_boundary<jstring>(nullptr, [=] {
        return string(env, from(h) ? from(h)->tracks_json() : "[]");
    });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_pstrAndroidStreamRelease(JNIEnv *, jobject, jlong handle) noexcept {
    c_boundary([=] { pstr_android_stream_release(static_cast<uint64_t>(handle)); });
}

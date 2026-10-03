#include "pstr_mpv.h"

#include <EGL/egl.h>
#include <android/log.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>
#include <jni.h>
#include <mpv/client.h>
#include <mpv/render_gl.h>
#include <mpv/stream_cb.h>

extern "C" {
#include <libavcodec/jni.h>
}

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
/// mpv's `hwdec` when hardware decoding is on. Why this value: `Player()`.
constexpr const char *kHardwareDecoding = "auto-copy-safe";

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

/// mpv wants its request slot back — a seek, or a teardown.
///
/// The flag alone only stops the *next* read; the one already parked in a 4 MiB
/// block fetch has to be interrupted in Rust, or `mpv_terminate_destroy` waits
/// out the whole fetch while joining the demuxer thread.
void stream_cancel(void *opaque) noexcept {
    c_boundary([=] {
        if (auto *stream = static_cast<StreamCookie *>(opaque)) {
            stream->cancelled.store(true, std::memory_order_relaxed);
            pstr_android_stream_cancel(stream->handle);
        }
    });
}

/// Releases the Rust stream behind the file mpv has finished with.
///
/// Ownership sits here rather than in Kotlin because this is the one moment
/// that is *defined* to be after mpv's last read: `loadfile` is asynchronous, so
/// a Kotlin-side release issued when the next load returns can land while the
/// outgoing demuxer is still reading — and, if the release names the handle the
/// registry has already swapped, takes the incoming episode's stream with it.
void stream_close(void *opaque) noexcept {
    c_boundary([=] {
        std::unique_ptr<StreamCookie> cookie(static_cast<StreamCookie *>(opaque));
        if (cookie) pstr_android_stream_release(cookie->handle);
    });
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

/// Why the open file ended, mirroring `pstr_player::EndReason` so that the two
/// clients agree on what "finished" means. Only [Eof] is an episode the viewer
/// watched to the end; the rest are a stop, a shutdown or a failure, and none of
/// them may mark anything watched or advance to the next episode.
enum class EndReason : int {
    None = 0,
    Eof = 1,
    Stopped = 2,
    Quit = 3,
    Failed = 4,
    Other = 5,
};

EndReason end_reason_of(int reason) {
    switch (reason) {
        case MPV_END_FILE_REASON_EOF: return EndReason::Eof;
        case MPV_END_FILE_REASON_STOP: return EndReason::Stopped;
        case MPV_END_FILE_REASON_QUIT: return EndReason::Quit;
        case MPV_END_FILE_REASON_ERROR: return EndReason::Failed;
        default: return EndReason::Other;
    }
}

struct PlaybackState {
    double position = 0.0;
    double duration = 0.0;
    double volume = 100.0;
    bool paused = true;
    bool muted = false;
    bool ended = false;
    EndReason end_reason = EndReason::None;
    /// mpv has run out of buffered data and stopped to refill. Distinct from
    /// [paused], which is the viewer's doing: a frozen picture that nothing
    /// explains is indistinguishable from a hung app.
    bool buffering = false;
    /// How full the demuxer cache is while [buffering], 0–100.
    double cache_percent = 0.0;
    /// Between a seek being issued and playback actually resuming. The seek bar
    /// has moved but the picture has not.
    bool seeking = false;
    /// True between `loadfile` being issued and mpv acknowledging it with
    /// `START_FILE`. Everything mpv reports in that window still describes the
    /// *outgoing* file — including the `END_FILE(STOP)` that stopping it emits —
    /// and must not be attributed to the file being loaded.
    bool loading = false;
    /// The picture's display size, once mpv has decoded enough to know it.
    /// Zero until then, and the aspect Picture-in-Picture is given.
    double video_width = 0.0;
    double video_height = 0.0;
    /// Which loaded file this state describes. Bumped by every load, so a
    /// reader can tell "episode two, second zero" from a leftover reading of
    /// episode one — which is the difference between resuming an episode where
    /// the viewer left it and resuming it where the *previous* one ended.
    int64_t generation = 0;
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
        // The default; `load` sets it per file from the viewer's setting.
        // `-copy-safe`, not `-safe`. The direct MediaCodec decoders render into
        // an `ANativeWindow` the decoder is handed at init, and this player has
        // none to give: the picture goes through `vo_libmpv` and an EGL pbuffer,
        // and the SurfaceView — when there is one at all — attaches later than
        // the load. `auto-safe` picks them anyway and every file opens with
        // `hevc_mediacodec: Both surface and native_window are NULL` before
        // falling back. The copy variants decode in hardware and hand back
        // ordinary frames, which is what this render path can actually use.
        option("hwdec", kHardwareDecoding);
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
        observe("video-params/dw", 6, MPV_FORMAT_INT64);
        observe("video-params/dh", 7, MPV_FORMAT_INT64);
        observe("paused-for-cache", 8, MPV_FORMAT_FLAG);
        observe("cache-buffering-state", 9, MPV_FORMAT_INT64);
        // Errors mpv reports about the file itself, rather than about the
        // request that opened it. Without these a failed demux is silent.
        mpv_request_log_messages(mpv_, "error");
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
              const std::string &subtitle, bool subtitles,
              bool hardware_decoding) {
        if (!mpv_) return false;
        // Per file, before `loadfile`: a device whose decoder shows green
        // frames is fixed from the next episode without restarting the core.
        set_string("hwdec", hardware_decoding ? kHardwareDecoding : "no");
        // No wait for a render context. It is created with the render thread and
        // needs no window, and a load that cannot get one plays audio anyway —
        // waiting for a surface here is what made background playback fail after
        // a five-second stall.
        if (!audio.empty()) set_string("alang", audio);
        if (!subtitle.empty()) set_string("slang", subtitle);
        set_string("sid", subtitles ? "auto" : "no");
        pending_start_.store(std::max(0.0, start));
        // Retire the outgoing file's position, duration and end flag *before*
        // the new one loads. mpv only republishes them once it has demuxed
        // enough of the stream, and until then every reader would otherwise see
        // the last episode's clock under the new episode's name.
        {
            std::lock_guard lock(state_mutex_);
            const double volume = state_.volume;
            const bool muted = state_.muted;
            // `pause` is a property of the core, not of the file: mpv carries it
            // across `loadfile` unchanged and therefore never re-publishes it.
            // Resetting it to the struct's default would leave every reader
            // believing a playing file is paused for as long as it is open, and
            // every "play" they then send is a no-op against a core that never
            // paused — which is a play/pause button that does nothing.
            const bool paused = state_.paused;
            const int64_t generation = state_.generation + 1;
            state_ = PlaybackState{};
            state_.volume = volume;
            state_.muted = muted;
            state_.paused = paused;
            state_.generation = generation;
            state_.loading = true;
            error_.clear();
        }
        const std::string url = "pstr://" + std::to_string(handle);
        const char *command[] = {"loadfile", url.c_str(), nullptr};
        if (mpv_command(mpv_, command) < 0) {
            // Nothing was issued, so no START_FILE will arrive to clear this.
            std::lock_guard lock(state_mutex_);
            state_.loading = false;
            return false;
        }
        return true;
    }

    /// Written through rather than left to the observer: the transport is drawn
    /// from this state, and a tap that only shows its effect on mpv's next
    /// property event reads as a button that missed.
    void pause(bool value) {
        set_flag("pause", value);
        std::lock_guard lock(state_mutex_);
        state_.paused = value;
    }
    void seek(double seconds) { set_double("time-pos", std::max(0.0, seconds)); }
    void volume(double value) { set_double("volume", std::clamp(value, 0.0, 100.0)); }
    void speed(double value) { set_double("speed", std::clamp(value, 0.25, 4.0)); }
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

    /// The pending error, if any, consumed by reading it.
    std::string take_error() {
        std::lock_guard lock(state_mutex_);
        return std::exchange(error_, std::string());
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

    /// The file's chapters, read one sub-property at a time like the tracks.
    /// What they *mean* is decided in Rust (`pstr_core::chapters`), so the
    /// desktop and this client offer the same skips.
    std::string chapters_json() const {
        if (!mpv_) return "[]";
        int64_t count = 0;
        if (mpv_get_property(mpv_, "chapter-list/count", MPV_FORMAT_INT64, &count) < 0) return "[]";
        std::string json = "[";
        for (int64_t i = 0; i < count; ++i) {
            const std::string base = "chapter-list/" + std::to_string(i) + "/";
            char *title_raw = nullptr;
            double start = 0.0;
            mpv_get_property(mpv_, (base + "title").c_str(), MPV_FORMAT_STRING, &title_raw);
            MpvString title(title_raw);
            mpv_get_property(mpv_, (base + "time").c_str(), MPV_FORMAT_DOUBLE, &start);
            if (i) json += ',';
            json += "{\"index\":" + std::to_string(i) + ",\"title\":\"" + escape(title.get()) +
                    "\",\"start\":" + std::to_string(start) + '}';
        }
        return json + ']';
    }

  private:
    struct MpvFree {
        void operator()(char *value) const noexcept { if (value) mpv_free(value); }
    };
    using MpvString = std::unique_ptr<char, MpvFree>;

    /// JSON-escape `text`, and re-encode it as Modified UTF-8.
    ///
    /// The result of this ends up in `NewStringUTF`, which does not take UTF-8:
    /// it takes Modified UTF-8, where a character outside the basic plane is a
    /// surrogate *pair* of three-byte sequences rather than one four-byte
    /// sequence. Handing it real UTF-8 renders an emoji in a track title as
    /// mojibake, and aborts the process under CheckJNI.
    ///
    /// Anything that is not well-formed UTF-8 is dropped rather than passed
    /// through, for the same reason: a truncated sequence is what CheckJNI
    /// rejects, and a track title is not worth a crash.
    static std::string escape(const char *text) {
        std::string out;
        if (!text) return out;
        const std::string_view input(text);
        for (size_t i = 0; i < input.size();) {
            const auto lead = static_cast<unsigned char>(input[i]);
            size_t length = 0;
            uint32_t code = 0;
            if (lead < 0x80) {
                length = 1;
                code = lead;
            } else if ((lead & 0xE0) == 0xC0) {
                length = 2;
                code = lead & 0x1Fu;
            } else if ((lead & 0xF0) == 0xE0) {
                length = 3;
                code = lead & 0x0Fu;
            } else if ((lead & 0xF8) == 0xF0) {
                length = 4;
                code = lead & 0x07u;
            } else {
                ++i; // A stray continuation or invalid lead byte.
                continue;
            }
            if (i + length > input.size()) break;
            bool valid = true;
            for (size_t k = 1; k < length; ++k) {
                const auto next = static_cast<unsigned char>(input[i + k]);
                if ((next & 0xC0) != 0x80) {
                    valid = false;
                    break;
                }
                code = (code << 6) | (next & 0x3Fu);
            }
            if (!valid) {
                ++i;
                continue;
            }
            i += length;

            if (code < 0x20) continue; // Control characters, including NUL.
            if (code == '"' || code == '\\') {
                out += '\\';
                out += static_cast<char>(code);
                continue;
            }
            if (code < 0x80) {
                out += static_cast<char>(code);
            } else if (code < 0x800) {
                out += static_cast<char>(0xC0 | (code >> 6));
                out += static_cast<char>(0x80 | (code & 0x3F));
            } else if (code < 0x10000) {
                append_three(out, code);
            } else {
                // The surrogate pair Modified UTF-8 wants, each half written as
                // its own three-byte sequence.
                const uint32_t rest = code - 0x10000;
                append_three(out, 0xD800 + (rest >> 10));
                append_three(out, 0xDC00 + (rest & 0x3FF));
            }
        }
        return out;
    }

    static void append_three(std::string &out, uint32_t code) {
        out += static_cast<char>(0xE0 | (code >> 12));
        out += static_cast<char>(0x80 | ((code >> 6) & 0x3F));
        out += static_cast<char>(0x80 | (code & 0x3F));
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
                const auto *end = static_cast<mpv_event_end_file *>(event->data);
                std::lock_guard lock(state_mutex_);
                // The end of the file `loadfile` is replacing, not of the one it
                // is loading. Reporting it would hand every reader an episode
                // that ended before it started.
                if (state_.loading) continue;
                state_.ended = true;
                state_.end_reason = end ? end_reason_of(end->reason) : EndReason::Other;
            } else if (event->event_id == MPV_EVENT_START_FILE) {
                std::lock_guard lock(state_mutex_);
                state_.loading = false;
                state_.ended = false;
                state_.end_reason = EndReason::None;
            } else if (event->event_id == MPV_EVENT_SEEK) {
                std::lock_guard lock(state_mutex_);
                state_.seeking = true;
            } else if (event->event_id == MPV_EVENT_PLAYBACK_RESTART) {
                // The seek is *visibly* done here, not when it was issued.
                std::lock_guard lock(state_mutex_);
                state_.seeking = false;
            } else if (event->event_id == MPV_EVENT_LOG_MESSAGE) {
                record_log(*static_cast<mpv_event_log_message *>(event->data));
            } else if (event->event_id == MPV_EVENT_PROPERTY_CHANGE) {
                update_property(*static_cast<mpv_event_property *>(event->data));
            }
        }
    }

    /// Keep the first error of the open file, not the last.
    ///
    /// A failure cascades — one unreadable header produces a dozen follow-on
    /// complaints — and the first line is the one that names the cause. Cleared
    /// by [take_error], which is how the UI consumes it.
    ///
    /// Only mpv's own errors. Everything libavcodec and libavformat complain
    /// about arrives under an `ffmpeg/…` prefix, and mpv is the thing that
    /// decides whether any of it mattered: a decoder that refuses to
    /// initialise is an error there and a fallback here, so surfacing those
    /// puts a dialog over a file that goes on to play perfectly. When one of
    /// them really is fatal mpv says so itself, under a prefix of its own.
    void record_log(const mpv_event_log_message &message) {
        if (!message.text) return;
        if (message.prefix && !std::strncmp(message.prefix, "ffmpeg", 6)) return;
        std::lock_guard lock(state_mutex_);
        if (!error_.empty()) return;
        error_.assign(message.text);
        while (!error_.empty() && (error_.back() == '\n' || error_.back() == '\r')) error_.pop_back();
    }

    void update_property(const mpv_event_property &property) {
        if (!property.data) return;
        std::lock_guard lock(state_mutex_);
        // `volume`, `mute` and `pause` belong to the core and carry across a
        // load; the rest describe the open file, and a reading of them that
        // arrives mid-load is the outgoing episode's clock under the incoming
        // episode's name.
        if (!std::strcmp(property.name, "volume")) state_.volume = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "pause")) state_.paused = *static_cast<int *>(property.data);
        else if (!std::strcmp(property.name, "mute")) state_.muted = *static_cast<int *>(property.data);
        else if (state_.loading) return;
        else if (!std::strcmp(property.name, "time-pos")) state_.position = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "duration")) state_.duration = *static_cast<double *>(property.data);
        else if (!std::strcmp(property.name, "video-params/dw")) state_.video_width = static_cast<double>(*static_cast<int64_t *>(property.data));
        else if (!std::strcmp(property.name, "video-params/dh")) state_.video_height = static_cast<double>(*static_cast<int64_t *>(property.data));
        else if (!std::strcmp(property.name, "paused-for-cache")) state_.buffering = *static_cast<int *>(property.data);
        else if (!std::strcmp(property.name, "cache-buffering-state")) state_.cache_percent = static_cast<double>(*static_cast<int64_t *>(property.data));
    }

    static void render_update(void *opaque) noexcept {
        c_boundary([=] {
            auto *self = static_cast<Player *>(opaque);
            if (!self) return;
            self->frame_ready_.store(true);
            self->render_cv_.notify_one();
        });
    }

    /// Owns EGL and the mpv render context for the life of the player.
    ///
    /// Both are set up here rather than on the first surface attach, and neither
    /// depends on there being a window: a pbuffer is enough to make a context
    /// current, and `vo_libmpv` only needs *some* consumer. That is what makes
    /// audio playback with no surface — the screen off, the app backgrounded —
    /// structurally possible; before this, `load` waited five seconds for a
    /// render context that only a visible SurfaceView could ever create, and
    /// then failed.
    void render_loop() {
        EGLDisplay display = EGL_NO_DISPLAY;
        EGLContext context = EGL_NO_CONTEXT;
        EGLSurface surface = EGL_NO_SURFACE;
        EGLSurface pbuffer = EGL_NO_SURFACE;
        EGLConfig config = nullptr;
        ANativeWindow *window = nullptr;
        if (initialize_egl(display, context, config, pbuffer)) initialize_renderer();
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
                if (window && display == EGL_NO_DISPLAY) {
                    // EGL failed at startup and playback is audio-only. Keep the
                    // window rather than the surface: nothing here can retry.
                    ANativeWindow_release(window);
                    window = nullptr;
                }
                if (window) {
                    surface = eglCreateWindowSurface(display, config, window, nullptr);
                    eglMakeCurrent(display, surface, surface, context);
                }
            }
            const bool draw = frame_ready_.exchange(false);
            lock.unlock();
            // With no window there is still a frame to consume. `vo_libmpv`
            // expects its host to keep draining, and a video thread with no
            // consumer stalls — which is why video came back out of sync with
            // audio after the screen had been off.
            if (draw && surface == EGL_NO_SURFACE && render_) {
                eglMakeCurrent(display, pbuffer, pbuffer, context);
                if (mpv_render_context_update(render_) & MPV_RENDER_UPDATE_FRAME) {
                    int skip = 1;
                    mpv_render_param params[] = {{MPV_RENDER_PARAM_SKIP_RENDERING, &skip}, {MPV_RENDER_PARAM_INVALID, nullptr}};
                    mpv_render_context_render(render_, params);
                }
            }
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

    /// All of EGL, or none of it.
    ///
    /// Leaving a display initialised behind a failed context is what made a
    /// partial failure permanent: the retry guard is "is there a display", so a
    /// half-built state reads as success forever and every later attach uses an
    /// uninitialised config and `EGL_NO_CONTEXT`. Unwinding here keeps the guard
    /// honest, and `eglGetError` says which step failed instead of leaving a
    /// black screen with nothing in the log.
    bool initialize_egl(EGLDisplay &display, EGLContext &context, EGLConfig &config, EGLSurface &pbuffer) {
        const auto fail = [&](const char *step) {
            __android_log_print(ANDROID_LOG_ERROR, "pstr-mpv", "%s failed: 0x%04x", step, eglGetError());
            if (pbuffer != EGL_NO_SURFACE) eglDestroySurface(display, pbuffer);
            if (context != EGL_NO_CONTEXT) eglDestroyContext(display, context);
            if (display != EGL_NO_DISPLAY) eglTerminate(display);
            display = EGL_NO_DISPLAY;
            context = EGL_NO_CONTEXT;
            pbuffer = EGL_NO_SURFACE;
            config = nullptr;
            return false;
        };
        display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
        if (display == EGL_NO_DISPLAY) return fail("eglGetDisplay");
        if (!eglInitialize(display, nullptr, nullptr)) return fail("eglInitialize");
        const EGLint attributes[] = {EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, EGL_WINDOW_BIT | EGL_PBUFFER_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE};
        EGLint count = 0;
        if (!eglChooseConfig(display, attributes, &config, 1, &count) || count == 0) return fail("eglChooseConfig");
        const EGLint context_attributes[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
        context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attributes);
        if (context == EGL_NO_CONTEXT) return fail("eglCreateContext");
        const EGLint pbuffer_attributes[] = {EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE};
        pbuffer = eglCreatePbufferSurface(display, config, pbuffer_attributes);
        if (pbuffer == EGL_NO_SURFACE) return fail("eglCreatePbufferSurface");
        if (!eglMakeCurrent(display, pbuffer, pbuffer, context)) return fail("eglMakeCurrent");
        return true;
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
    /// The first error mpv logged about the open file, until it is consumed.
    std::string error_;
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
        if (!type) return;
        env->ThrowNew(type, "surface must not be null");
        // FindClass hands back a local reference, and this is called from a
        // long-lived attached thread where the frame is not popped for us.
        env->DeleteLocalRef(type);
    });
}

} // namespace

/// FFmpeg's MediaCodec decoders call into Android's Java API, and they can only
/// reach it through a JavaVM handed to them explicitly — there is no way for
/// them to discover one. Without this, every hardware decoder fails to open
/// with "hevc_mediacodec: No Java virtual machine has been registered" and
/// playback falls back to software decoding, which on a phone means a hot
/// device and dropped frames on anything above 1080p.
///
/// mpv-android's own JNI layer does this in its `create` entry point; this
/// library replaces that layer, so the registration has to happen here. The
/// loader calls this once per process before any native method, which is
/// earlier than any decoder can run.
extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *) {
    av_jni_set_java_vm(vm, nullptr);
    return JNI_VERSION_1_6;
}

extern "C" JNIEXPORT jlong JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeCreate(JNIEnv *, jclass) noexcept {
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
extern "C" JNIEXPORT jboolean JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeLoad(JNIEnv *env, jobject, jlong handle, jlong stream, jdouble start, jstring audio, jstring subtitle, jboolean subtitles, jboolean hardware_decoding) noexcept {
    return c_boundary<jboolean>(JNI_FALSE, [=] {
        std::string audio_text;
        std::string subtitle_text;
        if (!utf8(env, audio, audio_text) || !utf8(env, subtitle, subtitle_text)) {
            return jboolean{JNI_FALSE};
        }
        auto *p = from(handle);
        return static_cast<jboolean>(
            p && p->load(static_cast<uint64_t>(stream), start, audio_text,
                         subtitle_text, subtitles == JNI_TRUE,
                         hardware_decoding == JNI_TRUE)
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
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeSpeed(JNIEnv *, jobject, jlong h, jdouble value) noexcept {
    c_boundary([=] { if (auto *p = from(h)) p->speed(value); });
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
                                  state.ended ? 1.0 : 0.0,
                                  static_cast<jdouble>(state.generation),
                                  state.video_width, state.video_height,
                                  static_cast<jdouble>(state.end_reason),
                                  state.buffering ? 1.0 : 0.0, state.cache_percent,
                                  state.seeking ? 1.0 : 0.0};
        constexpr jsize kFields = 13;
        jdoubleArray result = env->NewDoubleArray(kFields);
        if (!result) return static_cast<jdoubleArray>(nullptr);
        env->SetDoubleArrayRegion(result, 0, kFields, values);
        if (env->ExceptionCheck()) return static_cast<jdoubleArray>(nullptr);
        return result;
    });
}
extern "C" JNIEXPORT jstring JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeTakeError(JNIEnv *env, jobject, jlong h) noexcept {
    return c_boundary<jstring>(nullptr, [=]() -> jstring {
        auto *player = from(h);
        if (!player) return nullptr;
        const std::string error = player->take_error();
        return error.empty() ? nullptr : string(env, error);
    });
}
extern "C" JNIEXPORT jstring JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeTracks(JNIEnv *env, jobject, jlong h) noexcept {
    return c_boundary<jstring>(nullptr, [=] {
        return string(env, from(h) ? from(h)->tracks_json() : "[]");
    });
}
extern "C" JNIEXPORT jstring JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_nativeChapters(JNIEnv *env, jobject, jlong h) noexcept {
    return c_boundary<jstring>(nullptr, [=] {
        return string(env, from(h) ? from(h)->chapters_json() : "[]");
    });
}
extern "C" JNIEXPORT void JNICALL Java_io_narl_protonstream_playback_NativeMpvHost_pstrAndroidStreamRelease(JNIEnv *, jobject, jlong handle) noexcept {
    c_boundary([=] { pstr_android_stream_release(static_cast<uint64_t>(handle)); });
}

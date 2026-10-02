import Foundation
import Libmpv

// libmpv's `pstr://<handle>` protocol, backed by the Rust stream registry. The
// Swift port of the stream half of android/app/src/main/cpp/pstr_mpv.cpp.

/// One open file on mpv's demuxer thread. `position` is only ever touched from
/// that thread; `cancelled` is also written from whichever thread cancels.
private final class StreamCookie {
    let handle: UInt64
    let size: Int64
    var position: Int64 = 0
    private let lock = NSLock()
    private var _cancelled = false

    init(handle: UInt64, size: Int64) {
        self.handle = handle
        self.size = size
    }

    var cancelled: Bool {
        get { lock.withLock { _cancelled } }
        set { lock.withLock { _cancelled = newValue } }
    }
}

private func cookie(_ opaque: UnsafeMutableRawPointer?) -> StreamCookie? {
    opaque.map { Unmanaged<StreamCookie>.fromOpaque($0).takeUnretainedValue() }
}

/// Register the protocol on a core that has not been initialized yet.
func registerStreamProtocol(_ mpv: OpaquePointer) -> Bool {
    mpv_stream_cb_add_ro(mpv, "pstr", nil, streamOpen) >= 0
}

private func streamOpen(
    _: UnsafeMutableRawPointer?,
    _ uri: UnsafeMutablePointer<CChar>?,
    _ info: UnsafeMutablePointer<mpv_stream_cb_info>?
) -> Int32 {
    let failed = MPV_ERROR_LOADING_FAILED.rawValue
    guard let uri, let info else { return failed }
    let value = String(cString: uri)
    guard value.hasPrefix("pstr://"), let handle = UInt64(value.dropFirst("pstr://".count)) else { return failed }
    let size = pstr_android_stream_size(handle)
    guard size >= 0 else { return failed }

    info.pointee.cookie = Unmanaged.passRetained(StreamCookie(handle: handle, size: size)).toOpaque()
    info.pointee.read_fn = { opaque, buffer, length in
        guard let stream = cookie(opaque), let buffer else { return -1 }
        if stream.cancelled { return -1 }
        let remaining = stream.size - stream.position
        if remaining <= 0 { return 0 }
        let requested = Int(min(length, UInt64(remaining)))
        let read = pstr_android_stream_read(stream.handle, UInt64(stream.position), buffer, requested)
        if read > 0 { stream.position += read }
        return read
    }
    info.pointee.seek_fn = { opaque, offset in
        guard let stream = cookie(opaque), offset >= 0, offset <= stream.size else {
            return Int64(MPV_ERROR_GENERIC.rawValue)
        }
        stream.position = offset
        stream.cancelled = false
        return offset
    }
    info.pointee.size_fn = { opaque in
        cookie(opaque)?.size ?? Int64(MPV_ERROR_GENERIC.rawValue)
    }
    // The flag stops the next read; the one already parked in a 4 MiB block
    // fetch has to be interrupted in Rust, or `mpv_terminate_destroy` waits
    // out the whole fetch while joining the demuxer thread (B16).
    info.pointee.cancel_fn = { opaque in
        guard let stream = cookie(opaque) else { return }
        stream.cancelled = true
        pstr_android_stream_cancel(stream.handle)
    }
    // The one moment defined to be after mpv's last read of this file, so the
    // Rust stream is released here and nowhere else (B15/B28).
    info.pointee.close_fn = { opaque in
        guard let opaque else { return }
        let stream = Unmanaged<StreamCookie>.fromOpaque(opaque).takeRetainedValue()
        pstr_android_stream_release(stream.handle)
    }
    return 0
}

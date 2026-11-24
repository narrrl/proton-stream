// The blocking stream ABI exported by crates/pstr-android (`lib.rs`,
// `pstr_android_stream_*`). It is not part of the UniFFI interface on purpose:
// decrypted bytes go straight from Rust into libmpv's buffer and never pass
// through Swift values.
#pragma once

#include <stddef.h>
#include <stdint.h>

/// Read up to `length` bytes at `offset` into `buffer`. Returns the byte count,
/// 0 at end of file, or -1 on failure or cancellation.
int64_t pstr_android_stream_read(uint64_t handle, uint64_t offset, void *buffer, size_t length);

/// The stream's size in bytes, or -1 for an unknown handle.
int64_t pstr_android_stream_size(uint64_t handle);

/// libmpv opened the stream: its size, or -1 for an unknown handle. Counted,
/// so that the release from closing one open does not end another — mpv opens
/// an ordered-chapters episode twice.
int64_t pstr_android_stream_open(uint64_t handle);

/// Interrupt the read in flight for this stream, if any.
void pstr_android_stream_cancel(uint64_t handle);

/// Drop the registry's reference. Call exactly once per published handle.
void pstr_android_stream_release(uint64_t handle);

/// Hand back a stream offered to libmpv that it never opened. One it has open
/// is left to its close.
void pstr_android_stream_release_unopened(uint64_t handle);

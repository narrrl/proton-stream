#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
int64_t pstr_android_stream_read(uint64_t handle, uint64_t offset, void *buffer, size_t length);
int64_t pstr_android_stream_size(uint64_t handle);
/// Interrupts an in-flight `pstr_android_stream_read`. A cancel with no reader
/// waiting is dropped, so this is safe to call speculatively.
void pstr_android_stream_cancel(uint64_t handle);
void pstr_android_stream_release(uint64_t handle);
#ifdef __cplusplus
}
#endif

#pragma once
#include <stdbool.h>
#include <stdint.h>
#include <ghostty/vt.h>

typedef struct HerdrRenderer HerdrRenderer;
typedef struct { uint32_t width, height; } HerdrCellSize;
// Main/UI thread only. No shell, PTY, configuration file, or renderer thread is created.
HerdrRenderer *herdr_renderer_new(const char *const *args, size_t argc, float points, double scale);
void herdr_renderer_free(HerdrRenderer *renderer);
HerdrCellSize herdr_renderer_cell_size(const HerdrRenderer *renderer);
// Borrows the VT and its prepared snapshot only for this call. Returns an owned
// CVPixelBufferRef, independent of Ghostty's swap chain. Release with CFRelease.
void *herdr_renderer_draw(HerdrRenderer *renderer, GhosttyTerminal terminal,
                         GhosttyRenderState snapshot, bool focused, bool full);

// Error detail from the last new/draw on the UI thread. Valid until next call.
const char *herdr_renderer_last_error(void);
// Register in-memory fonts process-wide for CoreText discovery, not on disk.
bool herdr_renderer_register_font(const uint8_t *data, size_t length, char *message, size_t capacity);
// Internal test seam for the upstream borrowed-snapshot reset boundary.
void herdr_renderer_test_age(HerdrRenderer *renderer, size_t frames);
bool herdr_renderer_has_font(const char *family);

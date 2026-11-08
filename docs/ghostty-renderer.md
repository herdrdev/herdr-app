# Ghostty renderer on macOS

This optional renderer uses the Ghostty version already pinned in `vendor/ghostty`.
It does not use Rex's compiled bundle or depend on a separately versioned terminal
widget. Normal builds and existing settings continue to use GPUI.

## Build and select

Install Rust, Zig 0.16.0 and Xcode Command Line Tools as for the normal app. Set
`ZIG` to the Zig executable if it is not on PATH. Metal shaders compile at runtime;
the separate Apple Metal compiler download is not required.

```sh
cargo build --locked --features ghostty-renderer
```

In **Settings → Look → Terminal renderer**, choose **Ghostty** or **Default**
(GPUI). The change applies immediately to open terminals and is saved in
`settings.json`. Builds without Ghostty support show its choice as unavailable.

For an isolated preview, you can also set the app home's `settings.json`:

```json
{
  "terminal_renderer": "ghostty",
  "font": "Monaco",
  "font_size": 15,
  "colors": {"terminal_foreground": "#bbbbbb"}
}
```

Run with `HERDR_APP_HOME` pointing at that scratch directory and a separate
`HC_SESSION` and `XDG_STATE_HOME`. Do not launch a second app against the home or
core session used by the installed app. A layout's font and size overrides still
win over settings, just as with GPUI.

Set `terminal_renderer` back to `gpui` to compare without reconnecting terminals.
Other platforms and builds without the feature use GPUI. Initialization or draw
failures are logged and fall back to GPUI; changing configuration retries native
initialization. Configuration diagnostics and native failure details reach Herdr’s
log. A compositor texture-allocation failure logs and skips the affected surface
instead of terminating the app.

## Integration

Herdr still owns the core connection, terminal byte stream, libghostty-vt state,
input encoding, clipboard policy, selection, search and scrollback. Ghostty draws
the *same prepared render snapshot*, before Herdr clears its dirty flags. It never
starts a subprocess, allocates a PTY or feeds a second terminal parser.

The bridge builds the VT C API and the renderer in one archive, so the shared
terminal types cannot disagree. `crates/ghostty-sys/renderer/ghostty.patch` is applied
to a private copy under Cargo's output directory; the pinned submodule stays
unchanged. The patch permits a detached Metal view and a host-prepared snapshot.
Source synchronization preserves Zig’s compiler and package caches, and patching
rejects fuzzy matches. A first build may download hash-pinned lazy dependencies;
offline builds need those packages cached beforehand. Internationalization is
disabled, including its otherwise-unnecessary static libintl dependency.
Ghostty initialization only enables rendering services; it does not install
signal handlers, change the host locale/limits, load Ghostty user configuration,
or initialize crash reporting.

Ghostty supplies physical cell metrics. Herdr converts them using the window's
scale factor and uses the result for terminal resizing, program pixel reports,
selection, scrolling, cursor placement and input-method candidate positioning.
Font, scale, ligature, contrast, opacity and theme changes recreate the drawing
resources while keeping the terminal attached. Initial display reports use these
metrics too. Bundled fonts and fonts loaded from the app home or a mod are
registered with CoreText for this process, making them available to both backends
without installing them in macOS. Unavailable font families produce a warning.

Herdr owns synchronized-output policy: Ghostty draws the captured snapshot even
while the live terminal is held. Ghostty’s periodic snapshot reset is disabled for
this borrowed state. Fresh renderer instances re-upload existing Kitty images.
Native rendering cannot scroll Herdr’s viewport or consume its search-dirty flag.
Kitty upload bookkeeping and animation advancement still touch the terminal on
the same UI thread; there is no concurrent parser access.

A detached layer hosts Ghostty's IOSurface. A GPU blit copies each completed frame
into a pooled CVPixelBuffer independent of Ghostty's swap chain; retaining a GPUI
scene can therefore never expose a subsequently overwritten terminal frame. GPUI
composites BGRA surfaces using the existing scene order and content masks, so pane
clipping, menus and popovers work normally. BGRA frames use premultiplied-alpha
blending to preserve translucent backgrounds. No CPU pixel readback is used to draw
the app. Herdr draws its search, link, IME and scrollbar overlays above the native
surface. Container opacity also applies to native surfaces, and CoreVideo texture
wrappers remain alive until GPU completion. If native drawing fails, Herdr
schedules another frame to restore GPUI geometry even on an idle terminal.

The copy reuses Ghostty’s GPU and command queue. Device selection follows GPUI’s
preference for integrated, non-removable GPUs on Intel Macs. Immutable Metal
shader libraries are cached per device across terminals and configuration changes.

## Validation

Run the native GPU regression test on macOS:

```sh
cargo test --locked --features ghostty-renderer --test ghostty_renderer
```

It uses headless GPUI rendering and a detached native view, without opening a user
window or connecting to a core. `HERDR_RENDERER_TEST_IMAGE=/tmp/ghostty-renderer.png`
saves the checked image. It checks glyphs, incremental output, resizing, font
metrics, clipping, overlays, terminal and container opacity, and retained-frame
lifetime. It also runs Herdr’s actual `Vt::frame_with` path for synchronized
output, the 100,000-update boundary, and Kitty images across renderer recreation;
it checks font registration, invalid-configuration diagnostics, and safe handling
of a non-Metal surface. Run the regular
tests with the feature enabled and the default build as well.

Set `HERDR_RENDERER_BENCH=/tmp/ghostty-benchmark.json` on that test command for a
ten-terminal benchmark (96 × 30 cells each at 2× scale). It measures creation plus
first frames with the shader library already cached, followed by 30 batches of
incremental updates. On the development Apple Silicon Mac, one run took 81 ms for
creation/first frames and 8.3 ms median / 15.1 ms p95 per ten-terminal update batch.
These timings exclude app/core work and GPUI composition; they are not an
end-to-end frame-rate measure or a portable performance guarantee.

The implementation was also checked in an isolated Apple Silicon macOS window
with Monaco 15 and foreground `#bbbbbb`: switching GPUI/Ghostty on the same live
terminal, terminal input, split panes, the search overlay, switching to bundled
Geist Mono, and changing terminal opacity.

## Current limits

This is an experimental macOS integration. Frame production and the GPU copy
complete synchronously on the UI thread. It does not yet move Ghostty work to a
separate renderer thread. Each terminal owns its font/renderer resources, and
hidden terminals retain them until they repaint or close. End-to-end frame pacing
and Intel/dual-GPU behavior still need hardware validation before making this
the default. Apple Silicon is the tested platform. The pinned renderer still
compiles its custom-shader/inspector support and their dependencies; this bridge
does not expose those features. Oniguruma is needed by terminal search.

The native renderer uses sRGB, Ghostty's default alpha blending and no extra font
thickening. This is upstream Ghostty rendering, not a promise of pixel-identical
Rex output. Ghostty supplies its own glyph fallback, decorations, selection and
contrast rendering. Herdr's search highlights and composing text remain GPUI
overlays. Existing Herdr cursor scheduling is retained; this bridge does not add
an independent blinking or animated-image clock.

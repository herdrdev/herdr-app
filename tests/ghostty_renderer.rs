//! Native Metal + GPUI composition on the main thread, without a real window/core.
// Use Herdr's actual VT/parser/snapshot implementation. Only the application
// settings source is a fixed fixture; no terminal or renderer behavior is mocked.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/links.rs"]
mod links;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[allow(unused_imports)]
#[path = "../src/sprites.rs"]
mod sprites;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/vt.rs"]
mod vt;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/win32_keys.rs"]
mod win32_keys;
#[cfg(target_os = "macos")]
mod mirror {
    // Only referenced by the VT module's separately-run link unit tests.
    pub fn qualify(id: &str, machine: Option<&str>) -> String {
        machine.map_or_else(|| id.to_owned(), |m| format!("{m}/{id}"))
    }
}
#[cfg(target_os = "macos")]
mod settings {
    pub struct Settings {
        pub minimum_contrast: f32,
    }
    pub fn get() -> Settings {
        Settings {
            minimum_contrast: 1.0,
        }
    }
    pub fn rgb(hex: &str) -> crate::vt::Rgb {
        let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap();
        crate::vt::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }
    pub fn theme() -> std::collections::BTreeMap<String, String> {
        let mut colors = std::collections::BTreeMap::from([
            ("background".into(), "#000000".into()),
            ("terminal_foreground".into(), "#bbbbbb".into()),
            ("cursor".into(), "#bbbbbb".into()),
            ("selection".into(), "#264f78".into()),
        ]);
        for i in 0..16 {
            colors.insert(format!("ansi{i}"), "#bbbbbb".into());
        }
        colors
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::base::TCFType;
    use core_video::pixel_buffer::CVPixelBuffer;
    use ghostty_sys::*;
    use gpui::{prelude::*, *};
    use std::{ffi::CString, ptr};

    struct Terminal {
        term: GhosttyTerminal,
        state: GhosttyRenderState,
        renderer: *mut HerdrRenderer,
        cell: HerdrCellSize,
    }
    impl Terminal {
        fn new(points: f32, scale: f64, opacity: &str) -> Self {
            let opacity = format!("--background-opacity={opacity}");
            let args = [
                "--font-family=Monaco",
                &opacity,
                "--foreground=#bbbbbb",
                "--background=#000000",
                "--font-thicken=false",
                "--cursor-style-blink=false",
            ];
            let args: Vec<_> = args.iter().map(|s| CString::new(*s).unwrap()).collect();
            let args: Vec<_> = args.iter().map(|s| s.as_ptr()).collect();
            unsafe {
                let renderer = herdr_renderer_new(args.as_ptr(), args.len(), points, scale);
                assert!(
                    !renderer.is_null(),
                    "real Ghostty Metal renderer must initialize"
                );
                let cell = herdr_renderer_cell_size(renderer);
                let mut term = ptr::null_mut();
                assert_eq!(
                    ghostty_terminal_new(ptr::null(), &mut term, 32, 4),
                    GHOSTTY_SUCCESS
                );
                let mut state = ptr::null_mut();
                assert_eq!(
                    ghostty_render_state_new(ptr::null(), &mut state),
                    GHOSTTY_SUCCESS
                );
                ghostty_terminal_resize(term, 32, 4, cell.width, cell.height);
                Self {
                    term,
                    state,
                    renderer,
                    cell,
                }
            }
        }
        fn write(&self, text: &str) {
            unsafe {
                ghostty_terminal_vt_write(self.term, text.as_ptr(), text.len());
            }
        }
        fn frame(&self) -> CVPixelBuffer {
            unsafe {
                assert_eq!(
                    ghostty_render_state_update(self.state, self.term),
                    GHOSTTY_SUCCESS
                );
                let frame = herdr_renderer_draw(self.renderer, self.term, self.state, true, false);
                assert!(
                    !frame.is_null(),
                    "native draw must return an IOSurface frame"
                );
                ghostty_render_state_clean(self.state);
                CVPixelBuffer::wrap_under_create_rule(frame.cast())
            }
        }
    }
    impl Drop for Terminal {
        fn drop(&mut self) {
            unsafe {
                herdr_renderer_free(self.renderer);
                ghostty_render_state_free(self.state);
                ghostty_terminal_free(self.term);
            }
        }
    }

    struct Scene {
        frame: CVPixelBuffer,
        opacity: f32,
    }
    impl Render for Scene {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let frame = self.frame.clone();
            div()
                .size_full()
                .bg(rgb(0xff00ff))
                .child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .top(px(4.))
                        .w(px(120.))
                        .h(px(60.))
                        .overflow_hidden()
                        .opacity(self.opacity)
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    window.paint_surface(
                                        Bounds::new(
                                            bounds.origin,
                                            size(
                                                px(frame.get_width() as f32 / 2.),
                                                px(frame.get_height() as f32 / 2.),
                                            ),
                                        ),
                                        frame.clone(),
                                    );
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(8.))
                        .top(px(8.))
                        .size(px(4.))
                        .bg(rgb(0x00ff00)),
                )
        }
    }

    fn capture(cx: &mut HeadlessAppContext, frame: CVPixelBuffer) -> image::RgbaImage {
        capture_opacity(cx, frame, 1.0)
    }
    fn capture_opacity(
        cx: &mut HeadlessAppContext,
        frame: CVPixelBuffer,
        opacity: f32,
    ) -> image::RgbaImage {
        let window = cx
            .open_window(size(px(150.), px(80.)), |_, cx| {
                cx.new(|_| Scene { frame, opacity })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
            window.render_to_image().unwrap()
        })
        .unwrap()
    }

    fn vt_frame(vt: &mut crate::vt::Vt, native: &Terminal) -> (crate::vt::Frame, CVPixelBuffer) {
        let mut pixels = None;
        let frame = vt.frame_with(|terminal, state, full, _| unsafe {
            let raw = herdr_renderer_draw(native.renderer, terminal, state, true, full);
            assert!(
                !raw.is_null(),
                "native VT frame: {}",
                std::ffi::CStr::from_ptr(herdr_renderer_last_error()).to_string_lossy()
            );
            pixels = Some(CVPixelBuffer::wrap_under_create_rule(raw.cast()));
        });
        (frame, pixels.unwrap())
    }

    fn snapshot_regressions(cx: &mut HeadlessAppContext) {
        let native = Terminal::new(15., 2., "1");
        let mut vt = crate::vt::Vt::new(32, 4);
        vt.resize(32, 4, native.cell.width, native.cell.height);
        vt.write(b"\x1b[?25lold row\r\nsecond row");
        let (_, first) = vt_frame(&mut vt, &native);
        let first = capture(cx, first);
        // A change preceding BSU must survive a paint during the hold, even
        // though Ghostty's live terminal is now in synchronized-output mode.
        vt.write(b"\x1b[Hnew row\x1b[?2026h\x1b[2;1Hinside hold");
        let (held_rows, held) = vt_frame(&mut vt, &native);
        assert_eq!(held_rows.rows[0][0].text, "n");
        assert_eq!(held_rows.rows[1][0].text, "s");
        let held = capture(cx, held);
        assert_ne!(
            first, held,
            "pre-BSU changes must be painted during the hold"
        );
        vt.write(b"\x1b[?2026l");
        let (released_rows, released) = vt_frame(&mut vt, &native);
        assert_eq!(released_rows.rows[1][0].text, "i");
        let released = capture(cx, released);
        // Compare to a clean renderer of the exact same prepared VT contents.
        let fresh = Terminal::new(15., 2., "1");
        let (_, expected) = vt_frame(&mut vt, &fresh);
        assert_eq!(
            released,
            capture(cx, expected),
            "ESU must not leave stale rows"
        );
        unsafe {
            herdr_renderer_test_age(native.renderer, 100_000);
        }
        let (aged_rows, aged) = vt_frame(&mut vt, &native);
        assert_eq!(aged_rows.rows.len(), 4);
        assert_eq!(
            released,
            capture(cx, aged),
            "aging must never free the host snapshot"
        );
        let (after_rows, after) = vt_frame(&mut vt, &native);
        assert_eq!(after_rows.rows[0][0].text, "n");
        assert_eq!(released, capture(cx, after));

        vt.write(b"\x1b[H\x1b_Ga=T,f=24,s=1,v=1,c=3,r=2,i=7,q=2;/wAA\x1b\\");
        assert!(
            !vt.placements().is_empty(),
            "Kitty fixture must be stored by Herdr"
        );
        let (_, image_frame) = vt_frame(&mut vt, &native);
        let image_frame = capture(cx, image_frame);
        assert!(
            image_frame
                .pixels()
                .any(|p| p[0] > 200 && p[1] < 40 && p[2] < 40),
            "native Kitty image must be visible"
        );
        let recreated = Terminal::new(15., 2., "1");
        let (_, restored) = vt_frame(&mut vt, &recreated);
        assert_eq!(
            image_frame,
            capture(cx, restored),
            "existing images must survive renderer recreation"
        );
    }

    fn benchmark() {
        if std::env::var_os("HERDR_RENDERER_BENCH").is_none() {
            return;
        }
        let start = std::time::Instant::now();
        let terminals: Vec<_> = (0..10)
            .map(|_| {
                let terminal = Terminal::new(15., 2., "1");
                unsafe {
                    ghostty_terminal_resize(
                        terminal.term,
                        96,
                        30,
                        terminal.cell.width,
                        terminal.cell.height,
                    );
                }
                terminal.write("\x1b[?25lTen-terminal Ghostty benchmark\r\n");
                let _ = terminal.frame();
                terminal
            })
            .collect();
        let create_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut samples = Vec::new();
        for frame in 0..30 {
            let start = std::time::Instant::now();
            for terminal in &terminals {
                terminal.write(&format!("\x1b[Hupdated frame {frame:03}"));
                let _ = terminal.frame();
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.);
        }
        samples.sort_by(f64::total_cmp);
        let result = serde_json::json!({"terminals":10,"columns":96,"rows":30,"scale":2,"warm_create_and_first_frames_ms":create_ms,"ten_terminal_update_p50_ms":samples[15],"ten_terminal_update_p95_ms":samples[28]});
        println!("Native render benchmark (excludes app/core/compositor): {result}");
        if let Ok(path) = std::env::var("HERDR_RENDERER_BENCH") {
            if path != "1" {
                std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
            }
        }
    }

    pub fn run() {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        // Register a bundled font through the same process-scoped bridge used
        // by the application, then verify CoreText discovers its family.
        let font = include_bytes!("../assets/fonts/GeistMono-Variable.ttf");
        let mut message = [0i8; 1024];
        assert!(unsafe {
            herdr_renderer_register_font(
                font.as_ptr(),
                font.len(),
                message.as_mut_ptr(),
                message.len(),
            )
        });
        assert!(unsafe { herdr_renderer_has_font(c"Geist Mono".as_ptr()) });
        let invalid = [c"--scroll-to-bottom=key".as_ptr()];
        assert!(unsafe { herdr_renderer_new(invalid.as_ptr(), 1, 15., 2.) }.is_null());
        let error =
            unsafe { std::ffi::CStr::from_ptr(herdr_renderer_last_error()) }.to_string_lossy();
        assert!(
            error.contains("scroll-to-bottom"),
            "diagnostics must cross the FFI: {error}"
        );
        // A CPU-only pixel buffer has no IOSurface for the Metal texture cache.
        // The compositor must skip it, leaving the parent visible, not panic.
        let unsupported = CVPixelBuffer::new(
            core_video::pixel_buffer::kCVPixelFormatType_32BGRA,
            160,
            80,
            None,
        )
        .unwrap();
        let skipped = capture(&mut cx, unsupported);
        assert_eq!(skipped.get_pixel(80, 70).0, [255, 0, 255, 255]);
        snapshot_regressions(&mut cx);
        let t = Terminal::new(15., 2., "1");
        assert!(t.cell.width > 0 && t.cell.height > 0);
        t.write("\x1b[?25l\x1b[2J\x1b[HMonaco 15 — Ghostty\r\n\x1b[31mred\x1b[0m bold \x1b[1mtext\x1b[0m");
        let first = t.frame();
        assert_eq!(first.get_width(), t.cell.width as usize * 32);
        assert_eq!(first.get_height(), t.cell.height as usize * 4);
        let image = capture(&mut cx, first.clone());
        let hidden = capture_opacity(&mut cx, first.clone(), 0.0);
        assert_eq!(
            hidden.get_pixel(80, 70).0,
            [255, 0, 255, 255],
            "opacity-zero containers must hide native surfaces"
        );
        let faded = capture_opacity(&mut cx, first.clone(), 0.5);
        let color = faded.get_pixel(80, 70).0;
        assert!(
            color[0].abs_diff(128) <= 1 && color[2].abs_diff(128) <= 1 && color[1] == 0,
            "element opacity must fade premultiplied surfaces: {color:?}"
        );
        assert_eq!(
            image.get_pixel(2, 2).0,
            [255, 0, 255, 255],
            "outside surface"
        );
        assert_eq!(
            image.get_pixel(18, 18).0,
            [0, 255, 0, 255],
            "GPUI overlay must cover the terminal"
        );
        assert_eq!(
            image.get_pixel(260, 40).0,
            [255, 0, 255, 255],
            "terminal must clip to pane"
        );
        assert!(
            image
                .pixels()
                .any(|p| p[0] > 50 && p[0] == p[1] && p[1] == p[2]),
            "native glyph pixels"
        );
        for n in 0..6 {
            t.write(&format!("\x1b[Hframe {n}             "));
            let _ = t.frame();
        }
        assert_eq!(
            image,
            capture(&mut cx, first),
            "retained frame must survive swap-chain reuse"
        );
        let changed = capture(&mut cx, t.frame());
        assert_ne!(
            image, changed,
            "incremental text changes must reach the renderer"
        );
        unsafe {
            ghostty_terminal_resize(t.term, 18, 3, t.cell.width, t.cell.height);
        }
        let resized = t.frame();
        assert_eq!(
            (resized.get_width(), resized.get_height()),
            (t.cell.width as usize * 18, t.cell.height as usize * 3)
        );
        let zoom = Terminal::new(20., 2., "1");
        assert!(zoom.cell.height > t.cell.height && zoom.cell.width > t.cell.width);
        // Composite real premultiplied Ghostty pixels over magenta. Applying
        // source alpha a second time would darken translucent backgrounds.
        let translucent = Terminal::new(15., 2., "0.5");
        translucent.write("\x1b[?25l\x1b]11;#808080\x07\x1b[2J");
        let frame = translucent.frame();
        assert_eq!(frame.lock_base_address(1), 0);
        let bgra = unsafe {
            std::slice::from_raw_parts(frame.get_base_address().cast::<u8>(), 4).to_vec()
        };
        assert_eq!(frame.unlock_base_address(1), 0);
        assert!(
            bgra[3] > 100 && bgra[3] < 155,
            "native background opacity: {bgra:?}"
        );
        let composed = capture(&mut cx, frame);
        let expected = [
            bgra[2].saturating_add(255 - bgra[3]),
            bgra[1],
            bgra[0].saturating_add(255 - bgra[3]),
            255,
        ];
        let actual = composed.get_pixel(80, 70).0;
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= 2,
                "premultiplied opacity must survive GPUI composition"
            );
        }
        benchmark();
        if let Ok(path) = std::env::var("HERDR_RENDERER_TEST_IMAGE") {
            image.save(path).unwrap();
        }
        println!(
            "Ghostty Metal renderer: VT snapshots/sync output, aging, image recreation, font registration, diagnostics, glyphs, resizing, clipping, overlays, element opacity and frame lifetime passed"
        );
    }
}

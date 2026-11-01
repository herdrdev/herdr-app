#![cfg(target_os = "linux")]

// Run focused renderer tests with the app's normal test suite.
#[path = "../vendor/gpui-pre-wgpu/src/colr.rs"]
mod colr;

use gpui::{
    AppContext, Bounds, Context, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId,
    HeadlessAppContext, Hsla, IntoElement, LineLayout, Pixels, PlatformTextSystem, Render,
    RenderGlyphParams, Size, Styled, TextRenderingMode, Window, canvas, point, px, rgb, size,
};
use gpui_wgpu::CosmicTextSystem;
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

static GPU_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CountingText {
    inner: CosmicTextSystem,
    rasters: AtomicUsize,
    native_keys: bool,
}
impl PlatformTextSystem for CountingText {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> anyhow::Result<()> {
        self.inner.add_fonts(fonts)
    }
    fn all_font_names(&self) -> Vec<String> {
        self.inner.all_font_names()
    }
    fn font_id(&self, f: &Font) -> anyhow::Result<FontId> {
        self.inner.font_id(f)
    }
    fn font_metrics(&self, f: FontId) -> FontMetrics {
        self.inner.font_metrics(f)
    }
    fn typographic_bounds(&self, f: FontId, g: GlyphId) -> anyhow::Result<Bounds<f32>> {
        self.inner.typographic_bounds(f, g)
    }
    fn advance(&self, f: FontId, g: GlyphId) -> anyhow::Result<Size<f32>> {
        self.inner.advance(f, g)
    }
    fn glyph_for_char(&self, f: FontId, c: char) -> Option<GlyphId> {
        self.inner.glyph_for_char(f, c)
    }
    fn glyph_uses_foreground(&self, p: &RenderGlyphParams) -> bool {
        !self.native_keys && self.inner.glyph_uses_foreground(p)
    }
    fn glyph_raster_bounds(&self, p: &RenderGlyphParams) -> anyhow::Result<Bounds<DevicePixels>> {
        self.inner.glyph_raster_bounds(p)
    }
    fn rasterize_glyph(
        &self,
        p: &RenderGlyphParams,
        b: Bounds<DevicePixels>,
    ) -> anyhow::Result<(Size<DevicePixels>, Vec<u8>)> {
        self.rasters.fetch_add(1, Ordering::Relaxed);
        self.inner.rasterize_glyph(p, b)
    }
    fn layout_line(&self, text: &str, size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.inner.layout_line(text, size, runs)
    }
    fn recommended_rendering_mode(&self, f: FontId, size: Pixels) -> TextRenderingMode {
        self.inner.recommended_rendering_mode(f, size)
    }
}

struct ColorScene {
    font: FontId,
    glyph: GlyphId,
    color: Hsla,
}
impl Render for ColorScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (font, glyph, color) = (self.font, self.glyph, self.color);
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window
                    .paint_emoji_with_color(point(px(4.), px(40.)), font, glyph, px(28.), color)
                    .unwrap();
            },
        )
        .size_full()
    }
}

struct RepeatedColorScene {
    font: FontId,
    glyph: GlyphId,
    font_size: Pixels,
    color: Hsla,
}
impl Render for RepeatedColorScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (font, glyph, font_size, color) = (self.font, self.glyph, self.font_size, self.color);
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                for i in 0..100 {
                    window
                        .paint_emoji_with_color(
                            point(
                                px(4. + (i % 10) as f32 * 32.),
                                px(32. + (i / 10) as f32 * 32.),
                            ),
                            font,
                            glyph,
                            font_size,
                            color,
                        )
                        .unwrap();
                }
            },
        )
        .size_full()
    }
}

#[test]
fn repeated_foreground_glyphs_render_once_per_live_variant() {
    let _gpu = GPU_TEST.lock().unwrap();
    let mut counts = Vec::new();
    for font_size in [14., 28.] {
        let inner = system();
        inner
            .add_fonts(vec![Cow::Borrowed(include_bytes!(
                "../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf"
            ))])
            .unwrap();
        let font = inner.font_id(&gpui::font("COLR Probe")).unwrap();
        let glyph = inner.glyph_for_char(font, '\u{e000}').unwrap();
        let text = Arc::new(CountingText {
            inner,
            rasters: AtomicUsize::new(0),
            native_keys: false,
        });
        let mut cx = HeadlessAppContext::with_platform(
            text.clone(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(328.), px(328.)), |_, cx| {
                cx.new(|_| RepeatedColorScene {
                    font,
                    glyph,
                    font_size: px(font_size),
                    color: Hsla::black(),
                })
            })
            .unwrap();
        assert_eq!(
            text.inner.glyph_image_render_count(),
            1,
            "initial policy discovery must also supply the bounds and upload image"
        );
        for (index, color) in [
            rgb(0xff0000).into(),
            rgb(0x0000ff).into(),
            Hsla {
                a: 0.5,
                ..rgb(0xff0000).into()
            },
        ]
        .into_iter()
        .enumerate()
        {
            let before = text.inner.glyph_image_render_count();
            let before_uploads = text.rasters.load(Ordering::Relaxed);
            for frame in 0..3 {
                cx.update_window(window.into(), |scene, window, cx| {
                    scene
                        .downcast::<RepeatedColorScene>()
                        .unwrap()
                        .update(cx, |scene, cx| {
                            scene.color = color;
                            cx.notify();
                        });
                    let _ = window.draw(cx);
                    let image = window.render_to_image().unwrap();
                    if frame == 2 {
                        if let Ok(dir) = std::env::var("COLR_EVIDENCE") {
                            image
                                .save(format!("{dir}/foreground-{font_size}-{index}.png"))
                                .unwrap();
                        }
                    }
                })
                .unwrap();
            }
            counts.push((
                font_size,
                index,
                text.inner.glyph_image_render_count() - before,
            ));
            assert_eq!(text.rasters.load(Ordering::Relaxed) - before_uploads, 1);
        }
    }
    eprintln!("actual image renders for 100 occurrences over three frames: {counts:?}");
    for (font_size, color, count) in counts {
        assert_eq!(count, 1, "size={font_size}, color={color}");
    }
}

#[test]
fn palette_and_native_color_glyphs_reuse_one_atlas_upload() {
    let _gpu = GPU_TEST.lock().unwrap();
    for native_keys in [false, true] {
        let inner = system();
        inner.add_fonts(vec![Cow::Borrowed(COLOR)]).unwrap();
        let line = layout(&inner, "🚀");
        let run = &line.runs[0];
        let (font, glyph) = (run.font_id, run.glyphs[0].id);
        let text = Arc::new(CountingText {
            inner,
            rasters: AtomicUsize::new(0),
            native_keys,
        });
        let mut cx = HeadlessAppContext::with_platform(
            text.clone(),
            Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        let window = cx
            .open_window(size(px(64.), px(64.)), |_, cx| {
                cx.new(|_| ColorScene {
                    font,
                    glyph,
                    color: Hsla::black(),
                })
            })
            .unwrap();
        for i in 0..64 {
            cx.update_window(window.into(), |scene, window, cx| {
                scene
                    .downcast::<ColorScene>()
                    .unwrap()
                    .update(cx, |scene, cx| {
                        scene.color = rgb(i * 123457).into();
                        cx.notify();
                    });
                let _ = window.draw(cx);
                window.render_to_image().unwrap();
            })
            .unwrap();
        }
        assert_eq!(
            text.rasters.load(Ordering::Relaxed),
            1,
            "native_keys={native_keys}"
        );
    }
}

#[test]
fn rejected_outline_uploads_to_real_color_atlas() {
    let _gpu = GPU_TEST.lock().unwrap();
    let inner = system();
    inner
        .add_fonts(vec![Cow::Owned(rejected_with_outline())])
        .unwrap();
    let font = inner.font_id(&gpui::font("COLR Probe")).unwrap();
    let glyph = inner.glyph_for_char(font, '\u{e000}').unwrap();
    let text = Arc::new(CountingText {
        inner,
        rasters: AtomicUsize::new(0),
        native_keys: false,
    });
    let mut cx = HeadlessAppContext::with_platform(
        text,
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let window = cx
        .open_window(size(px(64.), px(64.)), |_, cx| {
            cx.new(|_| ColorScene {
                font,
                glyph,
                color: rgb(0xff0000).into(),
            })
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let image = window.render_to_image().unwrap();
        assert!(image.pixels().any(|p| p[0] > 200 && p[1] == 0 && p[2] == 0));
    })
    .unwrap();
}

#[test]
fn foreground_variants_retire_after_leaving_both_live_scenes() {
    let _gpu = GPU_TEST.lock().unwrap();
    let inner = system();
    inner
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf"
        ))])
        .unwrap();
    let font = inner.font_id(&gpui::font("COLR Probe")).unwrap();
    let glyph = inner.glyph_for_char(font, '\u{e000}').unwrap();
    let text = Arc::new(CountingText {
        inner,
        rasters: AtomicUsize::new(0),
        native_keys: false,
    });
    let mut cx = HeadlessAppContext::with_platform(
        text.clone(),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let window = cx
        .open_window(size(px(64.), px(64.)), |_, cx| {
            cx.new(|_| ColorScene {
                font,
                glyph,
                color: Hsla::black(),
            })
        })
        .unwrap();
    let mut before_returning_to_red = 0;
    for (index, color) in [0xff0000, 0x00ff00, 0x0000ff, 0xffffff, 0xff0000]
        .into_iter()
        .enumerate()
    {
        if index == 4 {
            before_returning_to_red = text.rasters.load(Ordering::Relaxed);
        }
        cx.update_window(window.into(), |scene, window, cx| {
            scene
                .downcast::<ColorScene>()
                .unwrap()
                .update(cx, |scene, cx| {
                    scene.color = rgb(color).into();
                    cx.notify();
                });
            let _ = window.draw(cx);
            let image = window.render_to_image().unwrap();
            let expected = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
            assert!(image.pixels().any(|p| p.0[..3] == expected && p[3] == 255));
        })
        .unwrap();
    }
    assert_eq!(
        text.rasters.load(Ordering::Relaxed) - before_returning_to_red,
        1,
        "returning to red must upload again after the old variant retires"
    );
}

const COLOR: &[u8] = include_bytes!("../vendor/gpui-pre-wgpu/test-assets/Noto-COLRv1-subset.ttf");
fn system() -> CosmicTextSystem {
    let system = CosmicTextSystem::new_without_system_fonts("Geist");
    system
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../assets/fonts/Geist.ttf"
        ))])
        .unwrap();
    system
}
fn layout(system: &CosmicTextSystem, text: &str) -> gpui::LineLayout {
    let id = system.font_id(&gpui::font("Geist")).unwrap();
    system.layout_line(
        text,
        px(14.0),
        &[FontRun {
            len: text.len(),
            font_id: id,
        }],
    )
}
fn params(id: gpui::FontId, glyph: &gpui::ShapedGlyph) -> RenderGlyphParams {
    RenderGlyphParams {
        font_id: id,
        glyph_id: glyph.id,
        font_size: px(14.0),
        scale_factor: 1.0,
        subpixel_variant: point(0, 0),
        is_emoji: glyph.is_emoji,
        subpixel_rendering: false,
        dilation: 0,
        foreground: [0, 0, 0, 255],
    }
}

fn table_offset(font: &[u8], tag: &[u8; 4]) -> usize {
    let count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
    let at = (0..count)
        .map(|i| 12 + i * 16)
        .find(|i| &font[*i..*i + 4] == tag)
        .unwrap();
    u32::from_be_bytes(font[at + 8..at + 12].try_into().unwrap()) as usize
}

fn add_ascii_m_alias(font: &mut [u8]) {
    // Older backends reject explicitly named faces without ASCII m. Alias an
    // unused single-codepoint group so these tests exercise recovery itself.
    let cmap = table_offset(font, b"cmap");
    let count = u16::from_be_bytes(font[cmap + 2..cmap + 4].try_into().unwrap()) as usize;
    for i in 0..count {
        let at = cmap + 4 + i * 8;
        let sub = cmap + u32::from_be_bytes(font[at + 4..at + 8].try_into().unwrap()) as usize;
        if u16::from_be_bytes(font[sub..sub + 2].try_into().unwrap()) != 12 {
            continue;
        }
        let groups = u32::from_be_bytes(font[sub + 12..sub + 16].try_into().unwrap()) as usize;
        for group in 0..groups {
            let at = sub + 16 + group * 12;
            let start = u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
            let end = u32::from_be_bytes(font[at + 4..at + 8].try_into().unwrap());
            let previous = if group == 0 {
                0
            } else {
                u32::from_be_bytes(font[at - 8..at - 4].try_into().unwrap())
            };
            if start > 109 && start == end && previous < 109 {
                font[at..at + 4].copy_from_slice(&109u32.to_be_bytes());
                font[at + 4..at + 8].copy_from_slice(&109u32.to_be_bytes());
                break;
            }
        }
    }
}

fn rename_font(font: &mut [u8], old: &str, new: &str) {
    let old: Vec<_> = old.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let new: Vec<_> = new.encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert_eq!(old.len(), new.len());
    for i in 0..=font.len() - old.len() {
        if font[i..i + old.len()] == old {
            font[i..i + old.len()].copy_from_slice(&new);
        }
    }
}

fn broken_color() -> Vec<u8> {
    let mut font = COLOR.to_vec();
    let at = table_offset(&font, b"COLR");
    font[at..at + 2].copy_from_slice(&0u16.to_be_bytes());
    rename_font(&mut font, "Noto Color Emoji", "Fail Color Emoji");
    rename_font(&mut font, "NotoColorEmoji", "FailColorEmoji");
    add_ascii_m_alias(&mut font);
    font
}
fn next_emoji() -> Vec<u8> {
    let mut font = include_bytes!("../vendor/ghostty/src/font/res/NotoEmoji-Regular.ttf").to_vec();
    rename_font(&mut font, "Noto Emoji", "Zzzz Emoji");
    rename_font(&mut font, "NotoEmoji", "ZzzzEmoji");
    add_ascii_m_alias(&mut font);
    // Deliberately different advances and line metrics from the failed face.
    let hhea = table_offset(&font, b"hhea");
    font[hhea + 4..hhea + 6].copy_from_slice(&2500i16.to_be_bytes());
    let os2 = table_offset(&font, b"OS/2");
    font[os2 + 68..os2 + 70].copy_from_slice(&2500i16.to_be_bytes());
    font[os2 + 74..os2 + 76].copy_from_slice(&2500u16.to_be_bytes());
    let count = u16::from_be_bytes(font[hhea + 34..hhea + 36].try_into().unwrap()) as usize;
    let hmtx = table_offset(&font, b"hmtx");
    for i in 0..count {
        font[hmtx + i * 4..hmtx + i * 4 + 2].copy_from_slice(&3000u16.to_be_bytes());
    }
    font
}
fn configured_system(broken: bool) -> (CosmicTextSystem, gpui::FontId) {
    configured_system_with_fonts(broken, false)
}
fn configured_system_with_fonts(broken: bool, real: bool) -> (CosmicTextSystem, gpui::FontId) {
    let system = if real {
        let system = CosmicTextSystem::new("Geist");
        system
            .add_fonts(vec![Cow::Borrowed(include_bytes!(
                "../assets/fonts/Geist.ttf"
            ))])
            .unwrap();
        system
    } else {
        system()
    };
    let mut fonts = vec![
        Cow::Borrowed(
            include_bytes!("../vendor/ghostty/src/font/res/NotoEmoji-Regular.ttf").as_slice(),
        ),
        Cow::Owned(next_emoji()),
    ];
    if broken {
        fonts.push(Cow::Owned(broken_color()));
    }
    system.add_fonts(fonts).unwrap();
    let mut font = gpui::font("Geist");
    font.fallbacks = Some(gpui::FontFallbacks::from_fonts(if broken {
        vec!["Fail Color Emoji".into(), "Zzzz Emoji".into()]
    } else {
        vec!["Zzzz Emoji".into()]
    }));
    let id = system.font_id(&font).unwrap();
    system.font_id(&gpui::font("Zzzz Emoji")).unwrap();
    (system, id)
}
#[test]
fn recovery_keeps_validated_face_in_same_family() {
    let probe = include_bytes!("../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf");
    let mut broken = probe.to_vec();
    let at = table_offset(&broken, b"COLR");
    broken[at..at + 2].copy_from_slice(&0u16.to_be_bytes());
    let mut bold = probe.to_vec();
    rename_font(&mut bold, "COLRProbe", "BoldProbe");
    let os2 = table_offset(&bold, b"OS/2");
    bold[os2 + 4..os2 + 6].copy_from_slice(&700u16.to_be_bytes());
    let system = system();
    system
        .add_fonts(vec![Cow::Owned(broken), Cow::Owned(bold)])
        .unwrap();
    let id = system.font_id(&gpui::font("COLR Probe")).unwrap();
    for _ in 0..2 {
        let line = system.layout_line(
            "\u{e000}",
            px(28.0),
            &[FontRun {
                len: 3,
                font_id: id,
            }],
        );
        let run = &line.runs[0];
        assert_eq!(
            system.font_weight_and_style(run.font_id).unwrap().0.0,
            700.0
        );
        let p = params(run.font_id, &run.glyphs[0]);
        let bounds = system.glyph_raster_bounds(&p).unwrap();
        let (_, bytes) = system.rasterize_glyph(&p, bounds).unwrap();
        assert!(bytes.chunks_exact(4).any(|p| p[3] != 0));
    }
    assert!(
        !system
            .all_font_names()
            .iter()
            .any(|name| name.starts_with("__gpui_color_fallback_"))
    );
}

fn rejected_with_outline() -> Vec<u8> {
    include_bytes!("../vendor/gpui-pre-wgpu/test-assets/COLRRejected.ttf").to_vec()
}
#[test]
fn rejected_color_with_base_outline_returns_tinted_color_sprite() {
    let system = system();
    system
        .add_fonts(vec![Cow::Owned(rejected_with_outline())])
        .unwrap();
    let id = system.font_id(&gpui::font("COLR Probe")).unwrap();
    let line = system.layout_line(
        "\u{e000}",
        px(28.0),
        &[FontRun {
            len: 3,
            font_id: id,
        }],
    );
    let run = &line.runs[0];
    assert!(run.glyphs[0].is_emoji);
    let mut p = params(run.font_id, &run.glyphs[0]);
    p.foreground = [255, 0, 0, 128];
    let bounds = system.glyph_raster_bounds(&p).unwrap();
    let (_, bytes) = system.rasterize_glyph(&p, bounds).unwrap();
    assert_eq!(
        bytes.len(),
        (bounds.size.width.0 * bounds.size.height.0 * 4) as usize
    );
    assert!(bytes.chunks_exact(4).any(|p| p[3] > 100));
    assert!(
        bytes
            .chunks_exact(4)
            .all(|p| p[0] == 0 && p[1] == 0 && p[2] == p[3] && p[3] <= 128)
    );
}

#[test]
fn failed_color_cluster_relayouts_metrics_including_rtl() {
    // Only one drawable emoji face: old recovery chooses the right face but
    // still leaves the rejected face's advance/line metrics behind.
    let make = |broken| {
        let system = system();
        let mut fonts = vec![Cow::Owned(next_emoji())];
        if broken {
            fonts.push(Cow::Owned(broken_color()));
        }
        system.add_fonts(fonts).unwrap();
        let mut font = gpui::font("Geist");
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(if broken {
            vec!["Fail Color Emoji".into(), "Zzzz Emoji".into()]
        } else {
            vec!["Zzzz Emoji".into()]
        }));
        let id = system.font_id(&font).unwrap();
        (system, id)
    };
    let (actual, id) = make(true);
    let (expected, expected_id) = make(false);
    for text in ["A🚀B", "🚀A🚀B🚀", "A🚀\u{200d}🚀B", "אב🚀גד", "Aאב🚀גדB"]
    {
        let a = actual.layout_line(
            text,
            px(28.0),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        );
        let b = expected.layout_line(
            text,
            px(28.0),
            &[FontRun {
                len: text.len(),
                font_id: expected_id,
            }],
        );
        assert_eq!(a.width, b.width, "{text}");
        assert_eq!(a.ascent, b.ascent, "{text}");
        assert_eq!(a.descent, b.descent, "{text}");
        let ag: Vec<_> = a.runs.iter().flat_map(|r| &r.glyphs).collect();
        let bg: Vec<_> = b.runs.iter().flat_map(|r| &r.glyphs).collect();
        assert_eq!(ag.len(), bg.len());
        for (a, b) in ag.iter().zip(bg) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.position, b.position, "{text}");
        }
    }
}
#[test]
fn configured_next_family_wins_over_platform_fallback() {
    let (system, id) = configured_system(true);
    let line = system.layout_line(
        "🚀",
        px(28.0),
        &[FontRun {
            len: "🚀".len(),
            font_id: id,
        }],
    );
    let run = &line.runs[0];
    assert_eq!(
        system.advance(run.font_id, run.glyphs[0].id).unwrap().width,
        3000.0
    );
}

#[test]
fn repeated_failed_clusters_reuse_resolution_across_lines_and_sizes() {
    let (system, id) = configured_system_with_fonts(true, true);
    let shape = |text: &str, size| {
        system.layout_line(
            text,
            px(size),
            &[FontRun {
                len: text.len(),
                font_id: id,
            }],
        )
    };
    let begin = std::time::Instant::now();
    let single = shape("🚀", 14.0);
    let first = begin.elapsed();
    let repeated = "🚀".repeat(1000);
    let begin = std::time::Instant::now();
    let many = shape(&repeated, 28.0);
    let elapsed = begin.elapsed();
    assert!(many.width > single.width * 1000.0);
    // A generous relative bound tolerates noisy CI, but not 1000 database clones.
    assert!(
        elapsed < first * 20 + std::time::Duration::from_millis(100),
        "first={first:?}, repeated={elapsed:?}"
    );
}

#[test]
fn emoji_only_face_is_allowed_in_fallback_chain_not_as_primary() {
    let system = system();
    system.add_fonts(vec![Cow::Borrowed(COLOR)]).unwrap();
    assert!(system.font_id(&gpui::font("Noto Color Emoji")).is_err());
    let mut font = gpui::font("Geist");
    font.fallbacks = Some(gpui::FontFallbacks::from_fonts(vec![
        "Noto Color Emoji".into(),
    ]));
    let id = system.font_id(&font).unwrap();
    let line = system.layout_line(
        "🚀",
        px(14.0),
        &[FontRun {
            len: "🚀".len(),
            font_id: id,
        }],
    );
    assert!(
        line.runs
            .iter()
            .flat_map(|r| &r.glyphs)
            .all(|g| g.is_emoji && g.id != GlyphId(0))
    );
    assert!(system.font_id(&gpui::font("Noto Color Emoji")).is_err());
}

#[test]
fn valid_transparent_composite_does_not_trigger_font_replacement() {
    let system = system();
    system
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf"
        ))])
        .unwrap();
    let id = system.font_id(&gpui::font("COLR Probe")).unwrap();
    let text = "Ц";
    let line = system.layout_line(
        text,
        px(28.0),
        &[FontRun {
            len: text.len(),
            font_id: id,
        }],
    );
    assert_eq!(line.runs.len(), 1);
    assert_eq!(line.runs[0].font_id, id);
    let glyph = &line.runs[0].glyphs[0];
    assert!(glyph.is_emoji);
    let mut p = params(id, glyph);
    p.font_size = px(28.0);
    let bounds = system.glyph_raster_bounds(&p).unwrap();
    assert_eq!(bounds.size.width.0, 0);
    assert_eq!(bounds.size.height.0, 0);
}

#[test]
fn foreground_color_reaches_color_sprite_and_its_cache_key() {
    let probe = include_bytes!("../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf");
    assert!(colr::uses_foreground(probe, 0, 5));
    assert!(!colr::uses_foreground(probe, 0, 2));
    let system = system();
    system
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../vendor/gpui-pre-wgpu/test-assets/COLRProbe.ttf"
        ))])
        .unwrap();
    let id = system.font_id(&gpui::font("COLR Probe")).unwrap();
    let text = "\u{e000}";
    let line = system.layout_line(
        text,
        px(28.0),
        &[FontRun {
            len: text.len(),
            font_id: id,
        }],
    );
    let run = &line.runs[0];
    let glyph = &run.glyphs[0];
    assert!(glyph.is_emoji);
    let mut p = params(run.font_id, glyph);
    p.font_size = px(28.0);
    let mut results = Vec::new();
    for color in [[255; 4], [0, 0, 0, 255], [255, 0, 0, 128]] {
        p.foreground = color;
        let bounds = system.glyph_raster_bounds(&p).unwrap();
        let (_, bytes) = system.rasterize_glyph(&p, bounds).unwrap();
        assert!(bytes.chunks_exact(4).any(|p| p[3] > 100));
        if color == [255; 4] {
            assert!(
                bytes
                    .chunks_exact(4)
                    .any(|p| p[0] == 255 && p[1] == 255 && p[2] == 255)
            );
        }
        if color == [0, 0, 0, 255] {
            assert!(bytes.chunks_exact(4).all(|p| p[..3] == [0; 3]));
        }
        if color[3] == 128 {
            assert!(
                bytes
                    .chunks_exact(4)
                    .all(|p| p[3] <= 128 && p[0] == 0 && p[1] == 0)
            );
        }
        results.push(bytes);
    }
    assert_ne!(results[0], results[1]);
}

#[test]
fn empty_color_font_falls_back_without_changing_text_or_spaces() {
    let mut broken = COLOR.to_vec();
    // Keep valid cmap IDs and empty outline roots, but hide v1 paints behind
    // the font's empty v0 directory: the original swash failure mode.
    let count = u16::from_be_bytes([broken[4], broken[5]]) as usize;
    for i in 0..count {
        let entry = 12 + i * 16;
        if &broken[entry..entry + 4] == b"COLR" {
            let offset =
                u32::from_be_bytes(broken[entry + 8..entry + 12].try_into().unwrap()) as usize;
            broken[offset..offset + 2].copy_from_slice(&0u16.to_be_bytes());
        }
    }
    let system = system();
    let baseline = layout(&system, "ASCII  text");
    system
        .add_fonts(vec![
            Cow::Owned(broken),
            Cow::Borrowed(include_bytes!(
                "../vendor/ghostty/src/font/res/NotoEmoji-Regular.ttf"
            )),
        ])
        .unwrap();
    let emoji = layout(&system, "🚀");
    assert!(!emoji.runs.is_empty());
    for run in &emoji.runs {
        for glyph in &run.glyphs {
            assert!(!glyph.is_emoji, "must use drawable monochrome fallback");
            assert_ne!(glyph.id, GlyphId(0));
            let params = params(run.font_id, glyph);
            let bounds = system.glyph_raster_bounds(&params).unwrap();
            assert!(bounds.size.width.0 > 0 && bounds.size.height.0 > 0);
            let (_, bytes) = system.rasterize_glyph(&params, bounds).unwrap();
            assert!(bytes.iter().any(|b| *b != 0));
        }
    }
    let text = layout(&system, "ASCII  text");
    assert_eq!(text.width, baseline.width);
    assert_eq!(text.runs.len(), baseline.runs.len());
    for (a, b) in text.runs.iter().zip(&baseline.runs) {
        assert_eq!(a.font_id, b.font_id);
        assert_eq!(a.glyphs.len(), b.glyphs.len());
        for (a, b) in a.glyphs.iter().zip(&b.glyphs) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.position, b.position);
        }
    }
}
#[test]
fn colrv1_color_is_detected_by_glyph_not_postscript_name() {
    let mut font = COLOR.to_vec();
    let old: Vec<u8> = "NotoColorEmoji"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let new: Vec<u8> = "TestColorEmoji"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    assert_eq!(old.len(), new.len());
    let mut changed = false;
    for i in 0..=font.len() - old.len() {
        if font[i..i + old.len()] == old {
            font[i..i + old.len()].copy_from_slice(&new);
            changed = true;
        }
    }
    assert!(changed);
    let system = system();
    system.add_fonts(vec![Cow::Owned(font)]).unwrap();
    let text = layout(&system, "🚀");
    assert!(!text.runs.is_empty());
    for run in &text.runs {
        for glyph in &run.glyphs {
            assert!(glyph.is_emoji);
            let params = params(run.font_id, glyph);
            let bounds = system.glyph_raster_bounds(&params).unwrap();
            assert!(bounds.size.width.0 > 0 && bounds.size.height.0 > 0);
            let (_, bytes) = system.rasterize_glyph(&params, bounds).unwrap();
            assert_eq!(
                bytes.len(),
                (bounds.size.width.0 * bounds.size.height.0 * 4) as usize
            );
        }
    }
}
#[test]
fn drawable_colrv1_stays_color_and_uploads_premultiplied_bgra() {
    let system = system();
    system.add_fonts(vec![Cow::Borrowed(COLOR)]).unwrap();
    let text = layout(&system, "🚀🔥🎉✅🐕🌈");
    assert!(!text.runs.is_empty());
    for run in &text.runs {
        for glyph in &run.glyphs {
            assert!(glyph.is_emoji);
            let params = params(run.font_id, glyph);
            let bounds = system.glyph_raster_bounds(&params).unwrap();
            assert!(bounds.size.width.0 > 0 && bounds.size.height.0 > 0);
            let (_, bytes) = system.rasterize_glyph(&params, bounds).unwrap();
            assert_eq!(
                bytes.len(),
                (bounds.size.width.0 * bounds.size.height.0 * 4) as usize
            );
            let expected = colr::render(COLOR, 0, glyph.id.0, 14.0).unwrap();
            assert_eq!(bounds.origin.x.0, expected.placement.left);
            assert_eq!(bounds.origin.y.0, -expected.placement.top);
            for (bgra, rgba) in bytes.chunks_exact(4).zip(expected.data.chunks_exact(4)) {
                assert_eq!(bgra, &[rgba[2], rgba[1], rgba[0], rgba[3]]);
            }
            for p in bytes.chunks_exact(4) {
                assert!(p[..3].iter().all(|v| *v <= p[3]));
            }
            assert!(bytes.chunks_exact(4).any(|p| p[0] != p[1] || p[1] != p[2]));
        }
    }
}

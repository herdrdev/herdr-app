#![cfg(target_os = "linux")]

use gpui::{
    AppContext, Context, Font, FontId, FontStyle, FontWeight, HeadlessAppContext, IntoElement,
    ParentElement, PlatformTextSystem, Render, RenderGlyphParams, Styled, TextAlign, TextRun,
    Window, black,
    canvas, div, font, point, px, rgb, size,
};
use gpui_wgpu::CosmicTextSystem;
use std::{borrow::Cow, sync::Arc};

static GPU_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");

fn text_system() -> CosmicTextSystem {
    let text_system = CosmicTextSystem::new_without_system_fonts("IBM Plex Sans");
    text_system
        .add_fonts(vec![Cow::Borrowed(JETBRAINS_MONO)])
        .unwrap();
    text_system
}

fn raster_coverage(
    text_system: &CosmicTextSystem,
    font_id: FontId,
) -> anyhow::Result<u64> {
    let glyph_id = text_system
        .glyph_for_char(font_id, 'M')
        .expect("missing 'M' glyph");
    let params = RenderGlyphParams {
        font_id,
        glyph_id,
        font_size: px(20.0),
        subpixel_variant: point(0, 0),
        scale_factor: 1.0,
        is_emoji: false,
        subpixel_rendering: false,
        dilation: 0,
        foreground: [0, 0, 0, 255],
    };
    let bounds = text_system.glyph_raster_bounds(&params)?;
    let (_size, data) = text_system.rasterize_glyph(&params, bounds)?;
    Ok(data.iter().map(|&byte| u64::from(byte)).sum())
}

/// A variable font ships one face in fontdb, so every requested weight
/// resolves to the same face. The weight must still reach the rasterizer as a
/// `wght` variation or terminal bold renders identical to normal.
#[test]
fn variable_font_weight_reaches_the_rasterizer() -> anyhow::Result<()> {
    let text_system = text_system();
    let regular = Font {
        weight: FontWeight::MEDIUM,
        ..font("JetBrains Mono")
    };
    let bold = Font {
        weight: FontWeight::BOLD,
        ..regular.clone()
    };
    let regular_id = text_system.font_id(&regular)?;
    let bold_id = text_system.font_id(&bold)?;
    assert_ne!(regular_id, bold_id);
    assert_eq!(
        text_system.font_weight_and_style(bold_id)?,
        (FontWeight::BOLD, FontStyle::Normal)
    );

    let regular_coverage = raster_coverage(&text_system, regular_id)?;
    let bold_coverage = raster_coverage(&text_system, bold_id)?;
    // wght 700 strokes cover ~25% more pixels than wght 500.
    assert!(
        bold_coverage > regular_coverage * 11 / 10,
        "bold coverage {bold_coverage} should exceed regular {regular_coverage} by ~20%"
    );
    Ok(())
}

struct WeightScene {
    medium: Font,
    bold: Font,
}
impl Render for WeightScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (medium, bold) = (self.medium.clone(), self.bold.clone());
        div().size_full().bg(rgb(0xffffff)).child(
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    for (top, font) in [(px(20.), medium.clone()), (px(90.), bold.clone())] {
                        let line = window.text_system().shape_line(
                            "MMMM".into(),
                            px(30.),
                            &[TextRun {
                                len: 4,
                                font,
                                color: black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        );
                        line.paint(
                            point(px(20.), top),
                            px(50.),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .unwrap();
                    }
                },
            )
            .size_full(),
        )
    }
}

/// Same text, two `TextRun`s: the rendered frame must show more ink in the
/// bold half. This is what `printf '\e[1mBOLD\e[0m NORMAL'` looks like.
#[test]
fn bold_and_normal_render_different_pixels() -> anyhow::Result<()> {
    let _gpu = GPU_TEST.lock().unwrap();
    let text_system = Arc::new(text_system());
    let medium = Font {
        weight: FontWeight::MEDIUM,
        ..font("JetBrains Mono")
    };
    let bold = Font {
        weight: FontWeight::BOLD,
        ..font("JetBrains Mono")
    };
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let window = cx
        .open_window(size(px(400.), px(160.)), |_, cx| {
            cx.new(|_| WeightScene { medium, bold })
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let image = window.render_to_image().unwrap();
        let (width, height) = image.dimensions();
        let ink = |rows: std::ops::Range<u32>| -> u64 {
            let mut total = 0;
            for y in rows {
                for x in 0..width {
                    total += u64::from(255 - image.get_pixel(x, y).0[0]);
                }
            }
            total
        };
        let medium_ink = ink(0..height / 2);
        let bold_ink = ink(height / 2..height);
        assert!(
            bold_ink > medium_ink * 11 / 10,
            "bold half of the frame should carry ~20% more ink: medium {medium_ink}, bold {bold_ink}"
        );
    })
    .unwrap();
    Ok(())
}

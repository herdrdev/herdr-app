#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
//! Real CoreText rasterising and GPU readback: font_thicken's strength
//! thickens dark text on light as much as light text on dark (Ghostty's
//! `font-thicken`), where macOS's own smoothing thickens by text color.
//! Prints each case's ink: the mean of (pixel - background) / (text -
//! background) over the text's box, so both polarities compare directly.
use gpui::{prelude::*, *};

const TEXT: &str = "the quick brown fox jumps over the lazy dog 0123456789";
const DARK: u32 = 0x1b1b1e;
const LIGHT: u32 = 0xe4e4e8;
// Rows: (font weight, thicken, text, background). Ghostty on the LG display
// draws Regular (400) with font-thicken-strength 30.
const CASES: [(f32, Option<u8>, u32, u32); 6] = [
    (400., None, LIGHT, DARK),
    (400., None, DARK, LIGHT),
    (400., Some(30), LIGHT, DARK),
    (400., Some(30), DARK, LIGHT),
    (400., Some(255), LIGHT, DARK),
    (400., Some(255), DARK, LIGHT),
];
const ROW: f32 = 18.;
const WIDTH: f32 = 220.;

struct Scene {
    font_size: f32,
}

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let font_size = self.font_size;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for (i, (weight, thicken, fg, bg)) in CASES.into_iter().enumerate() {
                    let top = point(px(0.), px(ROW * i as f32));
                    window.paint_quad(fill(Bounds::new(top, size(px(WIDTH), px(ROW))), rgb(bg)));
                    let mut font = font("JetBrains Mono");
                    font.weight = FontWeight(weight);
                    let run = TextRun { len: TEXT.len(), font, color: rgb(fg).into(), background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(TEXT.into(), px(font_size), &[run], None);
                    window.with_glyph_thicken(thicken, |window| {
                        line.paint(top + point(px(2.), px(0.)), px(ROW), TextAlign::Left, None, window, cx).unwrap();
                    });
                }
            },
        )
        .size_full()
    }
}

fn ink(image: &image::RgbaImage, scale: f32, i: usize, fg: u32, bg: u32) -> f32 {
    let channel = |c: u32, k: u32| ((c >> (16 - 8 * k)) & 0xff) as f32;
    let (y0, y1) = ((ROW * i as f32 * scale) as u32, (ROW * (i + 1) as f32 * scale) as u32);
    let (mut sum, mut n) = (0.0, 0);
    for y in y0..y1 {
        for x in 0..(WIDTH * scale) as u32 {
            let p = image.get_pixel(x, y).0;
            let mean = (0..3).map(|k| (p[k as usize] as f32 - channel(bg, k)) / (channel(fg, k) - channel(bg, k))).sum::<f32>() / 3.0;
            sum += mean.clamp(0.0, 1.0);
            n += 1;
        }
    }
    sum / n as f32
}

fn check(window: &mut Window, cx: &mut App) {
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let inks: Vec<f32> = CASES.iter().enumerate().map(|(i, (_, _, fg, bg))| ink(&image, scale, i, *fg, *bg)).collect();
    for ((weight, thicken, fg, _), ink) in CASES.iter().zip(&inks) {
        let polarity = if *fg == DARK { "dark on light" } else { "light on dark" };
        println!("scale {scale} weight {weight} thicken {thicken:?} {polarity}: ink {ink:.4}");
    }
    let [off_light, off_dark, t30_light, t30_dark, t255_light, t255_dark] = inks[..] else { unreachable!() };
    assert!(off_light > off_dark * 1.05, "macOS smoothing thickens light text more ({off_light} vs {off_dark})");
    // One mask, tinted: thickened polarities differ only by rounding.
    for (light, dark) in [(t30_light, t30_dark), (t255_light, t255_dark)] {
        assert!((light - dark).abs() < light * 0.01, "thickened polarities match: {light} vs {dark}");
    }
    assert!(t255_dark > t30_dark * 1.05, "strength thickens: {t30_dark} to {t255_dark}");
}

fn add_font(cx: &mut App) {
    let bytes: &'static [u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");
    cx.text_system().add_fonts(vec![bytes.into()]).unwrap();
}

#[cfg(target_os = "macos")]
fn headless() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(add_font);
    // The test window is 2x: half of font_size 12 draws as 12 device pixels,
    // as on a 1x display.
    let window = cx.open_window(size(px(WIDTH), px(ROW * CASES.len() as f32)), |_, cx| cx.new(|_| Scene { font_size: 6. })).unwrap();
    cx.update_window(window.into(), |_, window, cx| check(window, cx)).unwrap();
}

#[cfg(target_os = "macos")]
fn native() {
    gpui_platform::application().run(|cx| {
        add_font(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(WIDTH), px(ROW * CASES.len() as f32))))),
            show: false,
            focus: false,
            titlebar: None,
            ..Default::default()
        };
        let window = cx.open_window(options, |_, cx| cx.new(|_| Scene { font_size: 12. })).unwrap();
        cx.spawn(async move |cx| {
            cx.update_window(window.into(), |_, window, cx| check(window, cx)).unwrap();
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    #[cfg(target_os = "macos")]
    if std::env::var_os("HERDR_SHOT_NATIVE_TEST").is_some() {
        native();
    } else {
        headless();
    }
}

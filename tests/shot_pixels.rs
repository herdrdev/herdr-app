//! Real GPU readback, not a mocked image. Linux CI uses Vulkan software rendering.
use gpui::{prelude::*, *};

#[path = "../src/sprites.rs"]
#[allow(unused_imports)]
mod sprites;

struct Scene;
impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x123456))
            .child(
                div()
                    .absolute()
                    .left(px(4.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0xff0000)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(24.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x00ff00)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(44.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x0000ff)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(64.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x808080)),
            )
            // Later paint must win over the red square.
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .top(px(8.))
                    .size(px(4.))
                    .bg(rgb(0xf08020)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    |_, _, window, _| {
                        for (x, width) in [(110., 9.), (119., 9.5)] {
                            for row in 0..2 {
                                sprites::paint(
                                    "⡇",
                                    Bounds::new(
                                        point(px(x), px(4. + row as f32 * 24.)),
                                        size(px(width), px(24.)),
                                    ),
                                    rgb(0xffffff).into(),
                                    window,
                                );
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

fn check(window: &mut Window, cx: &mut App) {
    assert!(sprites::is_sprite("⡇"));
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let viewport = window.viewport_size();
    assert_eq!(
        image.dimensions(),
        (
            (f32::from(viewport.width) * scale).round() as u32,
            (f32::from(viewport.height) * scale).round() as u32,
        )
    );
    for (x, y, expected) in [
        (2., 2., [0x12, 0x34, 0x56, 255]),
        (6., 6., [255, 0, 0, 255]),
        (26., 6., [0, 255, 0, 255]),
        (46., 6., [0, 0, 255, 255]),
        (66., 6., [128, 128, 128, 255]),
        (10., 10., [240, 128, 32, 255]),
        (100., 40., [0x12, 0x34, 0x56, 255]),
    ] {
        assert_eq!(
            image.get_pixel((x * scale) as u32, (y * scale) as u32).0,
            expected,
            "pixel ({x}, {y}) at scale {scale}"
        );
    }
    // The four dots in each Braille row should keep the same spacing across rows.
    for x in [112., 121.5] {
        for dot in 0..8 {
            let x = (x * scale) as u32;
            let y = ((7. + dot as f32 * 6.) * scale) as u32;
            assert_eq!(image.get_pixel(x, y).0, [255, 255, 255, 255], "dot {dot}");
            if dot < 7 {
                let gap = ((10. + dot as f32 * 6.) * scale) as u32;
                assert_eq!(
                    image.get_pixel(x, gap).0,
                    [0x12, 0x34, 0x56, 255],
                    "gap after dot {dot}"
                );
            }
        }
    }
    // Capture must not consume the frame or change the next result.
    assert_eq!(image, window.render_to_image().unwrap());
    println!(
        "shot pixel test passed: {}x{}, scale {scale}",
        image.width(),
        image.height()
    );
}

#[cfg(not(windows))]
fn headless() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    markdown_selection_pixels(&mut cx);
    // TestPlatform uses 2x HiDPI; these physical widths force row padding.
    for width in [129., 131.] {
        let window = cx
            .open_window(size(px(width), px(54.)), |_, cx| cx.new(|_| Scene))
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| check(window, cx))
            .unwrap();
    }
}

#[cfg(not(windows))]
fn markdown_selection_pixels(cx: &mut HeadlessAppContext) {
    use gpui_base::{TextSelectionLayer, TextView, TextViewState, TextViewStyle};

    struct MarkdownScene {
        text: Entity<TextViewState>,
    }
    impl Render for MarkdownScene {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(rgb(0x000000))
                .text_size(px(24.))
                .child(TextSelectionLayer)
                .child(
                    TextView::new(&self.text)
                        .scrollable(false)
                        .selectable(true)
                        .style(
                            TextViewStyle::default()
                                .with_foreground(rgb(0xffffff).into())
                                .with_selection(rgb(0xff0000).into()),
                        ),
                )
        }
    }

    let window = cx
        .open_window(size(px(320.), px(100.)), |_, cx| {
            gpui_base::init(cx);
            let text = cx.new(|cx| TextViewState::markdown("Selected glyphs stay visible", cx));
            cx.new(|_| MarkdownScene { text })
        })
        .unwrap();
    cx.update_window(window.into(), |root, window, cx| {
        let _ = window.draw(cx);
        let before = window.render_to_image().unwrap();
        root.downcast::<MarkdownScene>().unwrap().update(cx, |root, cx| {
            root.text.update(cx, |text, cx| text.select_all(cx));
        });
        let _ = window.draw(cx);
        let after = window.render_to_image().unwrap();
        let white = |image: &image::RgbaImage| {
            image.pixels().filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240).count()
        };
        let unselected_glyphs = white(&before);
        let selected_glyphs = white(&after);
        println!("markdown white glyph pixels: unselected={unselected_glyphs}, selected={selected_glyphs}");
        assert!(unselected_glyphs > 100, "fixture must render visible glyphs");
        assert!(after.pixels().any(|p| p[0] == 255 && p[1] == 0 && p[2] == 0),
            "fixture must paint a selection");
        assert!(selected_glyphs >= unselected_glyphs * 9 / 10,
            "selection must be behind glyphs");
    }).unwrap();
}

fn native() {
    gpui_platform::application().run(|cx| {
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(259.), px(107.)),
                    ))),
                    show: false,
                    focus: false,
                    titlebar: None,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Scene),
            )
            .unwrap();
        cx.spawn(async move |cx| {
            cx.update_window(window.into(), |_, window, cx| check(window, cx))
                .unwrap();
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    #[cfg(not(windows))]
    if std::env::var_os("HERDR_SHOT_NATIVE_TEST").is_none() {
        return headless();
    }
    native();
}

use core_foundation::base::TCFType;
use core_video::pixel_buffer::CVPixelBuffer;
use ghostty_sys::*;
use gpui::{Bounds, Pixels, Size, Window, px, size};
use std::{
    ffi::{CStr, CString},
    ptr::NonNull,
    rc::Rc,
};

#[derive(PartialEq)]
struct Configuration {
    font: String,
    fonts_added: u64,
    points: f32,
    scale: f32,
    ligatures: bool,
    contrast: f32,
    opacity: f32,
    theme: crate::vt::Theme,
}

thread_local! {
    static CONFIGURATION: std::cell::RefCell<Option<(Rc<crate::settings::Settings>, Rc<Configuration>)>> = const { std::cell::RefCell::new(None) };
    static METRICS: std::cell::RefCell<Option<(Rc<Configuration>, Size<Pixels>)>> = const { std::cell::RefCell::new(None) };
}
fn configuration(window: &Window) -> Rc<Configuration> {
    let settings = crate::settings::get();
    let fonts_added = crate::settings::fonts_added();
    let scale = window.scale_factor();
    CONFIGURATION.with_borrow_mut(|cached| {
        // settings::set replaces this Rc when either settings or the resolved
        // theme changes; font discovery and display scale change independently.
        if let Some((source, value)) = cached
            && Rc::ptr_eq(source, &settings)
            && value.fonts_added == fonts_added
            && value.scale == scale
        {
            return value.clone();
        }
        let value = Rc::new(Configuration {
            font: crate::settings::terminal_font(),
            fonts_added,
            points: crate::settings::font_size(),
            scale,
            ligatures: settings.font_ligatures,
            contrast: settings.minimum_contrast,
            opacity: settings.terminal_opacity,
            theme: crate::vt::theme(),
        });
        *cached = Some((settings, value.clone()));
        value
    })
}
pub(super) fn initial_cell_size(window: &Window) -> Option<Size<Pixels>> {
    if crate::settings::get().terminal_renderer != crate::settings::TerminalRenderer::Ghostty {
        return None;
    }
    let configuration = configuration(window);
    if let Some(cell) = METRICS.with(|cached| {
        cached
            .borrow()
            .as_ref()
            .filter(|(key, _)| *key == configuration)
            .map(|(_, cell)| *cell)
    }) {
        return Some(cell);
    }
    Renderer::default().prepare(window)
}

struct Native(NonNull<HerdrRenderer>);
impl Native {
    fn new(configuration: &Configuration) -> Result<Self, String> {
        let hex = |c: crate::vt::Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        let mut args = vec![
            format!("--font-family={}", configuration.font),
            format!("--minimum-contrast={}", configuration.contrast),
            format!("--background-opacity={}", configuration.opacity),
            format!("--foreground={}", hex(configuration.theme.fg)),
            format!("--background={}", hex(configuration.theme.bg)),
            format!("--cursor-color={}", hex(configuration.theme.cursor)),
            format!(
                "--selection-background={}",
                hex(configuration.theme.selection_bg)
            ),
            "--window-colorspace=srgb".into(),
            "--scroll-to-bottom=false".into(),
            "--cursor-style-blink=false".into(),
            "--font-thicken=false".into(),
        ];
        if !configuration.ligatures {
            for feature in ["-liga", "-clig", "-calt"] {
                args.push(format!("--font-feature={feature}"));
            }
        }
        let args = args
            .iter()
            .map(|s| CString::new(s.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "font settings contain a NUL character".to_owned())?;
        let family = CString::new(configuration.font.as_str())
            .map_err(|_| "font settings contain a NUL character".to_owned())?;
        if !unsafe { herdr_renderer_has_font(family.as_ptr()) } {
            tracing::warn!(font = %configuration.font, "font is unavailable to CoreText; Ghostty will use a fallback");
        }
        let pointers: Vec<_> = args.iter().map(|s| s.as_ptr()).collect();
        NonNull::new(unsafe {
            herdr_renderer_new(
                pointers.as_ptr(),
                pointers.len(),
                configuration.points,
                configuration.scale.into(),
            )
        })
        .map(Self)
        .ok_or_else(last_error)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { herdr_renderer_free(self.0.as_ptr()) };
    }
}

#[derive(Default)]
pub(super) struct Renderer {
    configuration: Option<Rc<Configuration>>,
    native: Option<Native>,
    frame: Option<CVPixelBuffer>,
    focused: Option<bool>,
}

impl Renderer {
    pub fn active(&self) -> bool {
        self.native.is_some()
    }
    pub fn prepare(&mut self, window: &Window) -> Option<Size<Pixels>> {
        let settings = crate::settings::get();
        if settings.terminal_renderer != crate::settings::TerminalRenderer::Ghostty {
            self.native = None;
            self.frame = None;
            self.configuration = None;
            return None;
        }
        let configuration = configuration(window);
        if self.configuration.as_ref() != Some(&configuration) {
            self.native = None;
            self.frame = None;
            self.native = match Native::new(&configuration) {
                Ok(native) => Some(native),
                Err(reason) => {
                    tracing::error!(%reason, "Ghostty renderer unavailable; using GPUI for this terminal");
                    None
                }
            };
            self.configuration = Some(configuration.clone());
        }
        let native = self.native.as_ref()?;
        let cell = unsafe { herdr_renderer_cell_size(native.0.as_ptr()) };
        let scale = window.scale_factor();
        let cell = size(
            px(cell.width as f32 / scale),
            px(cell.height as f32 / scale),
        );
        METRICS.with_borrow_mut(|cached| {
            if cached.as_ref().is_none_or(|(key, _)| key != &configuration) {
                *cached = Some((configuration, cell));
            }
        });
        Some(cell)
    }

    pub fn draw(
        &mut self,
        terminal: GhosttyTerminal,
        snapshot: GhosttyRenderState,
        focused: bool,
        full: bool,
        changed: bool,
    ) -> bool {
        let Some(native) = &self.native else {
            return false;
        };
        // UI-only redraws can reuse the completed GPU frame.
        if !changed && !full && self.focused == Some(focused) && self.frame.is_some() {
            return true;
        }
        let frame =
            unsafe { herdr_renderer_draw(native.0.as_ptr(), terminal, snapshot, focused, full) };
        if frame.is_null() {
            self.native = None;
            self.frame = None;
            tracing::error!(reason = %last_error(), "Ghostty renderer failed; using GPUI for this terminal");
            return false;
        }
        self.frame = Some(unsafe { CVPixelBuffer::wrap_under_create_rule(frame.cast()) });
        self.focused = Some(focused);
        true
    }

    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        if let Some(frame) = &self.frame {
            window.paint_surface(bounds, frame.clone());
        }
    }
}

fn last_error() -> String {
    unsafe {
        CStr::from_ptr(herdr_renderer_last_error())
            .to_string_lossy()
            .into_owned()
    }
}

pub(super) fn register_fonts(fonts: &[std::borrow::Cow<'_, [u8]>]) {
    for bytes in fonts {
        let mut message = [0i8; 1024];
        if !unsafe {
            herdr_renderer_register_font(
                bytes.as_ptr(),
                bytes.len(),
                message.as_mut_ptr(),
                message.len(),
            )
        } {
            let reason = unsafe { CStr::from_ptr(message.as_ptr()) }.to_string_lossy();
            tracing::warn!(%reason, "font unavailable to Ghostty's CoreText discovery");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn configuration_cache_follows_live_settings(cx: &mut gpui::TestAppContext) {
        let previous = crate::settings::get();
        let mut settings = crate::settings::Settings::default();
        crate::settings::set(settings.clone());
        let window = cx.add_window(|_, _| gpui::Empty);
        window
            .update(cx, |_, window, _| {
                let initial = configuration(window);
                assert!(Rc::ptr_eq(&initial, &configuration(window)));

                settings.font_size += 1.;
                crate::settings::set(settings.clone());
                let resized = configuration(window);
                assert_eq!(resized.points, initial.points + 1.);

                settings
                    .colors
                    .insert("terminal_foreground".into(), "#123456".into());
                crate::settings::set(settings.clone());
                let recolored = configuration(window);
                assert_eq!(recolored.theme.fg, crate::vt::Rgb(0x12, 0x34, 0x56));
                assert!(Rc::ptr_eq(&recolored, &configuration(window)));

                settings.ui_font_size += 1.;
                crate::settings::set(settings.clone());
                assert!(
                    configuration(window) == recolored,
                    "interface changes must not recreate the terminal renderer"
                );
            })
            .unwrap();
        crate::settings::set((*previous).clone());
    }
}

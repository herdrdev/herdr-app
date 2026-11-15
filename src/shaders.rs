//! Custom shaders, Ghostty style: a layout root's `"shaders": [paths]`, each a
//! Shadertoy-style `mainImage(out vec4, in vec2)` that reads the frame from
//! `iChannel0`. They run over the whole window after every frame, in order,
//! with the uniforms Ghostty gives its custom shaders, so the same files work
//! in both.

use gpui::{Bounds, Pixels, ShaderValue, Window};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// Ghostty's uniforms, the frame as `iChannel0`, and GLSL's bottom-left origin:
/// the backend's `gl_FragCoord` and texture coordinates are top-left.
const PRELUDE: &str = r#"#version 450
layout(set = 0, binding = 0) uniform texture2D herdrFrame;
layout(set = 0, binding = 1) uniform sampler herdrSampler;
layout(set = 0, binding = 2) uniform Globals {
    vec3 iResolution;
    float iTime;
    float iTimeDelta;
    float iFrameRate;
    int iFrame;
    float iChannelTime[4];
    vec3 iChannelResolution[4];
    vec4 iMouse;
    vec4 iDate;
    float iSampleRate;
    vec4 iCurrentCursor;
    vec4 iPreviousCursor;
    vec4 iCurrentCursorColor;
    vec4 iPreviousCursorColor;
    int iCurrentCursorStyle;
    int iPreviousCursorStyle;
    int iCursorVisible;
    float iTimeCursorChange;
    int iFocus;
    float iTimeFocus;
    vec3 iPalette[256];
    vec3 iBackgroundColor;
    vec3 iForegroundColor;
    vec3 iCursorColor;
    vec3 iCursorText;
    vec3 iSelectionBackgroundColor;
    vec3 iSelectionForegroundColor;
};
#define iChannel0 sampler2D(herdrFrame, herdrSampler)
vec4 herdrSample(vec2 uv) { return texture(sampler2D(herdrFrame, herdrSampler), vec2(uv.x, 1.0 - uv.y)); }
#define texture(s, uv) herdrSample(uv)
layout(location = 0) out vec4 herdrColor;
"#;

const EPILOGUE: &str = r#"
void main() {
    mainImage(herdrColor, vec2(gl_FragCoord.x, iResolution.y - gl_FragCoord.y));
}
"#;

/// A shader file as the backend takes it: the prelude, the file, a `main`.
pub fn source(file: &str) -> String {
    // The prelude sets the version; a file's own would come too late.
    let body: String = file.lines().map(|l| if l.trim_start().starts_with("#version") { "" } else { l }).collect::<Vec<_>>().join("\n");
    format!("{PRELUDE}#line 1\n{body}\n{EPILOGUE}")
}

/// `~/x` from home, a relative path from the app's folder.
fn resolve(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return crate::mods::home().join(rest);
    }
    let p = Path::new(path);
    if p.is_absolute() { p.to_owned() } else { crate::mods::app_dir().join(p) }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub struct Shaders {
    /// What the window runs now: each file and the time it was read at.
    loaded: Vec<(PathBuf, Option<SystemTime>)>,
    started: Instant,
    last_frame: Instant,
    frame: i32,
    checked: Option<Instant>,
    cursor: Option<[f32; 4]>,
    previous: [f32; 4],
    cursor_changed: f32,
    focused: Option<bool>,
    focus_changed: f32,
}

impl Default for Shaders {
    fn default() -> Self {
        let now = Instant::now();
        Shaders { loaded: Vec::new(), started: now, last_frame: now, frame: 0, checked: None, cursor: None, previous: [0.0; 4], cursor_changed: -10.0, focused: None, focus_changed: -10.0 }
    }
}

impl Shaders {
    pub fn active(&self) -> bool {
        !self.loaded.is_empty()
    }

    /// Loads the layout's shaders when the list or a file changes (files are
    /// looked at once a second). Returns why loading failed, once per change.
    pub fn sync(&mut self, paths: &[String], window: &Window) -> Option<String> {
        let wanted: Vec<PathBuf> = paths.iter().map(|p| resolve(p)).collect();
        let same_list = wanted.len() == self.loaded.len() && wanted.iter().zip(&self.loaded).all(|(a, (b, _))| a == b);
        if same_list {
            if self.checked.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0) {
                return None;
            }
            self.checked = Some(Instant::now());
            if self.loaded.iter().all(|(p, at)| modified(p) == *at) {
                return None;
            }
        }
        self.checked = Some(Instant::now());
        let stamps: Vec<_> = wanted.iter().map(|p| (p.clone(), modified(p))).collect();
        let mut sources = Vec::new();
        for path in &wanted {
            match std::fs::read_to_string(path) {
                Ok(text) => sources.push(source(&text)),
                Err(e) => {
                    self.loaded = stamps;
                    let _ = window.set_post_process_shaders(&[]);
                    return Some(format!("{}: {e}", path.display()));
                }
            }
        }
        // Keep the stamps either way: a broken file is reported once, not every second.
        self.loaded = stamps;
        if let Err(e) = window.set_post_process_shaders(&sources) {
            let _ = window.set_post_process_shaders(&[]);
            return Some(format!("{e:#}"));
        }
        self.started = Instant::now();
        self.frame = 0;
        None
    }

    /// This frame's uniforms. `cursor` is the focused terminal's cursor cell in
    /// window coordinates. Keeps frames coming while the window has focus.
    pub fn frame(&mut self, window: &mut Window, cursor: Option<Bounds<Pixels>>) {
        if !self.active() {
            return;
        }
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let (w, h) = (f32::from(viewport.width) * scale, f32::from(viewport.height) * scale);
        let now = self.started.elapsed().as_secs_f32();
        let delta = self.last_frame.elapsed().as_secs_f32();
        self.last_frame = Instant::now();
        self.frame += 1;

        // Ghostty's cursor rects: left, top, width, height, from the bottom left.
        if let Some(c) = cursor {
            let rect = [f32::from(c.origin.x) * scale, h - f32::from(c.origin.y) * scale, f32::from(c.size.width) * scale, f32::from(c.size.height) * scale];
            if self.cursor != Some(rect) {
                self.previous = self.cursor.unwrap_or(rect);
                self.cursor = Some(rect);
                self.cursor_changed = now;
            }
        }
        let focused = window.is_window_active();
        if self.focused != Some(focused) {
            self.focused = Some(focused);
            self.focus_changed = now;
        }

        let t = crate::vt::theme();
        let rgb = |c: crate::vt::Rgb| [c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0];
        let cursor_color = rgb(t.cursor);
        let palette: Vec<[f32; 3]> = (0..256).map(|i| if i < 16 { rgb(t.palette16[i]) } else { xterm(i) }).collect();
        let current = self.cursor.unwrap_or([0.0; 4]);
        let day = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| (d.as_secs() % 86_400) as f32).unwrap_or(0.0);
        window.set_post_process_values(&[
            ("iResolution", ShaderValue::Vec3([w, h, 1.0])),
            ("iTime", ShaderValue::Float(now)),
            ("iTimeDelta", ShaderValue::Float(delta)),
            ("iFrameRate", ShaderValue::Float(if delta > 0.0 { 1.0 / delta } else { 60.0 })),
            ("iFrame", ShaderValue::Int(self.frame)),
            ("iChannelResolution", ShaderValue::Vec3Array(vec![[w, h, 1.0]])),
            ("iDate", ShaderValue::Vec4([0.0, 0.0, 0.0, day])),
            ("iCurrentCursor", ShaderValue::Vec4(current)),
            ("iPreviousCursor", ShaderValue::Vec4(self.previous)),
            ("iCurrentCursorColor", ShaderValue::Vec4([cursor_color[0], cursor_color[1], cursor_color[2], 1.0])),
            ("iPreviousCursorColor", ShaderValue::Vec4([cursor_color[0], cursor_color[1], cursor_color[2], 1.0])),
            ("iCursorVisible", ShaderValue::Int(i32::from(cursor.is_some()))),
            ("iTimeCursorChange", ShaderValue::Float(self.cursor_changed)),
            ("iFocus", ShaderValue::Int(i32::from(focused))),
            ("iTimeFocus", ShaderValue::Float(self.focus_changed)),
            ("iPalette", ShaderValue::Vec3Array(palette)),
            ("iBackgroundColor", ShaderValue::Vec3(rgb(t.bg))),
            ("iForegroundColor", ShaderValue::Vec3(rgb(t.fg))),
            ("iCursorColor", ShaderValue::Vec3(cursor_color)),
            ("iCursorText", ShaderValue::Vec3(rgb(t.bg))),
            ("iSelectionBackgroundColor", ShaderValue::Vec3(rgb(t.selection_bg))),
            ("iSelectionForegroundColor", ShaderValue::Vec3(rgb(t.fg))),
        ]);
        // Like Ghostty's `custom-shader-animation = true`: animate while focused,
        // and draw once more after focus leaves so effects settle.
        if focused || now - self.focus_changed < 0.5 {
            window.request_animation_frame();
        }
    }
}

/// xterm's 256-color palette above the 16 named colors.
fn xterm(i: usize) -> [f32; 3] {
    if i >= 232 {
        let v = (8 + (i - 232) * 10) as f32 / 255.0;
        return [v, v, v];
    }
    let i = i - 16;
    let level = |n: usize| if n == 0 { 0.0 } else { (55 + n * 40) as f32 / 255.0 };
    [level(i / 36), level((i / 6) % 6), level(i % 6)]
}

#[cfg(test)]
mod tests {
    #[test]
    fn ghostty_shaders_translate() {
        // The prelude, a typical cursor shader and the epilogue are valid GLSL for naga.
        let shader = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 base = texture(iChannel0, uv);
    float since = iTime - iTimeCursorChange;
    vec2 c = iCurrentCursor.xy - iPreviousCursor.xy;
    fragColor = mix(base, vec4(iPalette[6], 1.0), 0.1 * float(iFocus) * step(since, 0.3) * step(1.0, length(c)));
}
"#;
        let glsl = super::source(shader);
        let mut frontend = naga::front::glsl::Frontend::default();
        let module = frontend.parse(&naga::front::glsl::Options::from(naga::ShaderStage::Fragment), &glsl)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&glsl)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&glsl)));
    }

    #[test]
    fn palette_matches_xterm() {
        assert_eq!(super::xterm(16), [0.0, 0.0, 0.0]);
        assert_eq!(super::xterm(231), [1.0, 1.0, 1.0]);
        assert_eq!(super::xterm(232), [8.0 / 255.0; 3]);
    }
}

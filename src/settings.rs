//! Live settings and the resolved layout look, including persisted app zoom.
//!
//! A theme is a palette plus optional color overrides: a built-in preset
//! (graphite, ink, moss, paper) or `themes/<name>.json`. Chrome colors are
//! derived from the palette (dark or light by its background), so every
//! theme is consistent and overrides only need what they change.

use crate::vt::Rgb;
use gpui::{Hsla, Pixels, Rgba, px};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

/// A terminal palette, as herdr-app shipped them.
#[derive(Clone)]
struct Palette {
    background: u32,
    foreground: u32,
    selection: u32,
    cursor: Option<u32>,
    ansi: [u32; 16],
    /// working, blocked, done: the presets pick their own; theme files take
    /// them from ansi (yellow, red, blue). Idle is grey: it needs nobody.
    status: Option<[u32; 3]>,
}

const DARK_STATUS: Option<[u32; 3]> = Some([0xe5c07b, 0xf0705f, 0x76a8ff]);

const GRAPHITE: Palette = Palette {
    background: 0x17191d,
    foreground: 0xd2d4d9,
    selection: 0x394454,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x45484f, 0xdc8c95, 0xa6bd91, 0xd8be87, 0x96b2d7, 0xb5a2ce, 0x8cbdc3, 0xd2d4d9, 0x818793, 0xf0a5ad, 0xbad5a2, 0xead2a1, 0xb0cbef,
        0xcbb7e6, 0xa4d5db, 0xf0f0f2,
    ],
};
const INK: Palette = Palette {
    background: 0x111720,
    foreground: 0xcdd6e4,
    selection: 0x344861,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x414b5a, 0xe58c9a, 0x89c5aa, 0xd9be82, 0x91b6ee, 0xb9a0e2, 0x85c4d0, 0xcdd6e4, 0x7f91a7, 0xf5a5b2, 0xa5dec3, 0xedd39c, 0xaccbfa,
        0xd0b7f4, 0xa0dce5, 0xf0f5fc,
    ],
};
const MOSS: Palette = Palette {
    background: 0x151c19,
    foreground: 0xcdd7ce,
    selection: 0x354a40,
    cursor: None,
    status: DARK_STATUS,
    ansi: [
        0x435248, 0xdf978d, 0xa0c4a2, 0xd4bc8b, 0x99b9cd, 0xbba4c8, 0x8bbdb5, 0xcdd7ce, 0x82968a, 0xf1afa3, 0xb8dab9, 0xe9d2a5, 0xb1d0e2,
        0xd1bddc, 0xa4d6ca, 0xeff5ee,
    ],
};

/// Light: white paper, dark ink. Every ANSI color stays readable on white
/// (bright white is ink, not white: programs write it as "strong" text).
const PAPER: Palette = Palette {
    background: 0xffffff,
    foreground: 0x25272c,
    selection: 0xcfdcf5,
    cursor: Some(0x25272c),
    status: Some([0x9a6700, 0xc93c2c, 0x2f6fe4]),
    ansi: [
        0x25272c, 0xc62a2a, 0x1a7f37, 0x8f5d00, 0x1f62d0, 0x7a45c8, 0x0f6f74, 0x5f6068, 0x7b7e87, 0xd1242f, 0x1f883d, 0x9a6700, 0x0969da,
        0x8250df, 0x1b7c83, 0x111214,
    ],
};

pub const PRESETS: &[&str] = &["graphite", "ink", "moss", "paper"];

/// The theme a layout without one shows, by appearance.
pub const DEFAULT_DARK: &str = "graphite";
pub const DEFAULT_LIGHT: &str = "paper";

fn preset(name: &str) -> Option<&'static Palette> {
    match name {
        "graphite" => Some(&GRAPHITE),
        "ink" => Some(&INK),
        "moss" => Some(&MOSS),
        "paper" => Some(&PAPER),
        _ => None,
    }
}

/// The shipped theme files (defaults/themes), written to `themes/` when missing.
const THEMES: &[(&str, &str)] = &[
    ("atom-one-dark", include_str!("../defaults/themes/atom-one-dark.json")),
    ("win98", include_str!("../defaults/themes/win98.json")),
];
pub const SHIPPED_THEMES: &[&str] = &["atom-one-dark", "win98"];

/// `themes/<name>.json`: a palette, plus `colors` overrides like settings.json's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    background: String,
    foreground: String,
    selection: String,
    cursor: Option<String>,
    ansi: [String; 16],
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

fn theme_path(name: &str) -> std::path::PathBuf {
    crate::mods::app_dir().join("themes").join(format!("{name}.json"))
}

// Only these exact shipped versions may be replaced; personal themes stay intact.
const PREVIOUS_THEMES: &[(&str, &str)] = &[
    ("atom-one-dark", r##"{
  "background": "#21252b",
  "foreground": "#abb2bf",
  "selection": "#323844",
  "cursor": "#abb2bf",
  "ansi": [
    "#21252b", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
    "#767676", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf"
  ]
}
"##),
    ("win98", r##"{
  "background": "#ffffff",
  "foreground": "#c0c0c0",
  "selection": "#000080",
  "cursor": "#c0c0c0",
  "ansi": [
    "#000000", "#aa0000", "#00aa00", "#aa5500", "#0000aa", "#aa00aa", "#00aaaa", "#aaaaaa",
    "#555555", "#ff5555", "#55ff55", "#ffff55", "#5555ff", "#ff55ff", "#55ffff", "#ffffff"
  ],
  "colors": {
    "foreground": "#000000",
    "surface": "#c0c0c0",
    "popover": "#c0c0c0",
    "raised": "#dfdfdf",
    "hover": "#dfdfdf",
    "border": "#808080",
    "input": "#ffffff",
    "muted": "#404040",
    "dim": "#808080",
    "accent": "#000080",
    "accent_foreground": "#ffffff",
    "ring": "#000000",
    "working": "#808000",
    "blocked": "#ff0000",
    "done": "#0000ff",
    "idle": "#808080",
    "unknown": "#808080"
  }
}
"##),
];

pub fn install_themes() {
    let _ = std::fs::create_dir_all(crate::mods::app_dir().join("themes"));
    for (name, text) in THEMES {
        install_theme(&theme_path(name), name, text);
    }
}

fn install_theme(path: &std::path::Path, name: &str, text: &str) {
    let previous = PREVIOUS_THEMES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
    if !path.exists() || std::fs::read_to_string(path).ok().as_deref().is_some_and(|s| Some(s) == previous) {
        let _ = crate::persist::write_atomic(path, text.as_bytes());
    }
}

/// A theme by name: a preset, or `themes/<name>.json`.
fn palette(name: &str) -> Result<(Palette, BTreeMap<String, String>), String> {
    if let Some(p) = preset(name) {
        return Ok((p.clone(), BTreeMap::new()));
    }
    let path = theme_path(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("theme {name:?} is not one of {} and there is no themes/{name}.json", PRESETS.join(", ")))?;
    parse_theme_file(name, &text)
}

fn parse_theme_file(name: &str, text: &str) -> Result<(Palette, BTreeMap<String, String>), String> {
    let f: ThemeFile = serde_json::from_str(text).map_err(|e| format!("themes/{name}.json: {e}"))?;
    let c = |k: &str, v: &str| {
        let h = v.trim().trim_start_matches('#');
        (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten().ok_or(format!("themes/{name}.json: {k} {v:?} is not #rrggbb"))
    };
    let mut ansi = [0; 16];
    for (i, v) in f.ansi.iter().enumerate() {
        ansi[i] = c(&format!("ansi[{i}]"), v)?;
    }
    for (k, v) in &f.colors {
        if parse_hex(v).is_none() {
            return Err(format!("themes/{name}.json: colors.{k}: {v:?} is not #rrggbb or #rrggbbaa"));
        }
    }
    let cursor = f.cursor.as_deref().map(|v| c("cursor", v)).transpose()?;
    let p = Palette {
        background: c("background", &f.background)?,
        foreground: c("foreground", &f.foreground)?,
        selection: c("selection", &f.selection)?,
        cursor,
        ansi,
        status: None,
    };
    Ok((p, f.colors))
}

#[test]
fn shipped_theme_upgrades_preserve_personal_edits() {
    let root = std::env::temp_dir().join(format!("herdr-theme-upgrades-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (name, previous) in PREVIOUS_THEMES {
        let path = root.join(format!("{name}.json"));
        let current = THEMES.iter().find(|(n, _)| n == name).unwrap().1;
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        std::fs::write(&path, previous).unwrap();
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        let edited = previous.replace("#abb2bf", "#abcdef").replace("#c0c0c0", "#abcdef");
        std::fs::write(&path, &edited).unwrap();
        install_theme(&path, name, current);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn shipped_theme_files() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/themes");
    let mut found = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        parse_theme_file(name, &text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(THEMES.iter().find(|(n, _)| *n == name).unwrap().1, text);
        found.insert(name.to_owned());
    }
    assert_eq!(found, THEMES.iter().map(|(n, _)| n.to_string()).collect());
    assert_eq!(found, SHIPPED_THEMES.iter().map(|n| n.to_string()).collect());
}

/// Whether `name` is a theme that can be shown.
pub fn theme_exists(name: &str) -> bool {
    palette(name).is_ok()
}

/// Whether a theme is light (by its background), for picking its half of a pair.
pub fn theme_is_light(name: &str) -> bool {
    palette(name).map(|(p, _)| is_light(p.background)).unwrap_or(false)
}

fn is_light(c: u32) -> bool {
    let ch = |s: u32| ((c >> s) & 255) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0) > 0.5
}

/// Every theme that can be shown, presets first, each with its colors
/// (the settings screen draws them as swatches).
pub fn themes() -> Vec<(String, Theme)> {
    let mut files: Vec<String> = std::fs::read_dir(crate::mods::app_dir().join("themes"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|n| !PRESETS.contains(&n.as_str()))
        .collect();
    files.sort();
    PRESETS
        .iter()
        .map(|n| n.to_string())
        .chain(files)
        .filter_map(|n| palette(&n).ok().map(|(p, o)| (n, resolve(&p, &o))))
        .collect()
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    [16, 8, 0].iter().fold(0, |acc, s| {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        acc | (((x * (1.0 - t) + y * t).round() as u32) << s)
    })
}

/// Every color the app and mods use, resolved. Values are `#rrggbb` or `#rrggbbaa`.
pub type Theme = BTreeMap<String, String>;

/// The panel colors around a palette. `chrome` (sidebars, bars) and
/// `canvas` (behind cards) step away from the terminal's background:
/// darker on a dark theme, toward warm grey on a light one.
fn resolve(p: &Palette, overrides: &BTreeMap<String, String>) -> Theme {
    let bg = p.background;
    let light = is_light(bg);
    let fg = if light { 0x1b1b1e } else { 0xeeeeef };
    let [working, blocked, done] = p.status.unwrap_or([p.ansi[3], p.ansi[1], p.ansi[4]]);
    let (popover, raised, hover, selected, border, input, canvas, chrome) = if light {
        let warm = 0xe8e8e2;
        (hex(bg), hex(mix(bg, 0, 0.035)), "#0000000c".into(), "#00000012".into(), "#00000017".into(), "#00000026".into(), mix(bg, warm, 0.2), mix(bg, warm, 0.6))
    } else {
        (
            hex(mix(bg, 0xffffff, 0.06)),
            hex(mix(bg, 0xffffff, 0.09)),
            "#ffffff0e".into(),
            "#ffffff17".into(),
            "#ffffff18".into(),
            "#ffffff24".into(),
            mix(bg, 0, 0.16),
            mix(bg, 0, 0.32),
        )
    };
    let mut t: Theme = [
        ("background", hex(bg)),
        ("terminal_foreground", hex(p.foreground)),
        ("foreground", hex(fg)),
        ("surface", hex(bg)),
        ("canvas", hex(canvas)),
        ("chrome", hex(chrome)),
        ("popover", popover),
        ("raised", raised),
        ("hover", hover),
        ("selected", selected),
        ("border", border),
        ("input", input),
        ("muted", hex(mix(bg, fg, if light { 0.66 } else { 0.6 }))),
        ("dim", hex(mix(bg, fg, if light { 0.46 } else { 0.38 }))),
        ("accent", hex(p.ansi[4])),
        ("accent_foreground", hex(bg)),
        ("solid", hex(fg)),
        ("solid_foreground", hex(if light { 0xffffff } else { mix(bg, 0, 0.28) })),
        ("ring", hex(mix(bg, fg, 0.7))),
        ("selection", hex(p.selection)),
        ("cursor", hex(p.cursor.unwrap_or(p.foreground))),
        ("working", hex(working)),
        ("blocked", hex(blocked)),
        ("done", hex(done)),
        ("idle", hex(mix(bg, fg, 0.38))),
        ("unknown", hex(mix(bg, fg, 0.38))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (i, c) in p.ansi.iter().enumerate() {
        t.insert(format!("ansi{i}"), hex(*c));
    }
    for (k, v) in overrides {
        t.insert(k.clone(), v.clone());
    }
    t
}

/// settings.json: preferences used where a layout leaves its look unset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    /// How big the whole app is drawn (1 = as designed). The zoom keys change it.
    pub zoom: f32,
    pub sounds: Sounds,
    /// The herdr-core to use, when it isn't where Herdr looks (core_bin.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<String>,
    /// Fonts for layouts that don't set their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    /// The panels' font (their sizes follow zoom).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_font: Option<String>,
    /// Anonymous usage data (telemetry.rs). On unless set to false.
    #[serde(skip_serializing_if = "is_true")]
    pub telemetry: bool,
    /// Allow terminal programs (including remote ones) to copy text locally.
    #[serde(skip_serializing_if = "is_true")]
    pub clipboard_write: bool,
    /// Minimum terminal text contrast ratio; 1 leaves program colors unchanged.
    pub minimum_contrast: f32,
    /// Seconds a multi-key binding (`ctrl-b c`) waits for its next key.
    pub key_timeout: f32,
    /// macOS Option sends Alt shortcuts, or follows the keyboard layout.
    #[serde(skip_serializing_if = "OptionAsAlt::is_auto")]
    pub option_as_alt: OptionAsAlt,
    /// Light or dark: the system's, or one always.
    #[serde(skip_serializing_if = "Appearance::is_system")]
    pub appearance: Appearance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<crate::layout::ThemeChoice>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    fn is_system(&self) -> bool {
        *self == Appearance::System
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsAlt {
    #[default]
    Auto,
    Off,
    On,
}

impl OptionAsAlt {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

fn is_true(b: &bool) -> bool {
    *b
}

impl Default for Person {
    fn default() -> Self {
        Person {
            zoom: 1.0,
            sounds: Sounds::default(),
            core: None,
            font: None,
            font_size: None,
            ui_font: None,
            telemetry: true,
            clipboard_write: true,
            minimum_contrast: 1.0,
            key_timeout: 1.0,
            option_as_alt: OptionAsAlt::Auto,
            appearance: Appearance::System,
            theme: None,
            colors: BTreeMap::new(),
        }
    }
}

/// What the app runs with: the shown layout's look over the defaults, at
/// the person's zoom. Everything reads this one value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub font_family: String,
    pub font_size: f32,
    pub ui_font_family: String,
    pub ui_font_size: f32,
    /// How big the whole app is drawn (1 = as designed). The zoom keys change it.
    pub zoom: f32,
    /// The theme showing: a preset or a file in themes/.
    pub theme: String,
    /// The layout's theme for each appearance (the same one twice when it names one).
    pub theme_light: String,
    pub theme_dark: String,
    /// Whether the window is dark now (the person's `appearance`, else the system's).
    pub dark: bool,
    /// Any theme color by name (see `theme()` in herdr.d.ts), `#rrggbb` or `#rrggbbaa`.
    pub colors: BTreeMap<String, String>,
    /// Space between a terminal's edge and its text, in px: [x, y].
    pub terminal_padding: [f32; 2],
    /// 0..1: below 1, what's behind a terminal shows through.
    pub terminal_opacity: f32,
    pub clipboard_write: bool,
    pub minimum_contrast: f32,
    pub key_timeout: f32,
    pub option_as_alt: OptionAsAlt,
    pub sounds: Sounds,
}

/// Attention sounds: when an agent needs you, and when one finished while you
/// weren't looking. `done` / `blocked`: an MP3 file instead of the bundled one
/// (absolute, or relative to ~/.config/herdr-app).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds { enabled: true, done: None, blocked: None }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_family: DEFAULT_FONT.into(),
            font_size: 14.0,
            ui_font_family: "Geist".into(),
            ui_font_size: 13.0,
            zoom: 1.0,
            theme: DEFAULT_DARK.into(),
            theme_light: DEFAULT_LIGHT.into(),
            theme_dark: DEFAULT_DARK.into(),
            dark: true,
            colors: BTreeMap::new(),
            terminal_padding: [10.0, 6.0],
            terminal_opacity: 1.0,
            clipboard_write: true,
            minimum_contrast: 1.0,
            key_timeout: 1.0,
            option_as_alt: OptionAsAlt::Auto,
            sounds: Sounds::default(),
        }
    }
}

impl Settings {
    pub fn compose(person: &Person, look: &crate::layout::Look, system_dark: bool) -> Settings {
        let d = Settings::default();
        let mut colors = person.colors.clone();
        colors.extend(look.colors.clone().unwrap_or_default());
        let dark = match person.appearance {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        };
        let (theme_light, theme_dark) = match look.theme.as_ref().or(person.theme.as_ref()) {
            Some(c) => (c.pick(false).to_owned(), c.pick(true).to_owned()),
            None => (DEFAULT_LIGHT.to_owned(), DEFAULT_DARK.to_owned()),
        };
        Settings {
            // The layout's own, else the person's, else Herdr's.
            font_family: look.font.clone().or(person.font.clone()).unwrap_or(d.font_family),
            font_size: look.font_size.or(person.font_size).unwrap_or(d.font_size),
            ui_font_family: look.ui_font.clone().or(person.ui_font.clone()).unwrap_or(d.ui_font_family),
            ui_font_size: d.ui_font_size,
            zoom: person.zoom,
            theme: if dark { theme_dark.clone() } else { theme_light.clone() },
            theme_light,
            theme_dark,
            dark,
            colors,
            terminal_padding: look.terminal_padding.unwrap_or(d.terminal_padding),
            terminal_opacity: look.terminal_opacity.unwrap_or(1.0).clamp(0.0, 1.0),
            sounds: person.sounds.clone(),
            clipboard_write: person.clipboard_write,
            minimum_contrast: person.minimum_contrast,
            key_timeout: person.key_timeout,
            option_as_alt: person.option_as_alt,
        }
    }
}

pub fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    let (rgb, a) = match h.len() {
        6 => (v, 255),
        8 => (v >> 8, v & 255),
        _ => return None,
    };
    Some(
        Rgba {
            r: ((rgb >> 16) & 255) as f32 / 255.0,
            g: ((rgb >> 8) & 255) as f32 / 255.0,
            b: (rgb & 255) as f32 / 255.0,
            a: a as f32 / 255.0,
        }
        .into(),
    )
}

pub fn rgb(s: &str) -> Rgb {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(&h[..h.len().min(6)], 16).unwrap_or(0xff00ff);
    Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Settings {
    /// Problems an agent should hear about before the file is applied.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for name in [&self.theme_light, &self.theme_dark] {
            if let Err(e) = palette(name) {
                out.push(e);
            }
        }
        out.dedup();
        for (k, v) in &self.colors {
            if parse_hex(v).is_none() {
                out.push(format!("colors.{k}: {v:?} is not #rrggbb or #rrggbbaa"));
            }
        }
        if !(1.0..=21.0).contains(&self.minimum_contrast) {
            out.push(format!("minimum_contrast {} is outside 1..21", self.minimum_contrast));
        }
        if !KEY_TIMEOUT.contains(&self.key_timeout) {
            out.push(format!("key_timeout {} is outside 0.2..60", self.key_timeout));
        }
        if !(6.0..=72.0).contains(&self.font_size) {
            out.push(format!("font_size {} is outside 6..72", self.font_size));
        }
        out
    }
}

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("settings.json")
}

thread_local! {
    static CURRENT: RefCell<(Rc<Settings>, Rc<Theme>)> =
        RefCell::new((Rc::new(Settings::default()), Rc::new(resolve(&GRAPHITE, &BTreeMap::new()))));
}

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 3.0;

pub fn get() -> Rc<Settings> {
    CURRENT.with(|c| c.borrow().0.clone())
}

/// The resolved theme: every color by name.
pub fn theme() -> Rc<Theme> {
    CURRENT.with(|c| c.borrow().1.clone())
}

/// A theme color as GPUI paint (magenta if missing, so mistakes show).
pub fn color(name: &str) -> Hsla {
    theme().get(name).and_then(|v| parse_hex(v)).unwrap_or(gpui::hsla(0.83, 1.0, 0.5, 1.0))
}

/// A theme color name, or a literal `#rrggbb[aa]`.
pub fn color_or_literal(v: &str) -> Hsla {
    parse_hex(v).unwrap_or_else(|| color(v))
}

/// Reads settings.json (writing the defaults if it's missing), or says why it can't be used.
pub fn load_person() -> Result<Person, String> {
    let p = path();
    if !p.exists() {
        let _ = crate::persist::write_atomic(&p, serde_json::to_string_pretty(&Person::default()).unwrap() + "\n");
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let person: Person = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if !(ZOOM_MIN..=ZOOM_MAX).contains(&person.zoom) {
        return Err(format!("zoom {} is outside {ZOOM_MIN}..{ZOOM_MAX}", person.zoom));
    }
    Ok(person)
}

/// The value of settings.json's own `telemetry`, read even when the file has
/// another problem: an explicit `false` always counts.
pub fn telemetry_setting(person: &Result<Person, String>) -> bool {
    match person {
        Ok(p) => p.telemetry,
        Err(_) => {
            let raw = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            raw.and_then(|v| v.get("telemetry").and_then(serde_json::Value::as_bool)).unwrap_or(true)
        }
    }
}

pub fn write_person(person: &Person) -> std::io::Result<()> {
    crate::persist::write_atomic(&path(), serde_json::to_string_pretty(person).unwrap_or_default() + "\n")
}

const KEY_TIMEOUT: std::ops::RangeInclusive<f32> = 0.2..=60.0;

/// Installs new settings; returns whether anything changed.
pub fn set(s: Settings) -> bool {
    gpui::set_pending_input_timeout(std::time::Duration::from_secs_f32(
        s.key_timeout.clamp(*KEY_TIMEOUT.start(), *KEY_TIMEOUT.end()),
    ));
    #[cfg(target_os = "macos")]
    gpui_macos::set_option_as_alt(match s.option_as_alt {
        OptionAsAlt::Auto => None,
        OptionAsAlt::Off => Some(false),
        OptionAsAlt::On => Some(true),
    });
    CURRENT.with(|c| {
        if *c.borrow().0 == s {
            return false;
        }
        let (p, mut colors) = palette(&s.theme).unwrap_or_else(|_| (GRAPHITE.clone(), BTreeMap::new()));
        colors.extend(s.colors.clone());
        let theme = resolve(&p, &colors);
        gpui_shell::set_pixel_scale(s.zoom);
        *c.borrow_mut() = (Rc::new(s), Rc::new(theme));
        true
    })
}

/// The zoom a first start picks: the system's text size (GNOME's large text,
/// Windows' "make text bigger"), a little more on a big screen at 100%
/// (logical px: a scaled screen already draws bigger), in tenths.
pub fn first_zoom(display_width: Option<f32>) -> f32 {
    let text = system_text_scale().clamp(1.0, 2.0);
    let screen = if display_width.is_some_and(|w| w >= 2400.0) { 1.15 } else { 1.0 };
    ((text * screen * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

fn system_text_scale() -> f32 {
    let run = |cmd: &str, args: &[&str]| -> Option<String> {
        let out = crate::spawn::command(cmd).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    if cfg!(windows) {
        // Percent, 100..225, as REG_DWORD hex.
        return run("reg", &["query", r"HKCU\Software\Microsoft\Accessibility", "/v", "TextScaleFactor"])
            .and_then(|o| o.split_whitespace().last().and_then(|h| u32::from_str_radix(h.trim_start_matches("0x"), 16).ok()))
            .map_or(1.0, |p| p as f32 / 100.0);
    }
    if cfg!(target_os = "linux") {
        return run("gsettings", &["get", "org.gnome.desktop.interface", "text-scaling-factor"]).and_then(|o| o.trim().parse().ok()).unwrap_or(1.0);
    }
    1.0
}

/// The next zoom level in `steps` tenths (0 resets).
pub fn next_zoom(steps: i32) -> f32 {
    if steps == 0 {
        return 1.0;
    }
    ((zoom() * 10.0).round() + steps as f32).clamp(ZOOM_MIN * 10.0, ZOOM_MAX * 10.0) / 10.0
}

pub fn zoom() -> f32 {
    get().zoom
}

/// A designed size in px, at the current zoom.
pub fn z(v: f32) -> Pixels {
    px(v * zoom())
}

pub fn font_size() -> f32 {
    get().font_size * zoom()
}

pub fn ui_font_size() -> f32 {
    get().ui_font_size * zoom()
}

/// Font families on this machine (the bundled ones and the home's included).
static FONTS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

pub fn note_fonts(names: Vec<String>) {
    *FONTS.lock().unwrap() = Some(names.into_iter().map(|n| n.to_lowercase()).collect());
}

/// Font files the home brings: `fonts/` and each mod's `fonts/` (a layout's
/// look travels with its mod, as Herdr 98's does). Loads the ones not loaded
/// yet; true when any was.
pub fn load_home_fonts(cx: &mut gpui::App) -> bool {
    static LOADED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let home = crate::mods::app_dir();
    let mut dirs = vec![home.join("fonts")];
    dirs.extend(std::fs::read_dir(home.join("mods")).into_iter().flatten().flatten().map(|e| e.path().join("fonts")));
    let mut new = Vec::new();
    let mut loaded = LOADED.lock().unwrap();
    for dir in dirs {
        for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
            let font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "ttf" | "otf" | "ttc"));
            if font && !loaded.contains(&path) && let Ok(bytes) = std::fs::read(&path) {
                new.push(std::borrow::Cow::Owned(bytes));
                loaded.push(path);
            }
        }
    }
    if new.is_empty() {
        return false;
    }
    if let Err(e) = cx.text_system().add_fonts(new) {
        tracing::warn!("fonts in {} not loaded: {e:#}", home.display());
    }
    FONTS_ADDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    note_fonts(cx.text_system().all_font_names());
    true
}

static FONTS_ADDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times fonts were added while running: text shaped before may now
/// fall back to other glyphs.
pub fn fonts_added() -> u64 {
    FONTS_ADDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The terminal font, or the bundled default when this machine lacks it:
/// the platform's own fallback is proportional and breaks the grid.
pub fn terminal_font() -> String {
    let family = get().font_family.clone();
    match FONTS.lock().unwrap().as_ref() {
        Some(have) if !have.contains(&family.to_lowercase()) => DEFAULT_FONT.into(),
        _ => family,
    }
}

/// Fonts Herdr ships, so the defaults look the same everywhere.
pub const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/GeistMono-Variable.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Italic-Variable.ttf"),
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic-Variable.ttf"),
    SYMBOLS_FONT_FILE,
];

/// The symbols font itself: terminals read icons' glyphs from it directly.
pub const SYMBOLS_FONT_FILE: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFont-Regular.ttf");

/// Nerd Font icons for terminals, under any font (MIT, nerd-fonts).
pub const SYMBOLS_FONT: &str = "Symbols Nerd Font";

/// Terminals' font unless a layout or settings.json picks another.
pub const DEFAULT_FONT: &str = "JetBrains Mono";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_look_wins_and_settings_fill_what_it_leaves_unset() {
        let person = Person {
            font: Some("person".into()), font_size: Some(19.0), ui_font: Some("person-ui".into()),
            theme: Some(crate::layout::ThemeChoice::One("paper".into())),
            colors: [("accent".into(), "#123456".into())].into(),
            ..Person::default()
        };
        let look = crate::layout::Look {
            font: Some("layout".into()), font_size: Some(14.0), ui_font: Some("layout-ui".into()),
            theme: Some(crate::layout::ThemeChoice::One("ink".into())),
            colors: Some([("accent".into(), "#abcdef".into()), ("text".into(), "#ffffff".into())].into()),
            ..Default::default()
        };
        let inherited = Settings::compose(&person, &crate::layout::Look::default(), true);
        assert_eq!((inherited.font_family.as_str(), inherited.font_size, inherited.ui_font_family.as_str(), inherited.theme.as_str()), ("person", 19.0, "person-ui", "paper"));
        assert_eq!(inherited.colors["accent"], "#123456");
        let own = Settings::compose(&person, &look, true);
        assert_eq!((own.font_family.as_str(), own.font_size, own.ui_font_family.as_str(), own.theme.as_str()), ("layout", 14.0, "layout-ui", "ink"));
        assert_eq!(own.colors["accent"], "#abcdef");
        assert_eq!(own.colors["text"], "#ffffff");
    }

    #[test]
    fn option_as_alt_defaults_to_auto_and_round_trips() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.option_as_alt, OptionAsAlt::Auto);
        for (value, expected) in [("auto", OptionAsAlt::Auto), ("off", OptionAsAlt::Off), ("on", OptionAsAlt::On)] {
            let person: Person = serde_json::from_value(serde_json::json!({"option_as_alt": value})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.option_as_alt, expected);
            assert_eq!(serde_json::to_value(settings).unwrap()["option_as_alt"], value);
        }
        assert!(serde_json::from_value::<Person>(serde_json::json!({"option_as_alt": "left"})).is_err());
    }

    #[test]
    fn minimum_contrast_defaults_off_and_validates_the_ratio() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(default.minimum_contrast, 1.0);
        assert_eq!(Settings::default().minimum_contrast, 1.0);
        for ratio in [1.0, 1.6, 21.0, 0.0, 21.1] {
            let person: Person = serde_json::from_value(serde_json::json!({"minimum_contrast": ratio})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.minimum_contrast, ratio);
            assert_eq!(serde_json::to_value(person).unwrap()["minimum_contrast"], serde_json::json!(ratio));
            assert_eq!(settings.problems().is_empty(), (1.0..=21.0).contains(&ratio));
        }
    }

    #[test]
    fn key_timeout_defaults_to_a_second_and_validates_the_range() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert_eq!(Settings::compose(&default, &crate::layout::Look::default(), true).key_timeout, 1.0);
        for seconds in [0.2, 10.0, 60.0, 0.1, 61.0] {
            let person: Person = serde_json::from_value(serde_json::json!({"key_timeout": seconds})).unwrap();
            let settings = Settings::compose(&person, &crate::layout::Look::default(), true);
            assert_eq!(settings.key_timeout, seconds);
            assert_eq!(settings.problems().is_empty(), (0.2..=60.0).contains(&seconds));
            set(settings);
            assert_eq!(gpui::pending_input_timeout().as_secs_f32(), seconds.clamp(0.2, 60.0));
        }
    }

    #[test]
    fn program_clipboard_writes_default_on_and_can_be_disabled() {
        let default: Person = serde_json::from_str("{}").unwrap();
        assert!(default.clipboard_write);
        let off: Person = serde_json::from_str(r#"{"clipboard_write":false}"#).unwrap();
        assert!(!Settings::compose(&off, &crate::layout::Look::default(), true).clipboard_write);
        assert_eq!(serde_json::to_value(off).unwrap()["clipboard_write"], false);
    }

    #[test]
    fn shipped_looks_and_theme_clicks_have_the_same_precedence() {
        use crate::layout::{Node, ThemeChoice};
        let person = Person { font_size: Some(19.0), theme: Some(ThemeChoice::One("moss".into())), ..Person::default() };
        let main: Node = serde_json::from_str(include_str!("../defaults/layouts/main.json")).unwrap();
        assert!(main.look.theme.is_none() && main.look.colors.is_none() && main.look.font.is_none() && main.look.font_size.is_none() && main.look.ui_font.is_none());
        assert_eq!(Settings::compose(&Person::default(), &main.look, true).font_size, 14.0);
        assert_eq!(main.look.terminal_padding, Some([14.0, 12.0]));
        let terminals: Node = serde_json::from_str(include_str!("../defaults/layouts/terminals.json")).unwrap();
        assert_eq!(Settings::compose(&person, &terminals.look, true).font_size, 15.0);
        let main_settings = Settings::compose(&person, &main.look, true);
        assert_eq!((main_settings.font_size, main_settings.theme.as_str()), (19.0, "moss"));
        let mut win98: Node = serde_json::from_str(include_str!("../defaults/layouts/win98.json")).unwrap();
        let win_settings = Settings::compose(&person, &win98.look, true);
        assert_eq!(win_settings.theme, "win98");
        assert_eq!(win_settings.font_family, "PxPlus IBM VGA 8x16");
        let clicks = crate::layout_sync::State { theme: Some(ThemeChoice::One("ink".into())), ..Default::default() };
        clicks.apply(&mut win98);
        assert_eq!(Settings::compose(&person, &win98.look, true).theme, "ink");
    }

    /// The guide agents read (written to the home folder) explains every
    /// settings.json key.
    #[test]
    fn the_guide_explains_every_setting() {
        let all = Person { core: Some(String::new()), font: Some(String::new()), font_size: Some(1.0), ui_font: Some(String::new()), telemetry: false, clipboard_write: false, appearance: Appearance::Light, theme: Some(crate::layout::ThemeChoice::One(String::new())), colors: [("text".into(), String::new())].into(), ..Person::default() };
        let keys: Vec<String> = serde_json::to_value(&all).unwrap().as_object().unwrap().keys().cloned().collect();
        let guide = include_str!("../defaults/AGENTS.md");
        let section = guide.split("## The person").nth(1).and_then(|s| s.split("\n## ").next()).unwrap_or_default();
        let missing: Vec<&String> = keys.iter().filter(|k| !section.contains(&format!("`{k}`"))).collect();
        assert!(missing.is_empty(), "defaults/AGENTS.md (## The person) doesn't explain: {missing:?}");
    }
}

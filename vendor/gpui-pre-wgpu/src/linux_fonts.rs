//! Build the text system's font database from fontconfig's cache.
//!
//! `fontdb::Database::load_system_fonts()` reads the fontconfig configuration
//! and then opens and parses every font file it lists. On a machine with large
//! collections (several Nerd Font families, CJK, ...) that is thousands of
//! files and gigabytes of reads on a cold page cache: the first launch after a
//! boot can take twenty seconds before the window appears. `fc-list` reads
//! fontconfig's cache instead, which already knows the family, style and file
//! for every face and touches no font data at all.
//!
//! Faces are registered with `Source::File`, so a font file is only read when
//! one of its faces is actually used. The set of families is unchanged: every
//! family `load_system_fonts()` finds is also in fontconfig's cache.

use cosmic_text::{FontSystem, fontdb};
use fontdb::{Database, FaceInfo, ID, Language, Source, Stretch, Style, Weight};
use std::collections::HashSet;

/// One line per face: file, face index, families, weight, slant, stretch,
/// spacing, postscript name. Tabs separate them, so a family list can contain
/// commas without ambiguity.
const FORMAT: &str = "%{file}\t%{index}\t%{family}\t%{weight}\t%{slant}\t%{stretch}\t%{spacing}\t%{postscriptname}\n";

/// A text system backed by fontconfig's cache, or `None` when `fc-list` is not
/// available or produced nothing, so the caller can fall back to the scan.
pub fn font_system() -> Option<FontSystem> {
    let mut db = database()?;

    // The same generic families `FontSystem::new()` sets, so font lookup keeps
    // behaving as it did when the database came from the directory scan.
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Open Sans");
    db.set_serif_family("DejaVu Serif");

    Some(FontSystem::new_with_locale_and_db(locale(), db))
}

fn database() -> Option<Database> {
    let output = std::process::Command::new("fc-list")
        .args(["--format", FORMAT])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let db = parse(&String::from_utf8_lossy(&output.stdout));
    if db.is_empty() { None } else { Some(db) }
}

fn parse(listing: &str) -> Database {
    let mut db = Database::new();
    let mut seen: HashSet<(&str, u32)> = HashSet::new();

    for line in listing.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 8 {
            continue;
        }
        let path = fields[0].trim();
        if path.is_empty() {
            continue;
        }
        // The same file can be listed once per family alias; one face is enough.
        let index = number(fields[1], 0);
        if !seen.insert((path, index)) {
            continue;
        }
        let families: Vec<(String, Language)> = fields[2]
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| (name.to_owned(), Language::English_UnitedStates))
            .collect();
        if families.is_empty() {
            continue;
        }
        db.push_face_info(FaceInfo {
            id: ID::dummy(),
            source: Source::File(path.into()),
            index,
            families,
            post_script_name: fields[7].trim().to_owned(),
            style: style(fields[4]),
            weight: weight(fields[3]),
            stretch: stretch(fields[5]),
            monospaced: monospaced(fields[6]),
        });
    }

    db
}

fn number(field: &str, default: u32) -> u32 {
    field.trim().parse().unwrap_or(default)
}

/// fontconfig slants: 0 roman, 100 italic, 110 oblique.
fn style(field: &str) -> Style {
    match number(field, 0) {
        0 => Style::Normal,
        100 => Style::Italic,
        _ => Style::Oblique,
    }
}

/// fontconfig weights (0..=215) to fontdb's 1..=1000 scale: regular is 80
/// there and 400 here, bold 200 and 700.
fn weight(field: &str) -> Weight {
    Weight(match number(field, 80) {
        0..=25 => 100,
        26..=44 => 200,
        45..=60 => 300,
        61..=89 => 400,
        90..=149 => 500,
        150..=189 => 600,
        190..=203 => 700,
        204..=208 => 800,
        _ => 900,
    })
}

/// fontconfig stretch is a percentage: 100 normal, 50 ultra condensed.
fn stretch(field: &str) -> Stretch {
    match number(field, 100) {
        0..=56 => Stretch::UltraCondensed,
        57..=69 => Stretch::ExtraCondensed,
        70..=80 => Stretch::Condensed,
        81..=94 => Stretch::SemiCondensed,
        95..=106 => Stretch::Normal,
        107..=118 => Stretch::SemiExpanded,
        119..=137 => Stretch::Expanded,
        138..=174 => Stretch::ExtraExpanded,
        _ => Stretch::UltraExpanded,
    }
}

/// fontconfig spacing: 90 dual, 100 monospaced, 110 charcell. The rest is
/// proportional.
fn monospaced(field: &str) -> bool {
    matches!(number(field, 0), 90 | 100 | 110)
}

/// The session's locale, as `sys_locale` would give it ("es-ES", not
/// "es_ES.UTF-8"), so fallback selection matches `FontSystem::new()`.
fn locale() -> String {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if let Some(locale) = locale_from(&value) {
                return locale;
            }
        }
    }
    "en-US".to_owned()
}

fn locale_from(value: &str) -> Option<String> {
    let locale = value.split('.').next()?.replace('_', "-");
    (locale.len() >= 2 && locale != "C" && locale != "POSIX").then_some(locale)
}

//! Cumulative release notes and this launch's update range. Reading notes never
//! changes the last successful version; only expanded spotlights are recorded.
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Note {
    pub version: String,
    pub date: String,
    pub items: Vec<Item>,
    pub spotlight: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Item {
    pub headline: String,
    pub detail: String,
    pub fixed: bool,
    pub media: Vec<Media>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Media {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

pub fn parse(text: &str) -> Vec<Note> {
    parse_for_os(text, std::env::consts::OS)
}

fn parse_for_os(text: &str, os: &str) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    let mut fixed = false;
    for line in text.lines().map(str::trim) {
        if let Some(heading) = line.strip_prefix("## [") {
            if let Some((version, date)) = heading.split_once("] - ") {
                notes.push(Note { version: version.into(), date: date.into(), ..Note::default() });
                fixed = false;
            }
        } else if let Some(note) = notes.last_mut() {
            if let Some(platforms) = line.strip_prefix("<!-- spotlight ").and_then(|s| s.strip_suffix("-->")) {
                note.spotlight |= platforms.trim().is_empty() || platforms.split_whitespace().any(|platform| platform == os);
            } else if line.starts_with("### ") {
                fixed = line == "### Fixed";
            } else if let Some(url) = line.strip_prefix("![").and_then(|s| s.split_once("](")).and_then(|(_, s)| s.strip_suffix(')')) {
                if url.starts_with("https://") && let Some(item) = note.items.last_mut() {
                    item.media.push(Media { path: url.into(), ..Media::default() });
                }
            } else if let Some(text) = line.strip_prefix("- ") {
                let (headline, detail) = text.strip_prefix("**").and_then(|s| s.split_once("**"))
                    .map_or(("", text), |(headline, detail)| (headline, detail.trim()));
                note.items.push(Item { headline: headline.into(), detail: detail.into(), fixed, ..Item::default() });
            }
        }
    }
    notes
}

#[derive(Default, Deserialize, Serialize)]
struct Record {
    last_run: Option<String>,
    #[serde(default)]
    presented: Vec<String>,
}

struct Launch {
    record: Record,
    previous: Option<String>,
    running: String,
    changed: bool,
}

impl Launch {
    fn new(mut record: Record, running: &str, first_install: bool) -> Self {
        let previous = record.last_run.clone();
        let changed = !first_install && previous.as_deref().is_none_or(|v| crate::update::newer(running, v));
        record.last_run = Some(running.into());
        Self { record, previous, running: running.into(), changed }
    }

    fn range(&self, notes: &[Note], target: Option<&str>) -> Vec<Note> {
        let (lower, upper) = match target {
            Some(target) => (Some(self.running.as_str()), target),
            None if self.previous.as_deref().is_some_and(|v| crate::update::newer(v, &self.running)) => return Vec::new(),
            // Since the last version that ran; with nothing new, this version's own notes.
            None => (self.previous.as_deref().filter(|v| crate::update::newer(&self.running, v)), self.running.as_str()),
        };
        notes.iter().filter(|n| {
            !crate::update::newer(&n.version, upper)
                && lower.map_or(n.version == upper, |lower| crate::update::newer(&n.version, lower))
        }).cloned().collect()
    }
}

static LAUNCH: Mutex<Option<Launch>> = Mutex::new(None);
static FETCHED: Mutex<Option<Vec<Note>>> = Mutex::new(None);

fn path(name: &str) -> std::path::PathBuf {
    crate::mods::app_dir().join(name)
}

fn save(record: &Record) {
    let result = serde_json::to_vec_pretty(record).map_err(std::io::Error::other).and_then(|bytes| {
        std::fs::create_dir_all(crate::mods::app_dir())?;
        let file = path("update-notes.json");
        let temp = file.with_extension("json.new");
        std::fs::write(&temp, bytes)?;
        std::fs::rename(temp, file)
    });
    if let Err(e) = result { tracing::warn!("update notes state: {e}"); }
}

/// Called once the live window has connected and synchronized successfully.
pub fn started(first_install: bool) {
    let mut launch = LAUNCH.lock().unwrap();
    if launch.is_some() { return; }
    let record = std::fs::read(path("update-notes.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let state = Launch::new(record, env!("CARGO_PKG_VERSION"), first_install);
    save(&state.record);
    *launch = Some(state);
    std::thread::spawn(|| fetch_media(&all()));
}

pub fn cache(text: &str) {
    let notes = parse(text);
    if notes.is_empty() { return; }
    *FETCHED.lock().unwrap() = Some(notes.clone());
    std::thread::spawn(move || fetch_media(&notes));
    if let Err(e) = std::fs::write(path("update-notes-cache.md"), text) {
        tracing::warn!("update notes cache: {e}");
    }
}

pub fn has(version: &str) -> bool {
    all().iter().any(|note| note.version == version)
}

fn all() -> Vec<Note> {
    let mut notes = parse(include_str!("../CHANGELOG.md"));
    let fetched = FETCHED.lock().unwrap().clone().unwrap_or_else(|| {
        std::fs::read_to_string(path("update-notes-cache.md")).map(|s| parse(&s)).unwrap_or_default()
    });
    // The running binary's curated notes win over cached copies of its releases.
    for note in fetched {
        if !notes.iter().any(|n| n.version == note.version) { notes.push(note); }
    }
    notes.sort_by(|a, b| if crate::update::newer(&a.version, &b.version) { std::cmp::Ordering::Less } else if crate::update::newer(&b.version, &a.version) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Equal });
    notes
}

fn media_path(url: &str) -> std::path::PathBuf {
    let preview = std::env::var_os("HERDR_NOTES_MEDIA_DIR").is_some()
        && local_media_path(url, std::path::Path::new("")).is_some();
    path(if preview { "update-notes-preview" } else { "update-notes-media" }).join(crate::update::sha256(url.as_bytes()))
}

fn local_media_path(url: &str, dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let key = url.strip_prefix("https://media.herdr.dev/")?;
    let parts: Vec<_> = key.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))) {
        return None;
    }
    Some(dir.join(key))
}

fn preview_media(url: &str) -> bool {
    let Some(dir) = std::env::var_os("HERDR_NOTES_MEDIA_DIR") else { return false };
    let Some(local) = local_media_path(url, std::path::Path::new(&dir)) else { return false };
    let file = media_path(url);
    let _ = std::fs::create_dir_all(path("update-notes-preview"));
    let temp = file.with_extension(format!("{:?}.new", std::thread::current().id()));
    if std::fs::copy(local, &temp).and_then(|_| std::fs::rename(&temp, &file)).is_err() {
        let _ = std::fs::remove_file(temp);
        // A missing preview must not silently display an old downloaded clip.
        let _ = std::fs::remove_file(file);
    }
    true
}

fn fetch_media(notes: &[Note]) {
    let dir = path("update-notes-media");
    if std::fs::create_dir_all(&dir).is_err() { return; }
    for media in notes.iter().flat_map(|n| &n.items).flat_map(|item| &item.media) {
        let url = &media.path;
        if preview_media(url) { continue; }
        let file = media_path(url);
        if file.is_file() { continue; }
        // The same URL may be in both the startup and newly fetched feed.
        let temp = file.with_extension(format!("{:?}.new", std::thread::current().id()));
        let result = crate::spawn::command(crate::update::system("curl"))
            .args(["-fsSL", "--max-time", "30", "--max-filesize", "16777216", "-o"])
            .arg(&temp).arg(url).stdin(std::process::Stdio::null()).output();
        if result.is_ok_and(|out| out.status.success()) {
            let _ = std::fs::rename(&temp, &file);
        }
        let _ = std::fs::remove_file(temp);
    }
}

fn cached_media(file: &std::path::Path, root: &std::path::Path) -> Option<Media> {
    let (width, height) = image::ImageReader::open(file).ok()?.with_guessed_format().ok()?.into_dimensions().ok()?;
    if width == 0 || height == 0 { return None; }
    Some(Media { path: file.strip_prefix(root).ok()?.to_string_lossy().into_owned(), width, height })
}

pub fn notes(target: Option<&str>) -> Vec<Note> {
    let mut notes = all();
    for note in &mut notes {
        for item in &mut note.items {
            item.media = item.media.iter().filter_map(|media| {
                preview_media(&media.path);
                cached_media(&media_path(&media.path), &crate::mods::app_dir())
            }).collect();
        }
    }
    LAUNCH.lock().unwrap().as_ref().map(|l| l.range(&notes, target)).unwrap_or_default()
}

pub fn spotlights() -> Vec<Note> {
    let notes = notes(None);
    let launch = LAUNCH.lock().unwrap();
    let Some(l) = launch.as_ref().filter(|l| l.changed) else { return Vec::new() };
    notes.into_iter().filter(|n| n.spotlight && !l.record.presented.contains(&n.version)).collect()
}

pub fn changed() -> bool {
    LAUNCH.lock().unwrap().as_ref().is_some_and(|l| l.changed)
}

pub fn presented(versions: &[String]) {
    let known = all();
    if let Some(l) = LAUNCH.lock().unwrap().as_mut() {
        for version in versions {
            if known.iter().any(|n| n.version == *version && n.spotlight) && !l.record.presented.contains(version) {
                l.record.presented.push(version.clone());
            }
        }
        save(&l.record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Note> {
        parse("## [1.8.0] - 2026-10-23\n<!-- spotlight -->\n- New thing\n![demo](https://releases.herdr.dev/demo.png)\n## [1.7.0] - 2026-10-22\n- Another thing\n## [1.5.0] - 2026-10-20\n- Old thing\n")
    }
    fn from(version: Option<&str>, running: &str, fresh: bool) -> Launch {
        Launch::new(Record { last_run: version.map(str::to_owned), ..Record::default() }, running, fresh)
    }
    #[test]
    fn platform_spotlight_is_recognized() {
        let text = format!("## [1.8.0] - 2026-10-23\n<!-- spotlight {} -->\n- New thing\n", std::env::consts::OS);
        assert!(parse(&text)[0].spotlight);
    }
    #[test]
    fn spotlight_platform_filter_keeps_normal_notes() {
        for platforms in ["", "macos", "linux", "windows", "macos linux", "macos linux windows", "plan9"] {
            let text = format!("## [1.8.0] - 2026-10-23\n<!-- spotlight {platforms} -->\n### New\n- **New thing.** Still visible.\n## [1.7.0] - 2026-10-22\n- Older thing.\n");
            for os in ["macos", "linux", "windows"] {
                let notes = parse_for_os(&text, os);
                assert_eq!(notes[0].spotlight, platforms.is_empty() || platforms.split_whitespace().any(|p| p == os), "{platforms:?} on {os}");
                assert_eq!(notes[0].items[0].headline, "New thing.");
                assert_eq!(notes[0].items[0].detail, "Still visible.");
                assert!(!notes[1].spotlight);
                let launch = from(Some("1.7.0"), "1.8.0", false);
                assert_eq!(launch.range(&notes, None).len(), 1);
            }
        }
    }
    #[test]
    fn spotlight_requires_its_own_comment() {
        for marker in ["<!-- spotlightmacos -->", "<!-- spotlighted -->", "<!-- spotlight macos", "- <!-- spotlight macos -->"] {
            let text = format!("## [1.8.0] - 2026-10-23\n{marker}\n");
            assert!(!parse_for_os(&text, "macos")[0].spotlight);
        }
    }
    #[test]
    fn cached_notes_are_version_specific() {
        let previous = FETCHED.lock().unwrap().replace(Vec::new());
        assert!(!has("99.0.0"));
        *FETCHED.lock().unwrap() = Some(parse("## [99.0.0] - 2026-10-30\n- Test release\n"));
        assert!(has("99.0.0"));
        assert!(!has("99.0.1"));
        *FETCHED.lock().unwrap() = previous;
    }
    #[test]
    fn first_install_only_welcomes() {
        let l = from(None, "1.8.0", true);
        assert!(!l.changed);
        assert_eq!(l.record.last_run.as_deref(), Some("1.8.0"));
    }
    #[test]
    fn skipped_versions_and_pending_range() {
        let l = from(Some("1.5.0"), "1.8.0", false);
        assert_eq!(l.range(&fixture(), None).iter().map(|n| n.version.as_str()).collect::<Vec<_>>(), ["1.8.0", "1.7.0"]);
        let before = from(Some("1.5.0"), "1.5.0", false);
        assert_eq!(before.range(&fixture(), Some("1.8.0")).len(), 2);
        assert_eq!(before.record.last_run.as_deref(), Some("1.5.0"));
    }
    #[test]
    fn downgrade_records_without_replaying_future_notes() {
        let l = from(Some("1.8.0"), "1.5.0", false);
        assert!(!l.changed);
        assert!(l.range(&fixture(), None).is_empty());
        assert_eq!(l.record.last_run.as_deref(), Some("1.5.0"));
    }
    #[test]
    fn missing_baseline_is_current_release_only() {
        let l = from(None, "1.8.0", false);
        assert!(l.changed);
        assert_eq!(l.range(&fixture(), None).len(), 1);
        // Nothing new since the last run: Settings still shows this version's notes.
        let same = from(Some("1.8.0"), "1.8.0", false);
        assert!(!same.changed);
        assert_eq!(same.range(&fixture(), None).iter().map(|n| n.version.as_str()).collect::<Vec<_>>(), ["1.8.0"]);
    }
    #[test]
    fn spotlight_presented_before_restart_does_not_replay() {
        let mut before = from(Some("1.5.0"), "1.5.0", false);
        before.record.presented.push("1.8.0".into());
        let after = Launch::new(before.record, "1.8.0", false);
        assert!(after.changed);
        assert!(!after.range(&fixture(), None).iter().any(|n| n.spotlight && !after.record.presented.contains(&n.version)));
    }
    #[test]
    fn local_media_is_limited_to_canonical_media_urls() {
        let dir = std::path::Path::new("/var/tmp/media");
        assert_eq!(local_media_path("https://media.herdr.dev/0.1.4/peek.webp", dir), Some(dir.join("0.1.4/peek.webp")));
        for url in ["https://example.com/0.1.4/peek.webp", "https://media.herdr.dev/../peek.webp", "https://media.herdr.dev/0.1.4/a/b.webp", "https://media.herdr.dev/0.1.4/%2e%2e", "https://media.herdr.dev/0.1.4/a.webp?query"] {
            assert!(local_media_path(url, dir).is_none());
        }
    }
    #[test]
    fn cached_media_reads_dimensions_from_extensionless_files() {
        let root = std::path::PathBuf::from(format!("/var/tmp/herdr-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("hashed-image");
        image::RgbaImage::new(12, 7).save_with_format(&file, image::ImageFormat::Png).unwrap();
        let media = cached_media(&file, &root).unwrap();
        assert_eq!((media.path.as_str(), media.width, media.height), ("hashed-image", 12, 7));
        std::fs::write(&file, b"not an image").unwrap();
        assert!(cached_media(&file, &root).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sections_and_item_media() {
        let notes = parse("## [1.8.0] - 2026-10-23\n### New\n- **A headline.** Its detail.\n![demo](https://example.com/demo.png)\n- **Another headline.** More detail.\n![bad](http://example.com/no.png)\n### Fixed\n- A short fix.\n## [1.7.0] - 2026-10-22\n- Plain item.\n");
        let items = &notes[0].items;
        assert_eq!(items[0].headline, "A headline.");
        assert_eq!(items[0].detail, "Its detail.");
        assert_eq!(items[0].media[0].path, "https://example.com/demo.png");
        assert!(items[1].media.is_empty());
        assert!(items[2].fixed);
        assert_eq!(items[2].detail, "A short fix.");
        assert!(!notes[1].items[0].fixed);
        assert_eq!(notes[1].items[0].detail, "Plain item.");
    }
    #[test]
    fn curated_markdown_and_remote_media() {
        let notes = fixture();
        assert!(notes[0].spotlight);
        assert_eq!(notes[0].items[0].media[0].path, "https://releases.herdr.dev/demo.png");
        assert_eq!(notes[1].items[0].detail, "Another thing");
        assert!(notes[1].items[0].headline.is_empty());
        assert!(!notes[1].items[0].fixed);
        assert_eq!(parse(include_str!("../CHANGELOG.md"))[0].version, env!("CARGO_PKG_VERSION"));
    }
}

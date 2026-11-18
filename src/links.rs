//! Links in terminal text: Ghostty's URL and file path pattern, what a match
//! points to, and handing it to the system to open.

use fancy_regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Ghostty's default link pattern (`src/config/url.zig`), plus the editor
/// remote schemes (`cursor://vscode-remote/...`, `vscode://vscode-remote/...`).
static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    let schemes = r"https?://|mailto:|ftp://|file:|ssh:|git://|ssh://|tel:|magnet:|ipfs://|ipns://|gemini://|gopher://|news:|cursor://|vscode://";
    let ipv6 = r"(?:\[[:0-9a-fA-F]+(?:[:0-9a-fA-F]*)+\](?::[0-9]+)?)";
    let scheme_chars = r"[\w\-.~:/?#@!$&*+,;=%]";
    let path_chars = r"[\w\-.~:\/?#@!$&*+;=%]";
    let bracketed = r"(?:[\(\[]\w*[\)\]])?";
    let no_trailing_punctuation = r"(?<![,.])";
    let no_trailing_colon = r"(?<!:)";
    let dotted = r"(?=[\w\-.~:\/?#@!$&*+;=%]*\.)";
    let undotted = r"(?![\w\-.~:\/?#@!$&*+;=%]*\.)";
    let dotted_spaces = r"(?:(?<!:) (?!\w+:\/\/)(?!\.{0,2}\/)(?!~\/)[\w\-.~:\/?#@!$&*+;=%]*[\/.])*";
    let any_spaces = r"(?:(?<!:) (?!\w+:\/\/)(?!\.{0,2}\/)(?!~\/)[\w\-.~:\/?#@!$&*+;=%]+)*";
    let rooted = r"(?:\.\.\/|\.\/|(?<!\w)~\/|(?:[\w][\w\-.]*\/)*(?<!\w)\$[A-Za-z_]\w*\/|\.[\w][\w\-.]*\/|(?<![\w~\/])\/(?!\/))";
    let scheme_url = format!("(?:{schemes})(?:{ipv6}|{scheme_chars}+{bracketed})+{no_trailing_punctuation}");
    let rooted_path = format!(
        "{rooted}(?:{dotted}{path_chars}+{dotted_spaces}{no_trailing_colon}|{undotted}{path_chars}+{any_spaces}{no_trailing_colon})"
    );
    let bare_path = format!(r"{dotted}(?<!\$\d*)(?<!\w)[\w][\w\-.]*\/{path_chars}+{no_trailing_colon}");
    Regex::new(&format!("{scheme_url}|{rooted_path}|{bare_path}")).expect("link pattern")
});

/// The link in `text` that covers byte `at`.
pub fn find(text: &str, at: usize) -> Option<Range<usize>> {
    for m in PATTERN.find_iter(text) {
        let m = m.ok()?;
        if m.start() > at {
            return None;
        }
        if at < m.end() {
            return Some(m.range());
        }
    }
    None
}

/// The address-shaped word under the pointer, excluding surrounding punctuation.
pub fn word(text: &str, at: usize) -> Option<Range<usize>> {
    let allowed = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '/' | '.');
    if !text.get(at..)?.chars().next().is_some_and(allowed) {
        return None;
    }
    let start = text[..at].char_indices().rev().find(|(_, c)| !allowed(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let end = text[at..].char_indices().find(|(_, c)| !allowed(*c)).map_or(text.len(), |(i, _)| at + i);
    let trimmed = text[start..end].trim_matches('.');
    let start = start + text[start..end].len() - text[start..end].trim_start_matches('.').len();
    let end = start + trimmed.len();
    (start <= at && at < end).then_some(start..end)
}

/// Bare addresses refer to the hovered terminal's machine, never its names.
pub fn terminal_address<'a>(word: &str, source: &str, live: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let address = if word.contains('/') {
        word.to_owned()
    } else {
        crate::mirror::qualify(word, source.split_once('/').map(|(machine, _)| machine))
    };
    live.into_iter().any(|a| a == address).then_some(address)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    File(PathBuf),
}

fn has_scheme(link: &str) -> bool {
    link.split_once(':').is_some_and(|(s, _)| {
        s.len() > 1 && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
}

/// A spelling selected by the native detector or supplied by OSC 8.
pub struct Resolved<'a> {
    raw: &'a str,
    spelling: &'a str,
    pub native: Option<Target>,
}

/// What a link opens. File paths count only when they exist here; relative
/// ones need the terminal's folder.
pub fn resolve<'a>(link: &'a str, cwd: Option<&Path>) -> Resolved<'a> {
    let (native, spelling) = if has_scheme(link) {
        let native = file_url_spelling(link)
            .as_deref()
            .map(file_url_path)
            .filter(|p| p.exists())
            .map_or_else(|| Target::Url(link.to_owned()), Target::File);
        (Some(native), link)
    } else if let Some((path, spelling)) = local_file(link, cwd) {
        (Some(Target::File(path)), spelling)
    } else {
        (None, without_suffix(link))
    };
    Resolved { raw: link, spelling, native }
}

fn local_file<'a>(link: &'a str, cwd: Option<&Path>) -> Option<(PathBuf, &'a str)> {
    let full = expand_file(link, cwd)?;
    // Windows ignores a trailing period when checking whether a path exists.
    if !(cfg!(windows) && full.extension().is_some_and(|ext| ext.is_empty())) && full.exists() {
        return Some((full, link));
    }
    let bare = without_suffix(full.to_str()?);
    Path::new(bare).exists().then(|| (PathBuf::from(bare), without_suffix(link)))
}

fn expand_file(link: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let home = std::env::home_dir();
    let expanded = if let Some(rest) = link.strip_prefix("~/") {
        home?.join(rest)
    } else if let Some(var) = link.strip_prefix('$') {
        let (name, rest) = var.split_once('/').unwrap_or((var, ""));
        let base = match name {
            "PWD" => cwd?.to_path_buf(),
            "HOME" => home?,
            _ => PathBuf::from(std::env::var_os(name)?),
        };
        base.join(rest)
    } else {
        PathBuf::from(link)
    };
    Some(if expanded.is_absolute() { expanded } else { cwd?.join(expanded) })
}

// The period ending a sentence, or `:42` / `:42:7` naming a place in the file.
fn without_suffix(path: &str) -> &str {
    let mut bare = path.strip_suffix('.').unwrap_or(path);
    for _ in 0..2 {
        match bare.rsplit_once(':') {
            Some((head, n)) if is_number(n) => bare = head,
            _ => break,
        }
    }
    bare
}

/// The `:line[:column]` that `raw` adds after the path spelling `bare`.
fn place(raw: &str, bare: &str) -> Option<(u32, Option<u32>)> {
    let rest = raw.strip_prefix(bare)?.strip_prefix(':')?;
    let rest = rest.strip_suffix('.').unwrap_or(rest);
    let (line, column) = match rest.split_once(':') {
        Some((line, column)) => (line, Some(column.parse().ok()?)),
        None => (rest, None),
    };
    Some((line.parse().ok()?, column))
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The path spelling in a local `file:` URL, percent-decoded.
fn file_url_spelling(link: &str) -> Option<String> {
    let rest = link.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    // Another host's file, or no path.
    if !rest.starts_with('/') {
        return None;
    }
    decode_file_url_path(rest)
}

/// File rules see all file URIs. Only an empty authority or localhost
/// can name a local file.
fn handler_file_uri(link: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = link.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    let rest = rest.split(['?', '#']).next()?;
    Some(if let Some(rest) = rest.strip_prefix("//").or_else(|| rest.strip_prefix(r"\\")) {
        rest.split_at(rest.find(['/', '\\']).unwrap_or(rest.len()))
    } else {
        ("", rest)
    })
}

fn decode_file_url_path(path: &str) -> Option<String> {
    let rest = path.split(['?', '#']).next()?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut i = 0;
    while i < rest.len() {
        if rest.as_bytes()[i] == b'%' {
            bytes.push(u8::from_str_radix(rest.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(rest.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

fn file_url_path(path: &str) -> PathBuf {
    // `/C:/Users/...` names `C:\Users\...`.
    if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') && path.as_bytes()[1].is_ascii_alphabetic() {
        return PathBuf::from(path[1..].replace('/', "\\"));
    }
    PathBuf::from(path)
}

fn can_verify_handler_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        let bytes = path.as_os_str().as_encoded_bytes();
        if bytes.starts_with(br"\\") || bytes.starts_with(b"//")
            || matches!(path.components().next(), Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(..)))
        {
            return false;
        }
    }
    path.has_root()
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Url,
    File,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "RuleSpec")]
pub struct Rule {
    kind: Kind,
    pattern: regex::Regex,
}

#[derive(Deserialize)]
struct RuleSpec {
    kind: Kind,
    pattern: String,
}

impl TryFrom<RuleSpec> for Rule {
    type Error = regex::Error;

    fn try_from(spec: RuleSpec) -> Result<Self, Self::Error> {
        Ok(Self { kind: spec.kind, pattern: regex::Regex::new(&spec.pattern)? })
    }
}

pub type Handlers = Vec<(String, std::rc::Rc<[Rule]>)>;

#[derive(Clone, Debug, PartialEq)]
pub struct Click {
    /// Each mod receives the link view selected by its first matching rule.
    pub handlers: Vec<(String, Value)>,
    pub native: Option<Target>,
}

impl Resolved<'_> {
    pub fn click(self, remote: bool, active: &Handlers) -> Option<Click> {
        let Self { raw, spelling, native } = self;
        if active.is_empty() {
            return native.map(native_click);
        }

        let is_url = has_scheme(raw);
        let uri = handler_file_uri(raw);
        let decoded = uri.and_then(|(_, path)| decode_file_url_path(path));
        let file_spelling = if let Some((_, path)) = uri {
            Some(decoded.as_deref().unwrap_or(path))
        } else if !is_url {
            // The desktop's files cannot tell us whether a remote suffix is literal.
            Some(if remote { without_suffix(raw) } else { spelling })
        } else {
            None
        };

        let mut handlers = Vec::new();
        let mut file_link = None;
        let mut url_link = None;
        for (id, rules) in active {
            if handlers.iter().any(|(name, _)| name == id) {
                continue;
            }
            let Some(rule) = rules.iter().find(|rule| match rule.kind {
                Kind::Url => is_url && rule.pattern.is_match(raw),
                Kind::File => file_spelling.is_some_and(|path| rule.pattern.is_match(path)),
            }) else { continue };
            let link = match rule.kind {
                Kind::Url => url_link.get_or_insert_with(|| json!({"kind": "url", "raw": raw, "url": raw})),
                Kind::File => file_link.get_or_insert_with(|| {
                    let url_file;
                    let path = if remote {
                        None
                    } else {
                        match &native {
                            Some(Target::File(path)) => Some(path.as_path()),
                            _ if uri.is_some_and(|(host, _)| host.is_empty() || host.eq_ignore_ascii_case("localhost")) => {
                                url_file = decoded.as_deref().map(file_url_path)
                                    .filter(|path| can_verify_handler_path(path) && path.exists());
                                url_file.as_deref()
                            }
                            _ => None,
                        }
                    };
                    let mut link = json!({"kind": "file", "raw": raw, "path": path.and_then(Path::to_str)});
                    if let Some((line, column)) = file_spelling.and_then(|bare| place(raw, bare)) {
                        link["line"] = line.into();
                        if let Some(column) = column {
                            link["column"] = column.into();
                        }
                    }
                    link
                }),
            };
            handlers.push((id.clone(), link.clone()));
        }
        if handlers.is_empty() {
            return native.map(native_click);
        }

        let native = if remote && file_spelling.is_some() { None } else { native };
        Some(Click { handlers, native })
    }
}

fn native_click(native: Target) -> Click {
    Click { handlers: Vec::new(), native: Some(native) }
}

/// Returns true when a mod handles the click and false when native opening can proceed.
pub async fn dispatch<T, F>(
    handlers: impl IntoIterator<Item = (String, T)>,
    mut call: impl FnMut(&str, T) -> F,
) -> Result<bool, (String, String)>
where
    F: std::future::Future<Output = Result<String, String>>,
{
    for (name, entry) in handlers {
        let result = call(&name, entry).await.map_err(|error| (name.clone(), error))?;
        match result.as_str() {
            "handled" => return Ok(true),
            "pass" => {}
            _ => return Err((name, format!("handleLink returned {result:?}; expected \"handled\" or \"pass\""))),
        }
    }
    Ok(false)
}

/// Hands the target to the system's default handler.
pub fn open(target: &Target) {
    let arg = match target {
        Target::Url(u) => std::ffi::OsStr::new(u),
        Target::File(p) => p.as_os_str(),
    };
    open_system(arg);
}

#[cfg(windows)]
fn open_system(arg: &std::ffi::OsStr) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize},
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };

    let mut wide: Vec<u16> = arg.encode_wide().collect();
    if wide.contains(&0) {
        tracing::warn!("cannot open a link containing a NUL");
        return;
    }
    wide.push(0);
    // ShellExecuteW can pump messages; never reenter GPUI while it handles a click.
    std::thread::spawn(move || {
        let initialized = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
        // Explorer interprets '=' as its own arguments; the handler gets a literal target.
        let result = unsafe {
            ShellExecuteW(std::ptr::null_mut(), std::ptr::null(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
        } as isize;
        if result <= 32 {
            tracing::warn!("cannot open link: ShellExecuteW returned {result}");
        }
        if initialized >= 0 {
            unsafe { CoUninitialize() };
        }
    });
}

#[cfg(not(windows))]
fn open_system(arg: &std::ffi::OsStr) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let spawned = std::process::Command::new(opener)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        // Reaped off the UI thread so it never lingers as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => tracing::warn!("cannot run {opener}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(value: Value) -> std::rc::Rc<[Rule]> {
        serde_json::from_value::<Vec<Rule>>(value).unwrap().into()
    }

    #[test]
    fn rules_match_the_link_kind_and_full_destination_in_layout_order() {
        let localhost = rules(json!([
            {"kind": "url", "pattern": "^https?://localhost(?::[0-9]+)?(?:[/?#]|$)"},
            {"kind": "url", "pattern": "#anchor$"}
        ]));
        let active = [
            ("specialist".into(), localhost.clone()),
            ("viewer".into(), rules(json!([{"kind": "file", "pattern": "\\.png$"}]))),
            ("browser".into(), rules(json!([{"kind": "url", "pattern": "^https?:"}]))),
            ("specialist".into(), localhost),
        ].to_vec();
        let url = "http://localhost:8123/view.png?a=1#anchor";
        let hit = resolve(url, None).click(true, &active).unwrap();
        let payload = json!({"kind":"url", "raw":url, "url":url});
        assert_eq!(hit.handlers, [("specialist".to_owned(), payload.clone()), ("browser".to_owned(), payload)]);
        assert_eq!(hit.native, Some(Target::Url(url.into())));

        let other = resolve("http://localhost.example.com", None).click(false, &active).unwrap();
        assert_eq!(other.handlers.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["browser"]);
        let foreign = "file://server/plot.png";
        let hit = resolve(foreign, None).click(false, &active).unwrap();
        assert_eq!(hit.handlers, [("viewer".to_owned(), json!({"kind":"file", "raw":foreign, "path":null}))]);
        assert_eq!(hit.native, Some(Target::Url(foreign.into())));
    }

    #[test]
    fn handlers_use_the_native_match_and_opener_destination() {
        use Kind::{File, Url};

        let tmp = if cfg!(windows) { std::env::temp_dir() } else { crate::test_tmp() };
        let cwd = tmp.join(format!("herdr-link-targets-{}", std::process::id()));
        for name in ["src/main.rs", "plot.png", "test folder/file.txt", "folder/file.txt", "peer/t7"] {
            let path = cwd.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        #[cfg(unix)]
        std::fs::write(cwd.join("src/main.rs:7"), "").unwrap();
        let literal = if cfg!(unix) { "./src/main.rs:7" } else { "./src/main.rs" };
        let active = vec![("editor".into(), rules(json!([
            {"kind": "url", "pattern": r"^(?:https?://localhost:|tel:|file://nas/)"},
            {"kind": "file", "pattern": r"\.(png|rs|txt|ini)$|^(\./src/main\.rs:7|src/\.\.)$"}
        ])))];
        let unmatched = vec![("unused".into(), rules(json!([{"kind":"file","pattern":"^$"},{"kind":"url","pattern":"^$"}])))];
        let f = |path: &str| Some(Target::File(cwd.join(path)));
        let u = |url: &str| Some(Target::Url(url.into()));
        let localhost = "http://localhost:8080/docs";
        let full_url = "http://localhost:8123/view.png?a=1#anchor";
        let nas = "file://nas/tmp/plot.png";
        let slashed = cwd.join("plot.png").to_string_lossy().replace('\\', "/");
        let encoded: String = slashed.bytes().map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }
        }).collect();
        let root = if cfg!(windows) { "/" } else { "" };
        let file_uri = format!("file://localhost{root}{encoded}");
        let upper_uri = format!("FiLe://LOCALHOST{root}{encoded}");
        let uri_file = Some(Target::File(file_url_path(&format!("{root}{slashed}"))));

        // No pointer means OSC 8. The first target is master's opener
        // destination, the second is the destination after a handler passes.
        let cases = [
            ("(http://localhost:8080/docs)", Some("docs"), false, Some((Url, localhost)), u(localhost), u(localhost)),
            (full_url, None, true, Some((Url, full_url)), u(full_url), u(full_url)),
            ("tel:12345", Some("12345"), false, Some((Url, "tel:12345")), u("tel:12345"), u("tel:12345")),
            ("src/main.rs:12:5: error", Some("main.rs"), false, Some((File, "src/main.rs:12:5")), f("src/main.rs"), f("src/main.rs")),
            ("./src/main.rs:7", Some("main.rs"), false, Some((File, "./src/main.rs:7")), f(literal), f(literal)),
            ("src/..", Some(".."), false, Some((File, "src/..")), f("src/.."), f("src/..")),
            ("./test folder/file.txt", Some("file.txt"), false, Some((File, "./test folder/file.txt")), f("./test folder/file.txt"), f("./test folder/file.txt")),
            ("--out=$PWD/plot.png", Some("plot.png"), false, Some((File, "$PWD/plot.png")), f("plot.png"), f("plot.png")),
            ("./x.png", Some("x.png"), false, Some((File, "./x.png")), None, None),
            ("./gone.rs:42:7", Some("gone.rs"), false, Some((File, "./gone.rs:42:7")), None, None),
            ("./plot.png", Some("plot.png"), true, Some((File, "./plot.png")), f("./plot.png"), None),
            ("./x.png", Some("x.png"), true, Some((File, "./x.png")), None, None),
            (file_uri.as_str(), None, false, Some((File, file_uri.as_str())), uri_file.clone(), uri_file.clone()),
            (file_uri.as_str(), None, true, Some((File, file_uri.as_str())), uri_file, None),
            (upper_uri.as_str(), None, true, Some((File, upper_uri.as_str())), u(&upper_uri), None),
            ("file:C:/Windows/win.ini", Some("win.ini"), true, Some((File, "file:C:/Windows/win.ini")), u("file:C:/Windows/win.ini"), None),
            ("FiLe:C:/Windows/win.ini", None, true, Some((File, "FiLe:C:/Windows/win.ini")), u("FiLe:C:/Windows/win.ini"), None),
            (r"file:C:\Windows\win.ini", None, true, Some((File, r"file:C:\Windows\win.ini")), u(r"file:C:\Windows\win.ini"), None),
            (nas, Some("plot.png"), false, Some((Url, nas)), u(nas), u(nas)),
            ("https://example.com/outside", Some("outside"), false, None, u("https://example.com/outside"), u("https://example.com/outside")),
            ("can't open plot.png", Some("plot.png"), false, None, None, None),
            ("peer/t7", Some("t7"), false, None, None, None),
        ];

        for (text, pointer, remote, handled, expected_native, expected_pass) in cases {
            let raw = match pointer {
                Some(word) => {
                    let at = text.find(word).unwrap() + word.len() - 1;
                    find(text, at).map(|range| &text[range])
                }
                None => Some(text),
            };
            let click = |rules: &Handlers| raw.and_then(|raw| resolve(raw, Some(&cwd)).click(remote, rules));
            let expected_payload = handled.map(|(kind, expected_raw)| {
                assert_eq!(raw, Some(expected_raw), "{text}");
                match kind {
                    Url => json!({"kind":"url", "raw":expected_raw, "url":expected_raw}),
                    File => {
                        let path = match &expected_pass { Some(Target::File(path)) => Some(path.as_path()), _ => None };
                        let mut link = json!({"kind":"file", "raw":expected_raw, "path":path.map(Path::to_string_lossy)});
                        let place = match expected_raw {
                            "src/main.rs:12:5" => Some((12, Some(5))),
                            "./gone.rs:42:7" => Some((42, Some(7))),
                            "./src/main.rs:7" if !cfg!(unix) => Some((7, None)),
                            _ => None,
                        };
                        if let Some((line, column)) = place {
                            link["line"] = json!(line);
                            if let Some(column) = column {
                                link["column"] = json!(column);
                            }
                        }
                        link
                    }
                }
            });
            let hit = click(&active);
            let payload = hit.as_ref().and_then(|click| click.handlers.first().map(|(_, link)| link));
            assert_eq!(payload, expected_payload.as_ref(), "handler payload for {text}");
            assert_eq!(hit.and_then(|click| click.native), expected_pass, "opener after pass for {text}");
            assert_eq!(click(&Vec::new()), expected_native.clone().map(native_click), "no handlers for {text}");
            assert_eq!(click(&unmatched), expected_native.map(native_click), "no rule matched {text}");
        }
        let at = "peer/t7".len() - 1;
        assert_eq!(terminal_address(&"peer/t7"[word("peer/t7", at).unwrap()], "t1", ["peer/t7"]).as_deref(), Some("peer/t7"));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn places_come_from_the_suffix_after_the_path() {
        assert_eq!(place("src/a.rs:12:5", "src/a.rs"), Some((12, Some(5))));
        assert_eq!(place("src/a.rs:12", "src/a.rs"), Some((12, None)));
        assert_eq!(place("src/a.rs:12.", "src/a.rs"), Some((12, None)));
        assert_eq!(place("src/a.rs", "src/a.rs"), None);
        assert_eq!(place("src/a.rs:7", "src/a.rs:7"), None);
        assert_eq!(place("src/a.rs:x", "src/a.rs"), None);
        assert_eq!(place("file:///a.rs", "/a.rs"), None);
    }

    #[test]
    fn file_uris_offer_both_views_and_keep_the_local_opener() {
        let dir = std::env::temp_dir().join(format!("herdr-file-uri-handlers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plot.png"), "").unwrap();
        let slashed = dir.to_string_lossy().replace('\\', "/");
        let root = if cfg!(windows) { "/" } else { "" };
        let encoded: String = slashed.bytes().map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }
        }).collect();
        let active = vec![
            ("files".into(), rules(json!([{"kind":"file", "pattern":r"\.png$"}]))),
            ("urls".into(), rules(json!([{"kind":"url", "pattern":"(?i)^file:"}]))),
        ];
        let urls_only = vec![active[1].clone()];
        let drive_uri = "file:C:/Windows/win.ini";
        assert_eq!(resolve(drive_uri, None).click(true, &urls_only).unwrap().native, None);
        let unmatched = vec![("unused".into(), rules(json!([{"kind":"url", "pattern":"^https:"}])))];
        let existing = file_url_path(&format!("{root}{slashed}/plot.png"));
        for (name, exists) in [("plot.png", true), ("missing.png", false)] {
            let variants = [
                (format!("file:{root}{encoded}/{name}"), "", true),
                (format!("file://{root}{encoded}/{name}"), "", true),
                (format!("file://localhost{root}{encoded}/{name}"), "localhost", true),
                (format!("file://host{root}{encoded}/{name}"), "host", false),
                (format!(r"file:\\nas\share\{name}"), "nas", false),
                (format!(r"file://\\nas\share\{name}"), "", false),
                #[cfg(windows)]
                (format!("file:%2F%2Fnas%2Fshare%2F{name}"), "", false),
                #[cfg(windows)]
                (format!("file:%5C%5C%3F%5CUNC%5Cnas%5Cshare%5C{name}"), "", false),
                #[cfg(windows)]
                (format!("file:%5C%5C.%5Cdevice%5C{name}"), "", false),
                (format!("FiLe:{name}"), "", false),
            ];
            for (raw, authority, can_verify_local) in variants {
                let (host, spelling) = handler_file_uri(&raw).unwrap();
                assert_eq!(host, authority, "{raw}");
                if host.is_empty() && !can_verify_local {
                    // Check the guard before the click can reach a filesystem probe.
                    let path = file_url_path(&decode_file_url_path(spelling).unwrap());
                    assert!(!can_verify_handler_path(&path), "{raw}");
                }
                let original = native_target(&raw, None);
                for remote in [false, true] {
                    let verified = exists && !remote && can_verify_local;
                    let hit = resolve(&raw, None).click(remote, &active).unwrap();
                    let path = verified.then(|| existing.to_str().unwrap());
                    let url = json!({"kind":"url", "raw":raw, "url":raw});
                    let file = json!({"kind":"file", "raw":raw, "path":path});
                    let native = if remote { None } else { original.clone() };
                    assert_eq!(hit.handlers, [("files".to_owned(), file), ("urls".to_owned(), url.clone())], "{raw}");
                    assert_eq!(hit.native, native, "{raw}");

                    let hit = resolve(&raw, None).click(remote, &urls_only).unwrap();
                    assert_eq!(hit.handlers, [("urls".to_owned(), url)], "{raw}");
                    assert_eq!(hit.native, native, "{raw}");
                    assert_eq!(
                        resolve(&raw, None).click(remote, &unmatched),
                        original.clone().map(native_click),
                        "{raw}"
                    );
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_non_utf8_path_is_null_in_the_payload_and_exact_for_a_pass() {
        use std::os::unix::ffi::OsStringExt;

        let dir = crate::test_tmp().join(format!("herdr-link-non-utf8-{}", std::process::id()));
        let cwd = dir.join(std::ffi::OsString::from_vec(b"assets-\xff".to_vec()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("plot.png"), "").unwrap();
        let active = vec![("files".into(), rules(json!([{"kind":"file", "pattern":r"\.png$"}])))];
        let raw = "./plot.png";
        let hit = resolve(raw, Some(&cwd)).click(false, &active).unwrap();
        assert_eq!(hit.handlers[0].1, json!({"kind":"file", "raw":raw, "path":null}));
        assert_eq!(hit.native, Some(Target::File(cwd.join(raw))));

        let uri = format!("file://{}/assets-%FF/plot.png", dir.display());
        let hit = resolve(&uri, None).click(false, &active).unwrap();
        assert_eq!(hit.handlers[0].1, json!({"kind":"file", "raw":uri, "path":null}));
        assert_eq!(hit.native, Some(Target::Url(uri)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dispatch_keeps_passing_in_order_and_stops_on_handled_or_error() {
        use std::{future::{Future, ready}, pin::pin, task::{Context, Poll, Waker}};

        for (second, expected, calls) in [
            (Ok("pass"), Ok(false), 3),
            (Ok("handled"), Ok(true), 2),
            (Ok("invalid"), Err(("second".to_owned(), "handleLink returned \"invalid\"; expected \"handled\" or \"pass\"".into())), 2),
            (Err("tunnel failed"), Err(("second".to_owned(), "tunnel failed".into())), 2),
        ] {
            let mut visited = Vec::new();
            let handlers = [("first".into(), Ok("pass")), ("second".into(), second), ("third".into(), Ok("pass"))];
            {
                let mut future = pin!(dispatch(handlers, |name, result| {
                    visited.push(name.to_owned());
                    ready(result.map(str::to_owned).map_err(str::to_owned))
                }));
                assert_eq!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(expected));
            }
            assert_eq!(visited, ["first", "second", "third"][..calls]);
        }
    }

    fn native_target(link: &str, cwd: Option<&Path>) -> Option<Target> {
        resolve(link, cwd).native
    }

    fn first(text: &str) -> Option<&str> {
        PATTERN.find(text).unwrap().map(|m| m.as_str())
    }

    /// Ghostty's own cases (`src/config/url.zig`).
    #[test]
    fn matches_like_ghostty() {
        let cases = [
            ("hello https://example.com world", "https://example.com"),
            ("https://example.com/foo(bar) more", "https://example.com/foo(bar)"),
            ("https://example.com/foo(bar)baz more", "https://example.com/foo(bar)baz"),
            ("Link inside (https://example.com) parens", "https://example.com"),
            ("Link period https://example.com. More text.", "https://example.com"),
            ("Link trailing comma https://example.com, more text.", "https://example.com"),
            ("Link in double quotes \"https://example.com\" and more", "https://example.com"),
            ("Link in single quotes 'https://example.com' and more", "https://example.com"),
            ("some file with https://google.com https://duckduckgo.com links.", "https://google.com"),
            ("and links in it. links https://yahoo.com mailto:test@example.com ssh://1.2.3.4", "https://yahoo.com"),
            ("also match http://example.com non-secure links", "http://example.com"),
            ("match tel://+12123456789 phone numbers", "tel://+12123456789"),
            ("match with query url https://example.com?query=1&other=2 and more text.", "https://example.com?query=1&other=2"),
            (
                "url with dashes [mode 2027](https://github.com/contour-terminal/terminal-unicode-core) for better unicode support",
                "https://github.com/contour-terminal/terminal-unicode-core",
            ),
            ("dot.http://example.com", "http://example.com"),
            ("weird characters https://example.com/~user/?query=1&other=2#hash and more", "https://example.com/~user/?query=1&other=2#hash"),
            ("square brackets https://example.com/[foo] and more", "https://example.com/[foo]"),
            (
                "[13]:TooManyStatements: TempFile#assign_temp_file_to_entity has approx 7 statements [https://example.com/docs/Too-Many-Statements.md]",
                "https://example.com/docs/Too-Many-Statements.md",
            ),
            ("match ftp://example.com ftp links", "ftp://example.com"),
            ("match file://example.com file links", "file://example.com"),
            ("match ssh://example.com ssh links", "ssh://example.com"),
            ("match git://example.com git links", "git://example.com"),
            ("/tmp/test.txt http://www.google.com", "/tmp/test.txt"),
            ("match tel:+18005551234 tel links", "tel:+18005551234"),
            ("match magnet:?xt=urn:btih:1234567890 magnet links", "magnet:?xt=urn:btih:1234567890"),
            ("match ipfs://QmSomeHashValue ipfs links", "ipfs://QmSomeHashValue"),
            ("match ipns://QmSomeHashValue ipns links", "ipns://QmSomeHashValue"),
            ("match gemini://example.com gemini links", "gemini://example.com"),
            ("match gopher://example.com gopher links", "gopher://example.com"),
            ("match news:comp.infosystems.www.servers.unix news links", "news:comp.infosystems.www.servers.unix"),
            ("/Users/ghostty.user/code/example.py", "/Users/ghostty.user/code/example.py"),
            ("/Users/ghostty.user/code/../example.py", "/Users/ghostty.user/code/../example.py"),
            ("/Users/ghostty.user/code/../example.py hello world", "/Users/ghostty.user/code/../example.py"),
            ("../example.py", "../example.py"),
            ("../example.py ", "../example.py"),
            ("first time ../example.py contributor ", "../example.py"),
            ("[link](/home/user/ghostty.user/example)", "/home/user/ghostty.user/example"),
            ("Serving HTTP on :: port 8000 (http://[::]:8000/)", "http://[::]:8000/"),
            ("IPv6 address https://[2001:db8::1]:8080/path", "https://[2001:db8::1]:8080/path"),
            ("IPv6 localhost http://[::1]:3000", "http://[::1]:3000"),
            ("Complex IPv6 https://[2001:db8:85a3:8d3:1319:8a2e:370:7348]:443/", "https://[2001:db8:85a3:8d3:1319:8a2e:370:7348]:443/"),
            ("IPv6 with path https://[2001:db8::1]/path/to/resource", "https://[2001:db8::1]/path/to/resource"),
            ("IPv6 with query https://[2001:db8::1]:8080/api?param=value&other=123", "https://[2001:db8::1]:8080/api?param=value&other=123"),
            ("IPv6 compressed http://[2001:db8::]:80/", "http://[2001:db8::]:80/"),
            ("IPv6 multiple zeros http://[2001:0:0:0:0:0:0:1]", "http://[2001:0:0:0:0:0:0:1]"),
            ("IPv6 link-local https://[fe80::1234:5678:9abc]", "https://[fe80::1234:5678:9abc]"),
            ("IPv6 multicast http://[ff02::1]/stream", "http://[ff02::1]/stream"),
            ("IPv6 in markdown [link](http://[2001:db8::1]/docs)", "http://[2001:db8::1]/docs"),
            ("./spaces-end.   ", "./spaces-end."),
            ("./space middle", "./space middle"),
            ("../test folder/file.txt", "../test folder/file.txt"),
            ("/tmp/test folder/file.txt", "/tmp/test folder/file.txt"),
            ("/tmp/test  folder/file.txt", "/tmp/test"),
            ("diff --git a/src/font/shaper/harfbuzz.zig b/src/font/shaper/harfbuzz.zig", "a/src/font/shaper/harfbuzz.zig"),
            ("/tmp/foo /tmp/bar", "/tmp/foo"),
            ("/tmp/foo.txt /tmp/bar.txt", "/tmp/foo.txt"),
            ("src/config/url.zig", "src/config/url.zig"),
            ("app/folder/file.rb:1", "app/folder/file.rb:1"),
            ("modified:   src/config/url.zig", "src/config/url.zig"),
            ("lib/ghostty/terminal.zig:42:10", "lib/ghostty/terminal.zig:42:10"),
            ("some-pkg/src/file.txt more text", "some-pkg/src/file.txt"),
            ("src/foo.c,baz.txt", "src/foo.c"),
            ("~/foo/bar.txt", "~/foo/bar.txt"),
            ("open ~/Documents/notes.md please", "~/Documents/notes.md"),
            ("~/.config/ghostty/config", "~/.config/ghostty/config"),
            ("directory: ~/src/ghostty-org/ghostty", "~/src/ghostty-org/ghostty"),
            ("$HOME/src/config/url.zig", "$HOME/src/config/url.zig"),
            ("space dir: $PWD/src/ghostty/main.zig", "$PWD/src/ghostty/main.zig"),
            ("foo/$BAR/baz", "foo/$BAR/baz"),
            (".foo/bar/$VAR", ".foo/bar/$VAR"),
            (".config/ghostty/config", ".config/ghostty/config"),
            ("loaded from .local/share/ghostty/state.db now", ".local/share/ghostty/state.db"),
            ("../some/where", "../some/where"),
            ("  - shared/src/foo/SomeItem.m:12, shared/src/", "shared/src/foo/SomeItem.m:12"),
            ("foo.local/share", "foo.local/share"),
            ("2024/report.txt", "2024/report.txt"),
            ("./foo bar,baz", "./foo bar"),
            ("/tmp/foo bar,baz", "/tmp/foo bar"),
            ("./.config/ghostty: Needs upstream (main)", "./.config/ghostty"),
            ("./Downloads: Operation not permitted", "./Downloads"),
        ];
        for (input, expect) in cases {
            assert_eq!(first(input), Some(expect), "{input:?}");
        }
        for input in ["input/output", "foo/bar", "$10/bar", "$10/$20", "$10/bar.txt", "foo/bar,baz.txt", "foo$BAR/baz.txt", "foo~/bar.txt", "// foo bar", "//foo"] {
            assert_eq!(first(input), None, "{input:?}");
        }
    }

    #[test]
    fn query_links_keep_their_full_target() {
        for url in ["https://example.com/?a=1&b=2", "https://localhost:17480/login?t=abc%3D%3D", "file:///C:/Windows/System32/winver.exe"] {
            let text = format!("Dashboard: {url}");
            let range = find(&text, text.find(url).unwrap()).unwrap();
            assert_eq!(&text[range.clone()], url);
            // winver.exe exists on Windows, where it opens as the file.
            let expected = match url.strip_prefix("file:///") {
                Some(path) if cfg!(windows) => Target::File(path.replace('/', "\\").into()),
                _ => Target::Url(url.into()),
            };
            assert_eq!(native_target(&text[range], None), Some(expected));
        }
    }

    #[test]
    fn editor_remote_links_keep_their_whole_uri() {
        for url in [
            "cursor://vscode-remote/ssh-remote+dev_host/data/1/user/my_project",
            "vscode://vscode-remote/ssh-remote+dev_host/data/1/user/my_project",
        ] {
            let text = format!("Open {url} to continue.");
            let at = text.find(url).unwrap();
            let range = find(&text, at).unwrap();
            assert_eq!(&text[range.clone()], url);
            assert_eq!(native_target(&text[range], None), Some(Target::Url(url.into())));
        }
    }

    #[test]
    fn finds_the_link_under_the_pointer() {
        let text = "see https://a.com and /tmp/x.txt.";
        assert_eq!(find(text, 0), None);
        assert_eq!(find(text, 4).map(|r| &text[r]), Some("https://a.com"));
        assert_eq!(find(text, 16).map(|r| &text[r]), Some("https://a.com"));
        assert_eq!(find(text, 17), None);
        assert_eq!(find(text, 25).map(|r| &text[r]), Some("/tmp/x.txt."));
    }

    #[test]
    fn extracts_address_words_at_the_pointer() {
        for (text, expected) in [
            ("<t1hg>", "t1hg"),
            ("[\"macbook/t2w\"]", "macbook/t2w"),
            ("terminal=t1hg,", "t1hg"),
            ("'my-machine_2/t2w'.", "my-machine_2/t2w"),
            ("before t1hg after", "t1hg"),
            ("é t1hg", "t1hg"),
            ("<build.box/t1>.", "build.box/t1"),
            ("1.2.3.4/t1", "1.2.3.4/t1"),
            ("...t1hg.", "t1hg"),
        ] {
            let at = text.find(expected).unwrap();
            for (i, _) in expected.char_indices() {
                assert_eq!(word(text, at + i).map(|r| &text[r]), Some(expected), "{text}");
            }
        }
        for (text, at) in [("<t1>", 0), ("t1 ", 2), ("", 0), ("t1", 2), ("é", 1)] {
            assert_eq!(word(text, at), None);
        }
    }

    #[test]
    fn matches_only_live_addresses_relative_to_the_source_machine() {
        let live = ["t1", "t2", "macbook/t1", "macbook/t2w", "my-machine_2/t3", "build.box/t1", "box/t1"];
        for (word, source, expected) in [
            ("t1", "t2", Some("t1")),
            ("t1", "macbook/t2w", Some("macbook/t1")),
            ("t1", "build.box/t2", Some("build.box/t1")),
            ("t2", "macbook/t1", None),
            ("macbook/t2w", "t1", Some("macbook/t2w")),
            ("my-machine_2/t3", "macbook/t1", Some("my-machine_2/t3")),
            ("t99", "t1", None),
            ("Build", "t1", None),
            ("t", "t1", None),
            ("other/t1", "t1", None),
            ("macbook/t1/extra", "t1", None),
        ] {
            assert_eq!(terminal_address(word, source, live), expected.map(str::to_owned), "{word} in {source}");
        }
    }

    #[test]
    fn dotted_machines_never_match_a_different_suffix_machine() {
        let text = "<build.box/t1>.";
        let range = word(text, text.find("t1").unwrap()).unwrap();
        assert_eq!(&text[range.clone()], "build.box/t1");
        assert_eq!(terminal_address(&text[range.clone()], "t2", ["box/t1"]), None);
        assert_eq!(terminal_address(&text[range], "t2", ["box/t1", "build.box/t1"]), Some("build.box/t1".into()));
    }

    #[test]
    fn files_resolve_only_when_they_exist() {
        let dir = std::env::temp_dir().join(format!("herdr-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        let file = Target::File(dir.join("src/main.rs"));
        assert_eq!(native_target("src/main.rs", Some(&dir)), Some(file.clone()));
        assert_eq!(native_target("src/main.rs:42:7", Some(&dir)), Some(file.clone()));
        assert_eq!(native_target("src/main.rs.", Some(&dir)), Some(file.clone()));
        assert_eq!(native_target("src/.", Some(&dir)), Some(Target::File(dir.join("src/."))));
        assert_eq!(native_target("src/..", Some(&dir)), Some(Target::File(dir.join("src/.."))));
        assert_eq!(native_target("$PWD/src/main.rs", Some(&dir)), Some(file.clone()));
        assert_eq!(native_target(dir.join("src/main.rs").to_str().unwrap(), None), Some(file));
        assert_eq!(native_target("src/gone.rs", Some(&dir)), None);
        assert_eq!(native_target("src/main.rs", None), None);
        assert_eq!(native_target("https://a.com/x", None), Some(Target::Url("https://a.com/x".into())));
        #[cfg(unix)]
        {
            let dotted = dir.join("src/main.rs.");
            std::fs::write(&dotted, "").unwrap();
            assert_eq!(native_target("src/main.rs.", Some(&dir)), Some(Target::File(dotted)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_urls_open_as_the_file_they_name() {
        let dir = std::env::temp_dir().join(format!("herdr-file-urls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("测试 #1.txt");
        std::fs::write(&path, "").unwrap();
        // As programs write them: every byte outside the unreserved set encoded.
        let slashed = path.to_str().unwrap().replace('\\', "/");
        let encoded: String = slashed
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let root = if cfg!(windows) { "/" } else { "" };
        let file = Some(Target::File(path.clone()));
        assert_eq!(native_target(&format!("file://{root}{encoded}"), None), file);
        assert_eq!(native_target(&format!("file://localhost{root}{encoded}"), None), file);
        assert_eq!(native_target(&format!("file://{root}{}", slashed.replace('#', "%23")), None), file);
        for url in [format!("file://{root}{encoded}.gone"), format!("file://{root}{encoded}%ZZ"), format!("file://box{root}{encoded}")] {
            assert_eq!(native_target(&url, None), Some(Target::Url(url.clone())), "{url}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

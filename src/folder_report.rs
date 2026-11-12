//! A program's own report of its terminal's folder: OSC 7 (`file://host/path`),
//! OSC 9;9 and OSC 1337 CurrentDir (a bare path). Shells send it at every prompt;
//! Pi's Teleport sends it when a session starts and after every move. It is the
//! only way to know where a program is when its process folder says nothing,
//! such as Pi on Windows, whose terminal's foreground is the PowerShell it runs in.

/// Where a program said its terminal is, and the machine it named, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub host: Option<String>,
    /// An absolute path in its machine's own form (`C:\src` on Windows).
    pub path: String,
}

/// Reads what the terminal stored for a report (raw, as the program sent it).
/// None: cleared (an empty report) or nothing usable, such as a relative path,
/// another URI scheme or broken escapes.
pub fn parse(raw: &str) -> Option<Report> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (host, path) = match raw.split_once("://") {
        Some((scheme, rest)) => {
            // kitty's shell integration sends the same URI under its own scheme.
            if !scheme.eq_ignore_ascii_case("file")
                && !scheme.eq_ignore_ascii_case("kitty-shell-cwd")
            {
                return None;
            }
            let (host, path) = rest.split_at(rest.find('/')?);
            // A literal ? or # starts a query or fragment; in a path it is escaped.
            let path = path.split(['?', '#']).next().unwrap_or_default();
            (Some(decode(host)?).filter(|h| !h.is_empty()), decode(path)?)
        }
        None => (None, raw.to_owned()),
    };
    Some(Report {
        host,
        path: native(&path)?,
    })
}

/// Whether a report's `host` is the machine whose own hostname is `name`: no
/// host or `localhost` always is; otherwise the names match, letter case and
/// domain aside (`box` is `box.lan`). None: `name` isn't known yet.
pub fn on_machine(host: Option<&str>, name: Option<&str>) -> Option<bool> {
    let Some(host) = host.filter(|h| !h.eq_ignore_ascii_case("localhost")) else {
        return Some(true);
    };
    let first = |n: &str| n.split('.').next().unwrap_or_default().to_ascii_lowercase();
    Some(first(host) == first(name?.trim()))
}

/// Percent-decoding of a URI part into UTF-8 text.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The path as its machine writes it: `/C:/src` and `C:/src` become `C:\src`;
/// Unix paths and UNC shares stay. Relative paths and NULs aren't folders.
fn native(path: &str) -> Option<String> {
    if path.contains('\0') {
        return None;
    }
    let drive = |p: &str| {
        p.len() >= 2
            && p.as_bytes()[0].is_ascii_alphabetic()
            && p.as_bytes()[1] == b':'
            && (p.len() == 2 || matches!(p.as_bytes()[2], b'/' | b'\\'))
    };
    let windows = path
        .strip_prefix('/')
        .filter(|p| drive(p))
        .or(Some(path).filter(|p| drive(p)));
    if let Some(p) = windows {
        let p = p.replace('/', "\\");
        let p = p.trim_end_matches('\\');
        // The drive's root keeps its separator: `C:\`.
        return Some(if p.len() == 2 {
            format!("{p}\\")
        } else {
            p.to_owned()
        });
    }
    if path.starts_with(r"\\") {
        return Some(path.to_owned());
    }
    if !path.starts_with('/') {
        return None;
    }
    let p = path.trim_end_matches('/');
    Some(if p.is_empty() {
        "/".to_owned()
    } else {
        p.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::{on_machine, parse, Report};

    fn report(host: Option<&str>, path: &str) -> Option<Report> {
        Some(Report {
            host: host.map(str::to_owned),
            path: path.to_owned(),
        })
    }

    #[test]
    fn reads_osc7_uris() {
        assert_eq!(
            parse("file://box/home/me/src"),
            report(Some("box"), "/home/me/src")
        );
        assert_eq!(
            parse("file:///home/me/a%20b/"),
            report(None, "/home/me/a b"),
            "escaped, no host, trailing slash"
        );
        assert_eq!(parse("file://box/"), report(Some("box"), "/"));
        assert_eq!(
            parse("kitty-shell-cwd://box/tmp"),
            report(Some("box"), "/tmp")
        );
        assert_eq!(
            parse("file://box/srv/%D0%BF%D1%80%D0%BE%D0%B5%D0%BA%D1%82"),
            report(Some("box"), "/srv/проект")
        );
        assert_eq!(
            parse("file://box/srv/a%23b?x=1#y"),
            report(Some("box"), "/srv/a#b")
        );
    }

    #[test]
    fn reads_windows_paths() {
        assert_eq!(
            parse("file://ZinLaptop/C:/Users/me/dev"),
            report(Some("ZinLaptop"), r"C:\Users\me\dev")
        );
        assert_eq!(parse("file:///c:/"), report(None, r"c:\"));
        assert_eq!(
            parse(r"C:\src\app\"),
            report(None, r"C:\src\app"),
            "OSC 9;9"
        );
        assert_eq!(
            parse(r"\\server\share\x"),
            report(None, r"\\server\share\x")
        );
    }

    #[test]
    fn reads_bare_paths() {
        assert_eq!(
            parse("/home/me"),
            report(None, "/home/me"),
            "OSC 1337 CurrentDir"
        );
    }

    #[test]
    fn rejects_what_is_no_folder() {
        for raw in [
            "",
            "  ",
            "relative/dir",
            "https://box/x",
            "file://box",
            "file://box/a%zz",
            "file://box/a%ff",
            "file://box/a%00b",
            "C:relative",
        ] {
            assert_eq!(parse(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn matches_the_machine_by_hostname() {
        assert_eq!(on_machine(None, None), Some(true));
        assert_eq!(on_machine(Some("LOCALHOST"), None), Some(true));
        assert_eq!(on_machine(Some("ZinLaptop"), None), None, "not known yet");
        assert_eq!(
            on_machine(Some("ZinLaptop"), Some("zinlaptop\r\n")),
            Some(true)
        );
        assert_eq!(on_machine(Some("box.lan"), Some("box")), Some(true));
        assert_eq!(on_machine(Some("vps"), Some("ZinLaptop")), Some(false));
    }
}

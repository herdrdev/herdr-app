//! The interface language. Every text run a mod draws goes through
//! `translate` (the hook in gpui-shell, installed by `install`), so the shipped
//! mods stay written in English and a language is just a table:
//! `defaults/i18n/<code>.json` with `exact` strings and `patterns` where `{}`
//! stands for a piece that varies (a name, a count).
//!
//! English is the default and the fallback: a string with no entry is drawn as
//! written. Set `HERDR_I18N_DUMP=<file>` to append every string that has no
//! entry in the chosen language to that file (one JSON string per line).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, RwLock};

/// The languages the settings offer: (code in settings.json, name in its own language).
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("zh-CN", "简体中文")];

const ZH_CN: &str = include_str!("../defaults/i18n/zh-CN.json");

static CURRENT: RwLock<String> = RwLock::new(String::new());

struct Pattern {
    /// The literal pieces around the `{}`s, in order (one more than there are holes).
    parts: Vec<String>,
    to: String,
}

struct Table {
    exact: HashMap<String, String>,
    /// Keys in lowercase, for text a mod upper-cased (section headings).
    folded: HashMap<String, String>,
    /// Words that may stand where a pattern has a {} (directions), never alone.
    pieces: HashMap<String, String>,
    patterns: Vec<Pattern>,
}

fn load(json: &str) -> Table {
    let v: serde_json::Value = serde_json::from_str(json).expect("defaults/i18n table is valid JSON");
    let strings = |key: &str| -> Vec<(String, String)> {
        v.get(key)
            .and_then(|m| m.as_object())
            .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect())
            .unwrap_or_default()
    };
    let exact: HashMap<String, String> = strings("exact").into_iter().collect();
    let folded = exact.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    let mut patterns: Vec<Pattern> = strings("patterns")
        .into_iter()
        .map(|(from, to)| Pattern { parts: from.split("{}").map(str::to_owned).collect(), to })
        .collect();
    // The most specific first: more literal text wins.
    patterns.sort_by_key(|p| std::cmp::Reverse(p.parts.iter().map(String::len).sum::<usize>()));
    let pieces = strings("pieces").into_iter().collect();
    Table { exact, folded, pieces, patterns }
}

fn table_for(code: &str) -> Option<&'static Table> {
    static ZH: OnceLock<Table> = OnceLock::new();
    match code {
        "zh-CN" => Some(ZH.get_or_init(|| load(ZH_CN))),
        _ => None,
    }
}

/// A language code the app knows, else English.
pub fn normalize(code: &str) -> &'static str {
    LANGUAGES.iter().map(|(c, _)| *c).find(|c| c.eq_ignore_ascii_case(code)).unwrap_or("en")
}

pub fn set_language(code: &str) {
    *CURRENT.write().unwrap() = normalize(code).to_owned();
}

pub fn language() -> String {
    let c = CURRENT.read().unwrap();
    if c.is_empty() { "en".into() } else { c.clone() }
}

/// Puts the translator where mods' text goes through it.
pub fn install() {
    gpui_shell::set_text_translator(translate);
}

/// `pattern` against `text`: what each `{}` stood for, in order.
fn capture<'a>(parts: &[String], text: &'a str) -> Option<Vec<&'a str>> {
    let (first, rest) = parts.split_first()?;
    let mut at = text.strip_prefix(first.as_str())?;
    let mut out = Vec::new();
    for (i, part) in rest.iter().enumerate() {
        if i == rest.len() - 1 {
            // The last literal piece ends the text.
            out.push(at.strip_suffix(part.as_str())?);
        } else {
            let end = at.find(part.as_str())?;
            out.push(&at[..end]);
            at = &at[end + part.len()..];
        }
    }
    Some(out)
}

fn lookup(t: &Table, text: &str) -> Option<String> {
    if let Some(s) = t.exact.get(text) {
        return Some(s.clone());
    }
    if text.chars().any(|c| c.is_alphabetic()) && text == text.to_uppercase() {
        if let Some(s) = t.folded.get(&text.to_lowercase()) {
            return Some(s.clone());
        }
    }
    for p in &t.patterns {
        if p.parts.len() < 2 {
            continue;
        }
        if let Some(found) = capture(&p.parts, text) {
            let mut out = String::new();
            let mut it = found.into_iter();
            for (i, piece) in p.to.split("{}").enumerate() {
                if i > 0 {
                    // A piece that is itself text (a state word) is translated too.
                    let v = it.next().unwrap_or("");
                    out.push_str(&t.pieces.get(v).cloned().or_else(|| lookup(t, v)).unwrap_or_else(|| v.to_owned()));
                }
                out.push_str(piece);
            }
            return Some(out);
        }
    }
    None
}

fn dump(text: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let Ok(path) = std::env::var("HERDR_I18N_DUMP") else { return };
    if !text.chars().any(|c| c.is_alphabetic()) {
        return;
    }
    let mut seen = SEEN.get_or_init(Default::default).lock().unwrap();
    if seen.insert(text.to_owned()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{}", serde_json::to_string(text).unwrap_or_default());
        }
    }
}

/// What to draw for `text` in the chosen language: `None` keeps it as written.
pub fn translate(text: &str) -> Option<String> {
    let code = language();
    let Some(table) = table_for(&code) else {
        return None;
    };
    // Keep the padding a mod put around a phrase.
    let core = text.trim();
    if core.is_empty() {
        return None;
    }
    match lookup(table, core) {
        Some(s) => {
            let lead = &text[..text.len() - text.trim_start().len()];
            let trail = &text[text.trim_end().len()..];
            Some(format!("{lead}{s}{trail}"))
        }
        None => {
            dump(core);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zh(s: &str) -> Option<String> {
        lookup(table_for("zh-CN").unwrap(), s)
    }

    #[test]
    fn exact_and_upper_cased_text_translate() {
        assert_eq!(zh("Look").as_deref(), Some("外观"));
        assert_eq!(zh("LOOK").as_deref(), Some("外观"));
        assert_eq!(zh("no such text anywhere"), None);
    }

    #[test]
    fn patterns_fill_their_holes() {
        let t = load(r#"{"patterns": {"Remove {}? Its terminals keep running there.": "移除 {}?", "{} agents": "{} 个 Agent", "{}/{} checkouts": "{}/{}"}}"#);
        assert_eq!(lookup(&t, "Remove vps? Its terminals keep running there.").as_deref(), Some("移除 vps?"));
        assert_eq!(lookup(&t, "3 agents").as_deref(), Some("3 个 Agent"));
        assert_eq!(lookup(&t, "1/5 checkouts").as_deref(), Some("1/5"));
        assert_eq!(lookup(&t, "agents"), None);
    }

    #[test]
    fn other_languages_fall_back_to_english() {
        assert_eq!(normalize("fr"), "en");
        assert_eq!(normalize("zh-cn"), "zh-CN");
    }

    #[test]
    fn the_shipped_table_loads() {
        let t = table_for("zh-CN").unwrap();
        assert!(!t.exact.is_empty());
    }
}

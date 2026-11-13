//! Explicit interface messages. Never translate arbitrary rendered text or user data.
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

struct Language {
    code: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
}

const LANGUAGES: &[Language] = &[
    Language {
        code: "en",
        name: "English",
        aliases: &["en"],
    },
    Language {
        code: "zh-Hans",
        name: "简体中文",
        aliases: &["zh-hans", "zh-cn", "zh-sg", "zh"],
    },
    Language {
        code: "zh-Hant",
        name: "繁體中文",
        aliases: &["zh-hant", "zh-tw", "zh-hk", "zh-mo"],
    },
];

pub fn languages() -> Value {
    Value::Array(
        LANGUAGES
            .iter()
            .map(|l| json!({"code": l.code, "nativeName": l.name}))
            .collect(),
    )
}

fn supported(raw: &str) -> Option<&'static str> {
    let tag = raw
        .trim()
        .split(['.', '@'])
        .next()?
        .replace('_', "-")
        .to_ascii_lowercase();
    let parts: Vec<_> = tag.split('-').collect();
    // A script is more specific than a region (e.g. zh-Hant-CN).
    if parts.first() == Some(&"zh") {
        for script in ["hans", "hant"] {
            if parts.contains(&script) {
                let alias = format!("zh-{script}");
                return LANGUAGES
                    .iter()
                    .find(|l| l.aliases.contains(&alias.as_str()))
                    .map(|l| l.code);
            }
        }
    }
    // Match most specific prefix first, then the base language.
    (1..=parts.len()).rev().find_map(|n| {
        let candidate = parts[..n].join("-");
        LANGUAGES
            .iter()
            .find(|l| l.aliases.contains(&candidate.as_str()))
            .map(|l| l.code)
    })
}

fn resolve<'a>(
    preference: Option<&str>,
    system: impl IntoIterator<Item = &'a str>,
) -> &'static str {
    if let Some(pref) = preference.filter(|p| !p.eq_ignore_ascii_case("system")) {
        if let Some(code) = explicit_preference(pref) {
            return code;
        }
        tracing::warn!(
            language = pref,
            "unknown interface language; following system"
        );
    }
    system.into_iter().find_map(supported).unwrap_or("en")
}

static CURRENT: OnceLock<&'static str> = OnceLock::new();
static PREFERENCE: OnceLock<&'static str> = OnceLock::new();

fn explicit_preference(raw: &str) -> Option<&'static str> {
    if raw.eq_ignore_ascii_case("zh-CN") {
        return supported(raw);
    }
    LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(raw)).map(|l| l.code)
}

pub fn init(preference: Option<&str>) {
    let _ = PREFERENCE.set(preference.and_then(explicit_preference).unwrap_or("system"));
    let system: Vec<_> = sys_locale::get_locales().collect();
    let _ = CURRENT.set(resolve(preference, system.iter().map(String::as_str)));
}

/// Checkers and tests default to English without querying the host OS.
pub fn locale() -> &'static str {
    CURRENT.get().copied().unwrap_or("en")
}
pub fn preference() -> &'static str {
    PREFERENCE.get().copied().unwrap_or("system")
}

type Bundle = FluentBundle<FluentResource>;
static BUNDLES: OnceLock<Vec<Bundle>> = OnceLock::new();

fn bundles() -> &'static [Bundle] {
    BUNDLES.get_or_init(|| {
        LANGUAGES
            .iter()
            .map(|language| {
                let mut bundle =
                    Bundle::new_concurrent(vec![language.code.parse().expect("registered locale")]);
                // Paths and terminal names must keep their exact bytes in the output.
                bundle.set_use_isolating(false);
                for (path, bytes) in LOCALES {
                    if path.starts_with(&format!("{}/", language.code)) {
                        let resource =
                            FluentResource::try_new(String::from_utf8_lossy(bytes).into_owned())
                                .expect("valid bundled Fluent resource");
                        bundle.add_resource(resource).expect("unique message IDs");
                    }
                }
                #[cfg(test)]
                if language.code == "en" {
                    bundle
                        .add_resource(
                            FluentResource::try_new(
                                "test-english-fallback = English fallback".into(),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                }
                bundle
            })
            .collect()
    })
}

fn warn_once(code: &str, key: &str, problem: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(format!("{code}:{key}:{problem}"))
    {
        tracing::warn!(
            language = code,
            key,
            problem,
            "interface translation fallback"
        );
    }
}

pub fn translate(key: &str, params: &Value) -> String {
    translate_in(locale(), key, params)
}
pub fn text(key: &str) -> String {
    translate(key, &Value::Null)
}

pub fn translate_in(code: &str, key: &str, params: &Value) -> String {
    let mut args = FluentArgs::new();
    if let Some(params) = params.as_object() {
        for (name, value) in params {
            match value {
                Value::String(s) => args.set(name, s.as_str()),
                Value::Number(n) => {
                    if let Some(n) = n.as_f64() {
                        args.set(name, n);
                    }
                }
                _ => {}
            }
        }
    }
    let index = LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0);
    for i in [index, 0].into_iter().take(if index == 0 { 1 } else { 2 }) {
        let bundle = &bundles()[i];
        if let Some(pattern) = bundle.get_message(key).and_then(|m| m.value()) {
            let mut errors = Vec::new();
            let value = bundle.format_pattern(pattern, Some(&args), &mut errors);
            if errors.is_empty() {
                return value.into_owned();
            }
            warn_once(LANGUAGES[i].code, key, "format");
        } else {
            warn_once(LANGUAGES[i].code, key, "missing");
        }
    }
    key.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_preferences_and_aliases() {
        for (raw, expected) in [
            ("en_US.UTF-8", "en"),
            ("zh-CN", "zh-Hans"),
            ("zh_SG", "zh-Hans"),
            ("zh", "zh-Hans"),
            ("zh-TW", "zh-Hant"),
            ("zh-HK", "zh-Hant"),
            ("zh-MO", "zh-Hant"),
            ("zh-Hant-CN", "zh-Hant"),
            ("zh-Hans-TW", "zh-Hans"),
        ] {
            assert_eq!(supported(raw), Some(expected));
        }
        assert_eq!(resolve(None, ["fr", "zh-TW"]), "zh-Hant");
        assert_eq!(resolve(Some("en"), ["zh-TW"]), "en");
        assert_eq!(resolve(Some("unknown"), ["zh-CN"]), "zh-Hans");
        assert_eq!(resolve(Some("system"), ["C", "POSIX"]), "en");
        assert_eq!(resolve(None, []), "en");
    }

    #[test]
    fn resources_have_matching_messages_and_variables() {
        fn shape(code: &str) -> std::collections::BTreeMap<String, HashSet<String>> {
            let mut result = std::collections::BTreeMap::new();
            for (path, bytes) in LOCALES {
                if !path.starts_with(&format!("{code}/")) {
                    continue;
                }
                let source = std::str::from_utf8(bytes).unwrap();
                fluent_syntax::parser::parse(source).expect("valid Fluent");
                let mut id = String::new();
                for line in source.lines() {
                    if line.starts_with('#') || line.trim().is_empty() {
                        continue;
                    }
                    if !line.starts_with(char::is_whitespace) {
                        id = line.split(" =").next().unwrap().to_owned();
                        assert!(
                            result.insert(id.clone(), HashSet::new()).is_none(),
                            "duplicate {id}"
                        );
                    }
                    for tail in line.split('$').skip(1) {
                        let name: String = tail
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                            .collect();
                        result.get_mut(&id).unwrap().insert(name);
                    }
                }
            }
            result
        }
        let en = shape("en");
        assert!(!en.is_empty());
        for code in ["zh-Hans", "zh-Hant"] {
            assert_eq!(shape(code), en);
        }
        let _ = bundles();
    }

    #[test]
    fn user_parameters_are_not_translated() {
        let name = "Settings { $name } / 文件";
        assert!(
            translate_in(
                "zh-Hans",
                "settings-current-language",
                &json!({"name": name})
            )
            .contains(name)
        );
        assert_eq!(
            translate_in("zh-Hans", "missing-message", &Value::Null),
            "missing-message"
        );
        assert_eq!(
            translate_in("zh-Hans", "settings-current-language", &Value::Null),
            "settings-current-language"
        );
        assert_eq!(
            translate_in("zh-Hant", "test-english-fallback", &Value::Null),
            "English fallback"
        );
    }

    #[test]
    fn plural_rules_and_traditional_labels() {
        assert_eq!(
            translate_in("en", "terminal-match-count", &json!({"count":1})),
            "1 match"
        );
        assert_eq!(
            translate_in("en", "terminal-match-count", &json!({"count":2})),
            "2 matches"
        );
        assert_eq!(
            translate_in("zh-Hans", "terminal-match-count", &json!({"count":2})),
            "2 个匹配"
        );
        assert_eq!(
            translate_in("zh-Hant", "settings-language-label", &Value::Null),
            "介面語言"
        );
    }

    #[test]
    fn every_message_formats_with_its_declared_arguments() {
        for (path, bytes) in LOCALES {
            let code = path.split('/').next().unwrap();
            let source = std::str::from_utf8(bytes).unwrap();
            let mut params = serde_json::Map::new();
            for tail in source.split('$').skip(1) {
                let name: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                params.insert(name, json!(1));
            }
            for line in source
                .lines()
                .filter(|line| line.starts_with(|c: char| c.is_ascii_lowercase()))
            {
                let key = line.split(" =").next().unwrap();
                let value = translate_in(code, key, &Value::Object(params.clone()));
                assert_ne!(value, key, "{path}: {key} failed to format");
            }
        }
    }
}

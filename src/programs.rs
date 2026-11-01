//! What runs in a terminal, as people see it: a name, an icon and a tile.
//! One table for every mod: `defaults/programs.json` (built in), with the
//! person's `programs.json` in the home folder on top (entry by entry). The
//! shipped icons are written to `icons/` in the home folder (the app's;
//! rewritten when they change). Mods read the result as `program` on
//! panes, agents and terminals.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;

const BUILT_IN: &str = include_str!("../defaults/programs.json");
include!(concat!(env!("OUT_DIR"), "/default_icons.rs"));

#[derive(Deserialize, Default)]
struct Table {
    #[serde(default)]
    shells: Vec<String>,
    #[serde(default)]
    interpreters: Vec<String>,
    #[serde(default)]
    programs: BTreeMap<String, Entry>,
}

#[derive(Deserialize, Serialize, Default, Clone)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// A path in the home folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    /// A one-color glyph, drawn in `color` (or the text color); else a picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    /// Other process names (or agents' 7501 app names) for this entry.
    #[serde(default, rename = "match", skip_serializing_if = "Vec::is_empty")]
    also: Vec<String>,
    /// How to start it, for an agent people can start from a menu (`knownAgents()`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    run: Vec<String>,
}

impl Entry {
    fn over(&mut self, mine: Entry) {
        self.name = mine.name.or(self.name.take());
        self.icon = mine.icon.or(self.icon.take());
        self.tint = mine.tint.or(self.tint);
        self.tile = mine.tile.or(self.tile.take());
        self.color = mine.color.or(self.color.take());
        if !mine.also.is_empty() {
            self.also = mine.also;
        }
        if !mine.run.is_empty() {
            self.run = mine.run;
        }
    }
}

/// The agents people pick most, first in menus; the rest follow by name.
const AGENTS_FIRST: &[&str] = &["claude", "codex", "pi", "opencode", "cursor", "copilot", "amp", "gemini", "grok"];

/// Every agent the table can start (entries with `run`), most picked first:
/// `{ key, name, icon, tint, tile, color, run }`.
pub fn startable() -> Vec<Value> {
    TABLE.with(|t| {
        let t = t.borrow();
        let mut keys: Vec<&String> = t.programs.iter().filter(|(_, e)| !e.run.is_empty()).map(|(k, _)| k).collect();
        keys.sort_by_key(|k| (AGENTS_FIRST.iter().position(|f| f == k).unwrap_or(AGENTS_FIRST.len()), k.to_string()));
        keys.into_iter()
            .map(|k| {
                let e = &t.programs[k];
                let mut v = shown(k, e, None, false, true);
                v["run"] = json!(e.run);
                v
            })
            .collect()
    })
}

/// Which of `commands` this machine can start, asked of the shell agents start
/// in (so an app opened from the Dock finds what the person's terminal finds).
/// On another machine (`on`): its login/interactive shell, as remote agents start.
pub fn installed(commands: &[String], on: Option<&str>) -> Vec<String> {
    if commands.is_empty() {
        return Vec::new();
    }
    // Each found one as a marked line: an interactive shell may print its own
    // things (a prompt's escapes, a greeting) around the answer.
    let out = match on {
        Some(m) => {
            let argv = crate::remote_shell::check(commands);
            crate::mods::run_on(m, &argv[0], &argv[1..]).unwrap_or_default()
        }
        None if cfg!(windows) => {
            let list = commands.iter().map(|c| format!("'{}'", c.replace('\'', "''"))).collect::<Vec<_>>().join(",");
            let script = format!("foreach($c in @({list})){{ if(Get-Command $c -ErrorAction SilentlyContinue){{ \"herdr-has:$c\" }} }}");
            crate::mods::run_here(&crate::window::shell(), &["-NoLogo".into(), "-Command".into(), script]).unwrap_or_default()
        }
        None => {
            let shell = crate::window::shell();
            let line = installed_line(commands, base(&shell) == "fish");
            crate::mods::run_here(&shell, &["-ic".into(), line]).unwrap_or_default()
        }
    };
    let found: std::collections::HashSet<&str> = out.split("herdr-has:").skip(1).filter_map(|s| s.split_whitespace().next()).collect();
    commands.iter().filter(|c| found.contains(c.as_str())).cloned().collect()
}

/// Check in the shell itself for functions, but not aliases (`"$@"` cannot run them).
fn installed_line(commands: &[String], fish: bool) -> String {
    commands
        .iter()
        .map(|c| {
            let word = format!("'{}'", c.replace('\'', "'\\''"));
            let unalias = if fish { String::new() } else { format!("unalias -- {word} 2>/dev/null; ") };
            format!("{unalias}command -v {word} >/dev/null 2>&1 && printf 'herdr-has:%s\\n' {word};")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

thread_local! {
    static TABLE: RefCell<Table> = RefCell::new(serde_json::from_str(BUILT_IN).expect("defaults/programs.json"));
}

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("programs.json")
}

/// Writes the shipped icons into the home folder.
pub fn install_icons() {
    let dir = crate::mods::app_dir().join("icons");
    let _ = std::fs::create_dir_all(&dir);
    for (name, bytes) in DEFAULT_ICONS {
        let file = dir.join(name);
        if std::fs::read(&file).ok().as_deref() != Some(*bytes) {
            let _ = crate::persist::write_atomic(&file, bytes);
        }
    }
}

/// Reads the person's table on top of the built-in one. Err: what's wrong with their file.
pub fn load() -> Result<(), String> {
    let mut table: Table = serde_json::from_str(BUILT_IN).expect("defaults/programs.json");
    let result = match std::fs::read_to_string(path()) {
        Err(_) => Ok(()),
        Ok(text) => match serde_json::from_str::<Table>(&text) {
            Err(e) => Err(format!("programs.json: {e}")),
            Ok(mine) => {
                table.shells.extend(mine.shells);
                table.interpreters.extend(mine.interpreters);
                for (key, entry) in mine.programs {
                    table.programs.entry(key).or_default().over(entry);
                }
                Ok(())
            }
        },
    };
    TABLE.with(|t| *t.borrow_mut() = table);
    result
}

/// How a terminal shows: from the agent's own report (`app`), else the
/// program it runs (`argv`). `{ key, name, icon, tint, tile, color, shell, agent }`.
pub fn of(app: Option<&str>, argv: &[String]) -> Value {
    TABLE.with(|t| resolve(&t.borrow(), app, argv))
}

/// An agent's 7501 `app` as a name Herdr ships (`claude` for `claude-code`),
/// else `other` (`unknown` when it gave none): never the person's own words.
pub fn agent_name(app: Option<&str>) -> String {
    static NAMES: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| {
        let table: Table = serde_json::from_str(BUILT_IN).expect("defaults/programs.json");
        let mut names = BTreeMap::new();
        for (key, e) in table.programs {
            for alias in &e.also {
                names.insert(alias.to_lowercase(), key.clone());
            }
            names.insert(key.to_lowercase(), key);
        }
        names
    });
    match app.map(str::trim).filter(|a| !a.is_empty()) {
        None => "unknown".into(),
        Some(a) => names.get(&a.to_lowercase()).cloned().unwrap_or_else(|| "other".into()),
    }
}

fn base(s: &str) -> &str {
    let s = s.rsplit(['/', '\\']).next().unwrap_or(s);
    let s = s.trim_start_matches('-');
    // Windows: `pwsh.exe` is `pwsh`.
    match s.len().checked_sub(4) {
        Some(i) if s.is_char_boundary(i) && s[i..].eq_ignore_ascii_case(".exe") => &s[..i],
        _ => s,
    }
}

/// PowerShell's single quotes, which include the curly ones.
const PS_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// `& 'prog' 'arg 1'`: a command for PowerShell's `-Command`, each word quoted.
pub fn powershell_line(cmd: &[String]) -> String {
    let quote = |w: &String| {
        let mut s = String::from("'");
        for ch in w.chars() {
            if PS_QUOTES.contains(&ch) {
                s.push(ch);
            }
            s.push(ch);
        }
        s + "'"
    };
    format!("& {}", cmd.iter().map(quote).collect::<Vec<_>>().join(" "))
}

/// The words back from `powershell_line`.
fn powershell_words(line: &str) -> Option<Vec<String>> {
    let mut chars = line.strip_prefix("& ")?.chars().peekable();
    let mut words = Vec::new();
    while let Some(c) = chars.next() {
        if c == ' ' {
            continue;
        }
        if !PS_QUOTES.contains(&c) {
            return None;
        }
        let mut w = String::new();
        while let Some(ch) = chars.next() {
            if PS_QUOTES.contains(&ch) {
                if chars.peek().is_some_and(|n| PS_QUOTES.contains(n)) {
                    w.push(chars.next().unwrap_or(ch));
                    continue;
                }
                break;
            }
            w.push(ch);
        }
        words.push(w);
    }
    Some(words)
}

fn find<'a>(t: &'a Table, name: &str) -> Option<(&'a str, &'a Entry)> {
    let lower = name.to_lowercase();
    t.programs
        .iter()
        .find(|(k, _)| k.as_str() == name || k.to_lowercase() == lower)
        .or_else(|| t.programs.iter().find(|(_, e)| e.also.iter().any(|a| a == name || a.to_lowercase() == lower)))
        .map(|(k, e)| (k.as_str(), e))
}

fn resolve(t: &Table, app: Option<&str>, argv: &[String]) -> Value {
    if let Some(app) = app.filter(|a| !a.is_empty()) {
        return match find(t, app) {
            Some((key, e)) => shown(key, e, None, false, true),
            None => shown("agent", t.programs.get("agent").unwrap_or(&Entry::default()), Some(app), false, true),
        };
    }
    // The app starts commands in the person's shell: `zsh -ic '"$@"; exec "$0" -i' zsh <cmd>...`.
    if argv.len() > 4
        && t.shells.iter().any(|s| s == base(&argv[0]))
        && argv[1].starts_with('-')
        && argv[1].ends_with('c')
        && (argv[2].contains("\"$@\"") || argv[2] == "command $argv[2..]; exec $argv[1] -i")
    {
        return resolve(t, None, &argv[4..]);
    }
    // On Windows: `pwsh -NoExit -Command "& 'codex' ..."`.
    if argv.len() == 4
        && t.shells.iter().any(|s| s == base(&argv[0]))
        && argv[1] == "-NoExit"
        && argv[2] == "-Command"
        && let Some(words) = powershell_words(&argv[3])
    {
        return resolve(t, None, &words);
    }
    let Some(first) = argv.first().map(|a| base(a)) else {
        return shown("shell", t.programs.get("shell").unwrap_or(&Entry::default()), None, true, false);
    };
    // `node .../vite.js dev`, `npx vite`, `uv run pytest`: the program they run, when it's known.
    if t.interpreters.iter().any(|i| i == first) {
        for arg in argv[1..].iter().filter(|a| !a.starts_with('-')) {
            // A script's name without its extension (`vite.js`), never a module path (`http.server`).
            let name = base(arg);
            let stem = [".js", ".mjs", ".cjs", ".ts", ".py", ".rb"].iter().find_map(|x| name.strip_suffix(x)).unwrap_or(name);
            if let Some((key, e)) = find(t, stem).filter(|(k, _)| !t.interpreters.iter().any(|i| i == k)) {
                return shown(key, e, None, false, false);
            }
        }
    }
    if let Some((key, e)) = find(t, first) {
        return shown(key, e, None, false, false);
    }
    if t.shells.iter().any(|s| s == first) {
        return shown("shell", t.programs.get("shell").unwrap_or(&Entry::default()), Some(first), true, false);
    }
    let mut e = t.programs.get("program").cloned().unwrap_or_default();
    e.tile = Some(hash_color(first).into());
    e.color = Some("#ffffff".into());
    shown("program", &e, Some(first), false, false)
}

fn shown(key: &str, e: &Entry, name: Option<&str>, shell: bool, agent: bool) -> Value {
    json!({
        "key": key,
        "name": name.map(str::to_owned).or_else(|| e.name.clone()).unwrap_or_else(|| key.to_owned()),
        "icon": e.icon,
        "tint": e.tint.unwrap_or(true),
        "tile": e.tile,
        "color": e.color,
        "shell": shell,
        "agent": agent,
    })
}

/// A color for a program nobody listed, the same every time.
fn hash_color(s: &str) -> &'static str {
    const PALETTE: [&str; 8] = ["#6d5bd0", "#2f80ed", "#0f9d8a", "#d9822b", "#c2417d", "#3a7d44", "#8a5a44", "#4e5d94"];
    let h = s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Table {
        serde_json::from_str(BUILT_IN).unwrap()
    }
    fn key(app: Option<&str>, argv: &[&str]) -> String {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        resolve(&t(), app, &argv)["key"].as_str().unwrap().to_owned()
    }

    #[test]
    fn powershell_words_survive_quotes() {
        let words: Vec<String> = ["pi", "it's \u{2019}fine\u{2019}", "", "a  b"].map(String::from).to_vec();
        let line = powershell_line(&words);
        assert_eq!(line, "& 'pi' 'it''s \u{2019}\u{2019}fine\u{2019}\u{2019}' '' 'a  b'");
        assert_eq!(powershell_words(&line), Some(words));
    }

    #[test]
    fn agents_shells_tools_and_what_interpreters_run() {
        assert_eq!(key(Some("claude-code"), &["/bin/zsh"]), "claude");
        assert_eq!(key(Some("grok-build"), &["/bin/zsh"]), "grok");
        assert_eq!(key(Some("newagent"), &[]), "agent");
        assert_eq!(key(None, &["/bin/zsh"]), "shell");
        assert_eq!(key(None, &["-zsh"]), "shell");
        assert_eq!(key(None, &["nvim", "x.rs"]), "nvim");
        assert_eq!(key(None, &["vi"]), "vim");
        assert_eq!(key(None, &["node", "/x/node_modules/.bin/vite", "dev"]), "vite");
        assert_eq!(key(None, &["npx", "vitest"]), "vitest");
        assert_eq!(key(None, &["node", "server.js"]), "node");
        assert_eq!(key(None, &["python3", "-m", "http.server", "8799"]), "python");
        assert_eq!(key(None, &["uv", "run", "pytest"]), "pytest");
        assert_eq!(key(None, &["mytool"]), "program");
        assert_eq!(key(None, &["/bin/zsh", "-ic", "\"$@\"; exec \"$0\" -i", "/bin/zsh", "vi", "x"]), "vim");
        assert_eq!(key(None, &["/usr/bin/fish", "-ic", "command $argv[2..]; exec $argv[1] -i", "/usr/bin/fish", "vi", "x"]), "vim");
        let line = powershell_line(&["codex".into(), "fix it".into()]);
        assert_eq!(key(None, &["pwsh.exe", "-NoExit", "-Command", &line]), "codex");
        assert_eq!(key(None, &[r"C:\Windows\powershell.exe"]), "shell");
    }

    #[cfg(unix)]
    #[test]
    fn installed_checks_functions_and_quoted_names_but_not_aliases() {
        let commands = ["issue57_function", "issue57_alias", "printf", "missing'command; false"].map(String::from);
        let line = installed_line(&commands, false);
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("issue57_function() {{ :; }}; alias issue57_alias='printf alias'; {line}")])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "herdr-has:issue57_function\nherdr-has:printf\n");
    }

    #[test]
    fn every_entry_has_its_icon() {
        for (k, e) in t().programs {
            let icon = e.icon.unwrap_or_else(|| panic!("{k} has no icon"));
            let file = icon.strip_prefix("icons/").unwrap();
            assert!(DEFAULT_ICONS.iter().any(|(n, _)| *n == file), "{k}: {icon} not shipped");
        }
    }
}

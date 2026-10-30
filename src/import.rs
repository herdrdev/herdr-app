//! Importing herdr's workspaces: `import.herdr`.
//!
//! herdr (the terminal app before this one) keeps its layout in
//! `~/.config/herdr/session.json`. A workspace becomes a space, a tab a tab, and
//! a pane that recorded an agent is reopened with the flag that resumes it, so
//! the conversation continues instead of starting over.
//!
//! Nothing is copied: an agent resumes the session file it wrote. That is also
//! why this only reads the file: a session herdr is running right now must be
//! left alone, and that decision belongs to whoever asks for the import.

use serde_json::Value;

/// A tab to make: its name (empty: named by number), where it starts, and what
/// it runs (nothing: a shell).
pub struct Tab {
    pub name: String,
    pub folder: String,
    pub argv: Option<Vec<String>>,
}

/// A workspace to make, with its tabs in order.
pub struct Space {
    pub name: String,
    pub folder: String,
    pub tabs: Vec<Tab>,
}

/// Where herdr keeps its layout. The same shape on macOS, Linux and Windows.
pub fn default_path() -> String {
    format!(
        "{}/.config/herdr/session.json",
        crate::mods::home().display()
    )
}

/// The flag that reopens a conversation an agent left, and the kind of session
/// value it takes. Only agents whose own command can do it are listed: anything
/// else opens as a shell rather than pretending to resume.
fn resume(agent: &str) -> Option<(&'static str, &'static str)> {
    match agent {
        "pi" => Some(("--session", "path")),
        "claude" | "claude-code" => Some(("--resume", "id")),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// A pane's command: the agent started again on the session it recorded, if its
/// own flags allow it.
fn argv(pane: &Value) -> Option<Vec<String>> {
    let session = pane.get("agent_session")?;
    let agent = text(session.get("agent"));
    let (flag, kind) = resume(&agent)?;
    if text(session.get("kind")) != kind {
        return None;
    }
    let value = text(session.get("value"));
    if value.is_empty() {
        return None;
    }
    Some(vec![agent, flag.to_owned(), value])
}

/// Turns herdr's saved layout into what this app makes. Unknown fields are
/// ignored on purpose: herdr's file is versioned and grows.
pub fn parse(doc: &Value) -> Vec<Space> {
    let mut out = Vec::new();
    for workspace in doc
        .get("workspaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let identity = text(workspace.get("identity_cwd"));
        let mut tabs = Vec::new();
        for tab in workspace
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let panes: Vec<&Value> = tab
                .get("panes")
                .and_then(Value::as_object)
                .map(|p| p.values().collect())
                .unwrap_or_default();
            let Some(first) = panes.first() else { continue };
            let folder = text(first.get("cwd"));
            let mut made = Tab {
                name: text(tab.get("custom_name")),
                folder: if folder.is_empty() {
                    identity.clone()
                } else {
                    folder
                },
                argv: argv(first),
            };
            // Further panes in one tab are splits, which this import does not
            // make: a tab is one terminal here.
            if made.folder.is_empty() {
                made.folder = identity.clone();
            }
            tabs.push(made);
        }
        if tabs.is_empty() {
            continue;
        }
        let folder = if tabs[0].folder.is_empty() {
            identity
        } else {
            tabs[0].folder.clone()
        };
        out.push(Space {
            name: text(workspace.get("custom_name")),
            folder,
            tabs,
        });
    }
    out
}

/// Reads and parses a herdr layout file.
pub fn read(path: &str) -> Result<Vec<Space>, String> {
    let body =
        std::fs::read_to_string(path).map_err(|e| format!("no herdr layout at {path}: {e}"))?;
    let doc: Value = serde_json::from_str(&body).map_err(|e| format!("{path} is not JSON: {e}"))?;
    let spaces = parse(&doc);
    if spaces.is_empty() {
        return Err(format!("{path} has no workspaces to bring over"));
    }
    Ok(spaces)
}

#[cfg(test)]
mod tests {
    use super::{Space, argv, parse};
    use serde_json::json;

    /// The shape herdr writes: version 3, panes keyed by their number.
    fn saved() -> serde_json::Value {
        json!({
            "version": 3,
            "workspaces": [
                {
                    "id": "wK",
                    "custom_name": "Lets become rich!",
                    "identity_cwd": "/work/app",
                    "tabs": [
                        { "custom_name": "api", "panes": { "1": { "cwd": "/work/app", "agent_session": {
                            "agent": "pi", "kind": "path", "value": "/sessions/one.jsonl" } } } },
                        { "custom_name": "", "panes": { "2": { "cwd": "/work/app/sub" } } }
                    ]
                },
                { "id": "wT", "custom_name": "empty", "identity_cwd": "/work", "tabs": [] }
            ]
        })
    }

    #[test]
    fn a_workspace_becomes_a_space_with_its_tabs_in_order() {
        let spaces: Vec<Space> = parse(&saved());
        assert_eq!(spaces.len(), 1, "a workspace with no panes is not a space");
        assert_eq!(spaces[0].name, "Lets become rich!");
        assert_eq!(spaces[0].tabs.len(), 2);
        assert_eq!(spaces[0].tabs[0].name, "api");
        assert_eq!(spaces[0].tabs[1].name, "", "an unnamed tab stays unnamed");
        assert_eq!(
            spaces[0].tabs[1].folder, "/work/app/sub",
            "a tab keeps its own folder"
        );
    }

    #[test]
    fn an_agent_is_resumed_from_the_session_it_recorded() {
        let spaces = parse(&saved());
        assert_eq!(
            spaces[0].tabs[0].argv,
            Some(vec![
                "pi".into(),
                "--session".into(),
                "/sessions/one.jsonl".into()
            ])
        );
        assert_eq!(
            spaces[0].tabs[1].argv, None,
            "a tab without an agent runs a shell"
        );
    }

    #[test]
    fn an_agent_without_a_resume_flag_is_not_pretended_to() {
        let pane =
            json!({ "agent_session": { "agent": "some-agent", "kind": "path", "value": "/s" } });
        assert_eq!(argv(&pane), None);
    }

    #[test]
    fn a_session_kind_the_agent_cannot_take_is_not_pretended_to() {
        let pane = json!({ "agent_session": { "agent": "pi", "kind": "id", "value": "abc" } });
        assert_eq!(argv(&pane), None);
    }

    #[test]
    fn a_file_that_is_not_hers_reads_as_nothing() {
        assert!(parse(&json!({ "something": "else" })).is_empty());
    }
}

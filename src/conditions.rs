//! Derived panel visibility. Nothing in this module writes saved layout state.
use gpui::Task;
use gpui_shell::{HostValue, PreparedApplication};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub folder: Option<String>,
    pub machine: Option<String>,
}

pub fn watch(value: Option<HostValue>) -> Result<[bool; 4], String> {
    let Some(value) = value else {
        return Ok([true, false, false, false]);
    };
    let HostValue::Array(items) = value else {
        return Err("watch must be an array".into());
    };
    let mut watch = [false; 4];
    for item in items {
        let HostValue::Str(name) = item else {
            return Err("watch entries must be strings".into());
        };
        let index = match name.as_str() {
            "folder" => 0,
            "focus" => 1,
            "terminals" => 2,
            "agents" => 3,
            _ => return Err(format!("unknown watch entry {name:?}")),
        };
        watch[index] = true;
    }
    Ok(watch)
}

#[derive(Default)]
pub struct Overrides {
    context: Option<Context>,
    shown: std::collections::HashSet<String>,
}
impl Overrides {
    pub fn observe(&mut self, context: &Context) {
        if self
            .context
            .as_ref()
            .is_some_and(|previous| previous != context)
        {
            self.shown.clear();
        }
        self.context = Some(context.clone());
    }
    pub fn set(&mut self, slot: &str, show: bool) {
        if show {
            self.shown.insert(slot.to_owned());
        } else {
            self.shown.remove(slot);
        }
    }
    pub fn shown(&self, slot: &str) -> bool {
        self.shown.contains(slot)
    }
    pub fn clear(&mut self) {
        self.shown.clear();
    }
}

pub struct Panel {
    pub application: PreparedApplication,
    pub enabled: bool,
    pub state: State,
    pub task: Option<Task<()>>,
}

pub struct State {
    watch: [bool; 4],
    context: Option<Context>,
    observed: [u64; 5],
    revision: u64,
    pending: Option<u64>,
    completed: u64,
    eligible: bool,
    answer: Option<bool>,
    override_show: bool,
    pub error: Option<String>,
}

impl State {
    pub fn new(watch: [bool; 4]) -> Self {
        Self {
            watch,
            context: None,
            observed: [0; 5],
            revision: 1,
            pending: None,
            completed: 0,
            eligible: false,
            answer: None,
            override_show: false,
            error: None,
        }
    }

    pub fn visible(&self) -> bool {
        self.override_show || self.answer == Some(true)
    }

    pub fn manual(&mut self, show: bool) {
        self.override_show = show;
    }

    pub fn observe(&mut self, context: &Context, revisions: [u64; 5], eligible: bool) {
        let changed_context = self.context.as_ref() != Some(context);
        let changed = self
            .watch
            .iter()
            .enumerate()
            .any(|(i, watch)| *watch && self.observed[i] != revisions[i])
            || self.observed[4] != revisions[4];
        if changed_context {
            self.context = Some(context.clone());
            self.answer = None;
            self.error = None;
            self.override_show = false;
        }
        if changed_context || changed || (eligible && !self.eligible) {
            self.revision += 1;
        }
        self.eligible = eligible;
        self.observed = revisions;
    }

    pub fn begin(&mut self) -> Option<u64> {
        if !self.eligible || self.pending.is_some() || self.completed == self.revision {
            return None;
        }
        self.pending = Some(self.revision);
        Some(self.revision)
    }

    pub fn finish(&mut self, revision: u64, result: Result<HostValue, String>) -> bool {
        if self.pending != Some(revision) {
            return false;
        }
        self.pending = None;
        if revision != self.revision {
            return false;
        }
        self.completed = revision;
        match result {
            Ok(HostValue::Bool(answer)) => {
                self.answer = Some(answer);
                self.error = None;
            }
            Ok(_) => self.error = Some("when must return boolean or Promise<boolean>".into()),
            Err(error) => self.error = Some(error),
        }
        true
    }
}

/// Titles and report sequence numbers are intentionally not dependencies.
pub fn semantic(terminals: &Value) -> (Value, Value) {
    let mut terms = Vec::new();
    let mut agents = Vec::new();
    for terminal in terminals.as_array().into_iter().flatten() {
        terms.push(serde_json::json!([
            terminal["address"],
            terminal["state"],
            terminal["program"],
            terminal["foreground"]["argv"],
            terminal["argv"]
        ]));
        if !terminal["status"].is_null() {
            agents.push(serde_json::json!([terminal["address"], terminal["status"]]));
        }
    }
    terms.sort_by_key(Value::to_string);
    agents.sort_by_key(Value::to_string);
    (Value::Array(terms), Value::Array(agents))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn folder(name: &str) -> Context {
        Context {
            folder: Some(name.into()),
            machine: None,
        }
    }
    #[test]
    fn coalesces_and_discards_stale_replies() {
        let mut state = State::new([true, false, false, false]);
        state.observe(&folder("a"), [0; 5], true);
        let first = state.begin().unwrap();
        assert!(state.begin().is_none());
        state.observe(&folder("b"), [1, 0, 0, 0, 0], true);
        state.observe(&folder("c"), [2, 0, 0, 0, 0], true);
        assert!(!state.finish(first, Ok(HostValue::Bool(true))));
        assert!(!state.visible());
        let latest = state.begin().unwrap();
        assert!(state.finish(latest, Ok(HostValue::Bool(true))));
        assert!(state.visible());
        assert!(state.begin().is_none());
    }
    #[test]
    fn show_override_ends_only_on_context_change() {
        let mut state = State::new([true, true, true, true]);
        state.observe(&folder("a"), [0; 5], true);
        state.manual(true);
        let first = state.begin().unwrap();
        state.finish(first, Err("offline".into()));
        assert!(state.visible());
        state.observe(&folder("a"), [0, 1, 1, 1, 1], true);
        assert!(state.visible());
        state.manual(false);
        assert!(!state.visible());
        state.manual(true);
        state.observe(&folder("b"), [0, 1, 1, 1, 1], true);
        assert!(!state.visible());
    }
    #[test]
    fn eligibility_and_same_context_errors() {
        let mut state = State::new([true, false, false, false]);
        state.observe(&folder("a"), [0; 5], false);
        assert!(state.begin().is_none());
        state.observe(&folder("a"), [0; 5], true);
        let first = state.begin().unwrap();
        state.finish(first, Ok(HostValue::Bool(true)));
        state.observe(&folder("a"), [0, 0, 0, 0, 1], true);
        let next = state.begin().unwrap();
        state.finish(next, Ok(HostValue::Null));
        assert!(state.visible());
        assert!(state.error.is_some());
        state.observe(&folder("b"), [0, 0, 0, 0, 1], true);
        assert!(!state.visible());
    }
    #[test]
    fn show_before_copy_load_and_same_context_reload_keep_the_override() {
        let context = folder("project");
        let mut overrides = Overrides::default();
        overrides.observe(&context);
        overrides.set("panel", true);
        let mut first = State::new([true, false, false, false]);
        first.observe(&context, [0; 5], true);
        first.manual(overrides.shown("panel"));
        let call = first.begin().unwrap();
        first.finish(call, Ok(HostValue::Bool(false)));
        assert!(
            first.visible(),
            "Show must also work for a previously unloaded copy"
        );
        let mut reloaded = State::new([true, false, false, false]);
        reloaded.observe(&context, [0; 5], true);
        overrides.observe(&context);
        reloaded.manual(overrides.shown("panel"));
        assert!(
            reloaded.visible(),
            "hot reload must not end the Show override"
        );
        overrides.observe(&folder("other"));
        reloaded.observe(&folder("other"), [1, 0, 0, 0, 0], true);
        reloaded.manual(overrides.shown("panel"));
        assert!(!reloaded.visible());
        overrides.set("panel", true);
        overrides.set("panel", false);
        assert!(!overrides.shown("panel"));
    }

    #[test]
    fn default_watch_ignores_spinners_and_agent_changes() {
        let mut state = State::new(watch(None).unwrap());
        state.observe(&folder("a"), [0; 5], true);
        let first = state.begin().unwrap();
        state.finish(first, Ok(HostValue::Bool(false)));
        state.observe(&folder("a"), [0, 0, 0, 50, 0], true);
        assert!(state.begin().is_none());
        assert!(
            watch(Some(HostValue::Array(vec![HostValue::Str(
                "titles".into()
            )])))
            .is_err()
        );
    }
}

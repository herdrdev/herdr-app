//! Machine-owned layout documents and this screen's private view.
use crate::{
    layout::{Content, Kind, Node},
    model::{Agent, Dismissed, Model, Recent, Space, Tab},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub spaces: Vec<SharedSpace>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedSpace {
    id: String,
    name: String,
    folder: String,
    tabs: Vec<SharedTab>,
    /// Fields this build doesn't read, kept so a newer build's survive this one's write.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SharedTab {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default = "empty_root")]
    root: Node,
    /// Fields this build doesn't read, kept so a newer build's survive this one's write.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

fn empty_root() -> Node {
    Node::container("", crate::layout::Axis::Row, vec![])
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct View {
    active: Option<String>,
    spaces: BTreeMap<String, Option<String>>,
    tabs: BTreeMap<String, TabView>,
    recent: Vec<Recent>,
    attention: BTreeMap<String, Seen>,
    #[serde(default)]
    dismissed: BTreeMap<String, Dismissed>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct TabView {
    focused: Option<String>,
    before: Vec<String>,
    zoomed: Option<String>,
    settled: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Seen {
    word: Option<String>,
    seen: bool,
}

pub fn qualify(id: &str, machine: Option<&str>) -> String {
    machine.map_or_else(|| id.to_owned(), |m| format!("{m}/{id}"))
}

pub fn local<'a>(id: &'a str, machine: Option<&str>) -> &'a str {
    machine.and_then(|m| id.strip_prefix(m).and_then(|s| s.strip_prefix('/'))).unwrap_or(id)
}

fn map_node(node: &mut Node, map: &impl Fn(&str) -> String) {
    node.id = map(&node.id);
    match &mut node.kind {
        Kind::Block { content: Content::Terminal { terminal } } => *terminal = map(terminal),
        Kind::Stack { active: Some(active), .. } => *active = map(active),
        _ => {}
    }
    for float in &mut node.floating {
        float.id = map(&float.id);
        if let Content::Terminal { terminal } = &mut float.content {
            *terminal = map(terminal);
        }
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            map_node(child, map);
        }
    }
}

impl Document {
    pub fn capture(model: &Model, machine: Option<&str>) -> Self {
        let map = |s: &str| local(s, machine).to_owned();
        Self {
            spaces: model
                .spaces
                .iter()
                .filter(|s| s.machine.as_deref() == machine)
                .map(|s| SharedSpace {
                    id: map(&s.id),
                    name: s.name.clone(),
                    folder: s.folder.clone(),
                    tabs: s
                        .tabs
                        .iter()
                        .map(|t| {
                            let mut root = t.root.clone();
                            map_node(&mut root, &map);
                            SharedTab { id: map(&t.id), name: t.name.clone(), root, rest: t.rest.clone() }
                        })
                        .collect(),
                    rest: s.rest.clone(),
                })
                .collect(),
        }
    }

    /// Project visible edits while retaining hidden content from the supplied source.
    pub fn capture_with_base(model: &Model, machine: Option<&str>, base: Option<&Self>) -> Self {
        let mut doc = Self::capture(model, machine);
        let Some(base) = base.filter(|_| machine.is_some_and(|machine| model.dismissed.values().any(|d| d.machine == machine))) else { return doc };
        let qualified = base.project(machine);
        for (shared, space) in base.spaces.iter().zip(&qualified) {
            let hidden = model.dismissed.get(&space.id).is_some_and(|d| d.matches(&Dismissed::space(space)));
            if hidden {
                insert_hidden(&mut doc.spaces, &base.spaces, shared, |s| &s.id);
            } else {
                let hidden_tabs = shared.tabs.iter().zip(&space.tabs).filter(|(_, projected)| {
                    model.dismissed.get(&projected.id).is_some_and(|d| d.matches(&Dismissed::tab(machine.unwrap_or(""), projected)))
                }).map(|(tab, _)| tab).collect::<Vec<_>>();
                if let Some(visible) = doc.spaces.iter_mut().find(|s| s.id == shared.id) {
                    for tab in hidden_tabs { insert_hidden(&mut visible.tabs, &shared.tabs, tab, |t| &t.id); }
                } else if !hidden_tabs.is_empty() {
                    // Closing the last visible tab must not delete its hidden siblings.
                    let mut parent = shared.clone();
                    parent.tabs = hidden_tabs.into_iter().cloned().collect();
                    insert_hidden(&mut doc.spaces, &base.spaces, &parent, |s| &s.id);
                }
            }
        }
        doc
    }

    pub(crate) fn project(&self, machine: Option<&str>) -> Vec<Space> {
        let map = |s: &str| qualify(s, machine);
        self.spaces.iter().map(|s| Space {
            id: map(&s.id), name: s.name.clone(), folder: s.folder.clone(), machine: machine.map(str::to_owned), active: None,
            tabs: s.tabs.iter().map(|t| {
                let mut root = t.root.clone();
                map_node(&mut root, &map);
                Tab { id: map(&t.id), name: t.name.clone(), root, focused: None, before: vec![], zoomed: None, settled: None, rest: t.rest.clone() }
            }).collect(),
            rest: s.rest.clone(),
        }).collect()
    }

    pub fn replace(&self, model: &mut Model, machine: Option<&str>) {
        let view = View::capture(model);
        let at = model.spaces.iter().position(|s| s.machine.as_deref() == machine).unwrap_or(model.spaces.len());
        let mut spaces = self.project(machine);
        // Only an authoritative document for this machine can retire its exclusions.
        let mut identities = BTreeMap::new();
        if let Some(machine) = machine.filter(|machine| model.dismissed.values().any(|d| d.machine == *machine)) {
            for space in &spaces {
                identities.insert(space.id.clone(), Dismissed::space(space));
                for tab in &space.tabs { identities.insert(tab.id.clone(), Dismissed::tab(machine, tab)); }
            }
            model.dismissed.retain(|id, d| {
                if d.machine != machine { return true; }
                match identities.get(id) {
                    Some(other) if d.matches(other) => {
                        *d = other.clone(); // newly added terminals in hidden roots stay private too
                        true
                    }
                    Some(_) => false,
                    // A newborn labeled terminal may not have reached the document yet.
                    // Retain its identity, but capture never invents absent base nodes.
                    None => true,
                }
            });
        }
        spaces.retain(|s| !model.dismissed.get(&s.id).is_some_and(|d| identities.get(&s.id).is_some_and(|other| d.matches(other))));
        spaces.retain_mut(|space| {
            let had_tabs = !space.tabs.is_empty();
            space.tabs.retain(|t| !model.dismissed.get(&t.id).is_some_and(|d| identities.get(&t.id).is_some_and(|other| d.matches(other))));
            !had_tabs || !space.tabs.is_empty()
        });
        model.spaces.retain(|s| s.machine.as_deref() != machine);
        model.spaces.splice(at..at, spaces);
        model.order_spaces();
        view.apply(model);
    }

    /// Explicit ID selection is the only way to restore a dismissed view.
    pub fn restore(&self, model: &mut Model, machine: &str, id: &str, tab: bool) -> bool {
        // Compose before removing the exclusion: visible unsent edits win over
        // the fetched document, including while a write or drag is in progress.
        let current = Self::capture_with_base(model, Some(machine), Some(self));
        let spaces = current.project(Some(machine));
        let Some(space) = spaces.iter().find(|s| if tab { s.tabs.iter().any(|t| t.id == id) } else { s.id == id }) else { return false };
        let changed = model.dismissed.remove(&space.id).is_some();
        let changed = (tab && model.dismissed.remove(id).is_some()) || changed;
        if !changed { return false; }
        current.replace(model, Some(machine));
        true
    }
}

// Keep visible ordering and edits; hidden siblings retain their base order before
// the next surviving base sibling (or at the end).
fn insert_hidden<T: Clone>(visible: &mut Vec<T>, base: &[T], hidden: &T, id: impl Fn(&T) -> &String) {
    if visible.iter().any(|v| id(v) == id(hidden)) { return; }
    let at = base.iter().position(|v| id(v) == id(hidden)).unwrap();
    let before = base[at + 1..].iter().find_map(|next| visible.iter().position(|v| id(v) == id(next))).unwrap_or(visible.len());
    visible.insert(before, hidden.clone());
}

impl View {
    pub fn restore_dismissed(&self, model: &mut Model) { model.dismissed = self.dismissed.clone(); }

    pub fn capture(model: &Model) -> Self {
        Self {
            dismissed: model.dismissed.clone(),
            active: model.active.clone(),
            recent: model.recent.clone(),
            spaces: model.spaces.iter().map(|s| (s.id.clone(), s.active.clone())).collect(),
            tabs: model.tabs().map(|t| (t.id.clone(), TabView { focused: t.focused.clone(), before: t.before.clone(), zoomed: t.zoomed.clone(), settled: t.settled })).collect(),
            attention: model.agents.iter().map(|(a, s)| (a.clone(), Seen { word: s.word.clone(), seen: s.seen })).collect(),
        }
    }

    /// Restore saved choices when a machine's first document arrives, without
    /// resetting choices already made on other machines during startup.
    pub fn apply_machine(&self, model: &mut Model, machine: Option<&str>) {
        let mut merged = Self::capture(model);
        for space in model.spaces.iter().filter(|s| s.machine.as_deref() == machine) {
            if let Some(active) = self.spaces.get(&space.id) {
                merged.spaces.insert(space.id.clone(), active.clone());
            }
            for tab in &space.tabs {
                if let Some(view) = self.tabs.get(&tab.id) {
                    merged.tabs.insert(tab.id.clone(), view.clone());
                }
            }
            if self.active.as_deref() == Some(&space.id) {
                merged.active = self.active.clone();
            }
        }
        for (address, seen) in &self.attention {
            if address.split_once('/').map(|(m, _)| m) == machine {
                merged.attention.insert(address.clone(), seen.clone());
            }
        }
        if machine.is_none() {
            merged.recent = self.recent.clone();
        }
        merged.apply(model);
        for (address, seen) in &self.attention {
            if address.split_once('/').map(|(m, _)| m) == machine
                && let Some(agent) = model.agents.get_mut(address)
                && agent.word == seen.word
            {
                agent.seen = seen.seen;
            }
        }
    }

    pub fn preserve_unloaded(&mut self, saved: &Self, loaded: impl Fn(Option<&str>) -> bool) {
        let unloaded = |id: &str| !loaded(id.split_once('/').map(|(m, _)| m));
        self.spaces.extend(saved.spaces.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
        self.tabs.extend(saved.tabs.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
        self.attention.extend(saved.attention.iter().filter(|(id, _)| unloaded(id)).map(|(id, v)| (id.clone(), v.clone())));
    }

    pub fn apply(&self, model: &mut Model) {
        for s in &mut model.spaces {
            s.active = self.spaces.get(&s.id).cloned().flatten().filter(|id| s.tab(id).is_some()).or_else(|| s.tabs.first().map(|t| t.id.clone()));
            for t in &mut s.tabs {
                if let Some(v) = self.tabs.get(&t.id) {
                    t.focused = v.focused.clone().filter(|id| t.root.find(id).is_some());
                    t.zoomed = v.zoomed.clone().filter(|id| t.root.find(id).is_some());
                    t.before = v.before.iter().filter(|id| t.root.find(id).is_some()).cloned().collect();
                    t.settled = v.settled;
                }
                if t.focused.is_none() {
                    t.focused = t.root.panes().first().map(|n| n.id.clone());
                }
            }
        }
        model.active = self.active.clone().filter(|id| model.space(id).is_some()).or_else(|| model.spaces.first().map(|s| s.id.clone()));
        model.recent = self.recent.clone();
        for (a, s) in &self.attention {
            model.agents.entry(a.clone()).or_insert_with(|| Agent { word: s.word.clone(), seen: s.seen, ..Default::default() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Axis;
    fn model() -> Model {
        let mut m = Model::default();
        m.add_space("s", "work", "/work", Tab::new("t", "task", "p", "t1"));
        m
    }
    fn remote() -> (Model, Document) {
        let mut local = model();
        local.space_mut("s").unwrap().tabs.extend([
            Tab::new("t2", "second", "p2", "term2"), Tab::new("t3", "third", "p3", "term3"),
        ]);
        local.add_space("s2", "second space", "/two", Tab::new("t4", "fourth", "p4", "term4"));
        local.add_space("s3", "third space", "/three", Tab::new("t5", "fifth", "p5", "term5"));
        let doc = Document::capture(&local, None);
        let mut remote = Model::default();
        doc.replace(&mut remote, Some("mac"));
        (remote, doc)
    }

    #[test]
    fn dismissals_leave_shared_document_and_order_identical() {
        let (mut m, doc) = remote();
        assert!(m.dismiss_tab("mac/t"));
        assert!(m.dismiss_tab("mac/t2"));
        assert!(m.dismiss_space("mac/s2"));
        assert!(m.dismiss_space("mac/s3"));
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        assert_eq!(serde_json::to_vec(&Document::capture_with_base(&m, Some("mac"), Some(&doc))).unwrap(), serde_json::to_vec(&doc).unwrap());
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_none());
        assert!(m.space("mac/s2").is_none());
        m.tab_mut("mac/t3").unwrap().name = "edited here".into();
        let captured = Document::capture_with_base(&m, Some("mac"), Some(&doc));
        assert_eq!(captured.spaces[0].tabs[0], doc.spaces[0].tabs[0]);
        assert_eq!(captured.spaces[0].tabs[2].name, "edited here");
        assert_eq!(captured.spaces[1..], doc.spaces[1..]);
    }

    #[test]
    fn capture_preserves_visible_names_tabs_and_order_with_hidden_siblings() {
        let (mut m, doc) = remote();
        m.dismiss_tab("mac/t2");
        m.dismiss_space("mac/s2");
        m.tab_mut("mac/t").unwrap().name = "renamed tab".into();
        m.space_mut("mac/s").unwrap().name = "renamed space".into();
        m.space_mut("mac/s").unwrap().tabs.swap(0, 1);
        m.move_space("mac/s3", 0);
        let captured = Document::capture_with_base(&m, Some("mac"), Some(&doc));
        assert_eq!(captured.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s3", "s"]);
        let space = &captured.spaces[2];
        assert_eq!(space.name, "renamed space");
        assert_eq!(space.tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2", "t3", "t"]);
        assert_eq!(space.tabs[2].name, "renamed tab");
        assert_eq!(space.tabs[0], doc.spaces[0].tabs[1]);
    }

    #[test]
    fn closing_last_visible_tab_preserves_hidden_siblings_without_empty_space() {
        let (mut m, doc) = remote();
        m.dismiss_tab("mac/t");
        m.dismiss_tab("mac/t2");
        m.remove_tab("mac/t3");
        let captured = Document::capture_with_base(&m, Some("mac"), Some(&doc));
        assert_eq!(captured.spaces[0].tabs, doc.spaces[0].tabs[..2]);
        captured.replace(&mut m, Some("mac"));
        assert!(m.space("mac/s").is_none(), "a fully dismissed space has no empty shell");
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&captured)), captured);
        assert!(captured.restore(&mut m, "mac", "mac/t", true));
        assert!(m.tab("mac/t").is_some());
        assert!(m.tab("mac/t2").is_none());
    }

    #[test]
    fn hidden_content_always_comes_from_latest_base() {
        let (mut m, mut doc) = remote();
        m.dismiss_tab("mac/t");
        doc.spaces[0].tabs[0].name = "edited elsewhere".into();
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)).spaces[0].tabs[0].name, "edited elsewhere");
        doc.spaces[0].tabs.remove(0);
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        let old = View::capture(&m);
        doc.replace(&mut m, Some("mac"));
        old.apply_machine(&mut m, Some("mac"));
        assert!(m.dismissed.contains_key("mac/t"), "absent identities remain adoption tombstones");
    }

    #[test]
    fn reused_ids_do_not_hide_new_terminal_or_mod_content() {
        let (mut m, mut doc) = remote();
        m.dismiss_tab("mac/t");
        doc.spaces[0].tabs[0].root = Tab::new("t", "new", "new-pane", "new-terminal").root;
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_some());
        assert!(m.dismissed.is_empty());
        m.tab_mut("mac/t").unwrap().root = Node::block("mac/mod", Content::Mod { name: "board".into(), props: serde_json::json!({"id":1}) });
        let mut doc = Document::capture(&m, Some("mac"));
        m.dismiss_tab("mac/t");
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_none());
        doc.spaces[0].tabs[0].root = Node::block("new-mod", Content::Mod { name: "board".into(), props: serde_json::json!({"id":2}) });
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_some());
    }

    #[test]
    fn mod_identity_survives_props_resize_rename_and_siblings() {
        let (mut m, _) = remote();
        m.tab_mut("mac/t").unwrap().root = Node::block("mac/mod", Content::Mod { name: "board".into(), props: serde_json::json!({"value":1}) });
        let mut doc = Document::capture(&m, Some("mac"));
        m.dismiss_tab("mac/t");
        doc.spaces[0].tabs[0].name = "renamed".into();
        let mut leaf = Node::block("mod", Content::Mod { name: "board".into(), props: serde_json::json!({"value":2}) });
        leaf.size = Some(crate::layout::Size::Px { px: 250.0 });
        doc.spaces[0].tabs[0].root = Node::container("different-container", Axis::Row, vec![
            Node::block("sibling", Content::Mod { name: "other".into(), props: serde_json::json!({}) }), leaf,
        ]);
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_none());
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        m.add_space("mac/empty", "empty", "old", Tab::new("mac/temp", "", "mac/tmp", "mac/tmp-term"));
        m.space_mut("mac/empty").unwrap().machine = Some("mac".into());
        m.space_mut("mac/empty").unwrap().tabs.clear();
        let mut doc = Document::capture_with_base(&m, Some("mac"), Some(&doc));
        m.dismiss_space("mac/empty");
        let empty = doc.spaces.iter_mut().find(|s| s.id == "empty").unwrap();
        empty.name = "renamed empty".into();
        empty.folder = "new folder".into();
        doc.replace(&mut m, Some("mac"));
        assert!(m.space("mac/empty").is_none());
    }

    #[test]
    fn absent_newborn_identity_is_retained_without_inventing_shared_content() {
        let (mut m, doc) = remote();
        m.space_mut("mac/s").unwrap().tabs.push(Tab::new("mac/new", "newborn", "mac/np", "mac/nt"));
        m.dismiss_tab("mac/new");
        doc.replace(&mut m, Some("mac"));
        assert!(m.dismissed_terminal("mac/nt"));
        assert!(m.dismissed_target("mac/nt", "mac/s", Some("mac/new")));
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        assert!(!doc.restore(&mut m, "mac", "mac/new", true));
        assert!(m.dismissed.contains_key("mac/new"));
    }

    #[test]
    fn private_view_roundtrip_precedes_delayed_machine_load() {
        let (mut m, doc) = remote();
        m.dismiss_space("mac/s");
        let mut value = serde_json::to_value(View::capture(&m)).unwrap();
        let saved: View = serde_json::from_value(value.clone()).unwrap();
        value.as_object_mut().unwrap().remove("dismissed");
        assert!(serde_json::from_value::<View>(value).unwrap().dismissed.is_empty());
        let mut restarted = model();
        saved.restore_dismissed(&mut restarted);
        Document::capture(&model(), None).replace(&mut restarted, None);
        assert!(!restarted.dismissed.is_empty());
        doc.replace(&mut restarted, Some("mac"));
        saved.apply_machine(&mut restarted, Some("mac"));
        assert!(restarted.space("mac/s").is_none());
        assert!(restarted.space("s").is_some());
    }

    #[test]
    fn dismissing_last_mod_tab_also_preserves_previously_hidden_terminal_tabs() {
        let mut local = model();
        local.space_mut("s").unwrap().tabs.push(Tab::with_pane("board", "", Node::block("mod", Content::Mod { name: "board".into(), props: serde_json::json!({}) })));
        let doc = Document::capture(&local, None);
        let mut m = Model::default();
        doc.replace(&mut m, Some("mac"));
        m.dismiss_tab("mac/t");
        m.dismiss_tab("mac/board");
        assert!(m.spaces.is_empty());
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        doc.replace(&mut m, Some("mac"));
        assert!(m.spaces.is_empty());
    }

    #[test]
    fn authoritative_updates_refresh_hidden_terminal_identity() {
        let (mut m, mut doc) = remote();
        m.dismiss_tab("mac/t");
        doc.spaces[0].tabs[0].root = Node::container("t", Axis::Row, vec![
            Node::block("p", Content::Terminal { terminal: "t1".into() }),
            Node::block("new", Content::Terminal { terminal: "new-term".into() }),
        ]);
        doc.replace(&mut m, Some("mac"));
        assert!(m.dismissed_terminal("mac/new-term"));
        assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
    }

    #[test]
    fn explicit_restore_uses_latest_document_and_preserves_other_machines() {
        let (mut m, mut doc) = remote();
        m.dismiss_tab("mac/t");
        m.dismiss_space("mac/s2");
        Document::capture(&model(), None).replace(&mut m, None);
        m.select_tab("t");
        m.tab_mut("t").unwrap().zoomed = Some("p".into());
        doc.spaces[0].tabs[0].name = "latest".into();
        assert!(doc.restore(&mut m, "mac", "mac/t", true));
        assert_eq!(m.tab("mac/t").unwrap().name, "latest");
        assert!(m.space("mac/s2").is_none());
        assert_eq!(m.active.as_deref(), Some("s"));
        assert_eq!(m.tab("t").unwrap().zoomed.as_deref(), Some("p"));
        assert!(!doc.restore(&mut m, "mac", "mac/t", true));
    }

    #[test]
    fn restore_preserves_edits_newer_than_pending_snapshot() {
        let (mut m, doc) = remote();
        m.dismiss_tab("mac/t");
        m.tab_mut("mac/t2").unwrap().name = "pending rename".into();
        let pending = Document::capture_with_base(&m, Some("mac"), Some(&doc));
        m.tab_mut("mac/t2").unwrap().name = "newer edit".into();
        assert!(doc.restore(&mut m, "mac", "mac/t", true));
        assert_eq!(m.tab("mac/t2").unwrap().name, "newer edit");
        // Written acknowledges the prior snapshot without replacing the model.
        let next = Document::capture_with_base(&m, Some("mac"), Some(&pending));
        assert_eq!(next.spaces[0].tabs[1].name, "newer edit");
    }

    #[test]
    fn dismissed_terminal_moved_to_visible_view_is_not_suppressed() {
        let (mut m, _) = remote();
        m.dismiss_tab("mac/t");
        assert!(m.dismissed_terminal("mac/t1"));
        m.space_mut("mac/s2").unwrap().tabs.push(Tab::new("mac/moved", "", "mac/moved-pane", "mac/t1"));
        assert!(!m.dismissed_terminal("mac/t1"));
        assert!(!m.dismissed_target("mac/t1", "mac/s", Some("mac/t")));
    }

    #[test]
    fn mixed_tab_and_space_stay_dismissed_when_terminal_removed_elsewhere() {
        for whole_space in [false, true] {
            let (mut m, mut doc) = remote();
            if whole_space { doc.spaces[0].tabs.truncate(1); }
            doc.spaces[0].tabs[0].root = Node::container("mixed", Axis::Row, vec![
                Node::block("p", Content::Terminal { terminal: "t1".into() }),
                Node::block("board", Content::Mod { name: "board".into(), props: serde_json::json!({"value":1}) }),
            ]);
            // Keep a visible sibling so the tab case does not cascade to space.
            doc.replace(&mut m, Some("mac"));
            if whole_space { assert!(m.dismiss_space("mac/s")); }
            else { assert!(m.dismiss_tab("mac/t")); }
            let mut board = Node::block("board", Content::Mod { name: "board".into(), props: serde_json::json!({"value":2}) });
            board.size = Some(crate::layout::Size::Px { px: 250.0 });
            doc.spaces[0].name = "renamed".into();
            doc.spaces[0].tabs[0].root = board;
            doc.replace(&mut m, Some("mac"));
            assert!(m.tab("mac/t").is_none(), "surviving mod must remain dismissed (space={whole_space})");
            assert_eq!(Document::capture_with_base(&m, Some("mac"), Some(&doc)), doc);
        }
    }

    #[test]
    fn mixed_floating_mod_and_empty_root_keep_stable_dismissals() {
        for floating in [false, true] {
            let (mut m, mut doc) = remote();
            let root = &mut doc.spaces[0].tabs[0].root;
            if floating {
                root.floating.push(serde_json::from_value(serde_json::json!({
                    "id":"drawer", "content":{"mod":"board","props":{"value":1}}
                })).unwrap());
            } else {
                *root = Node::container("empty", Axis::Row, vec![]);
            }
            doc.replace(&mut m, Some("mac"));
            assert!(m.dismiss_tab("mac/t"));
            if floating {
                let float = doc.spaces[0].tabs[0].root.floating[0].clone();
                let mut root = Node::container("replacement-container", Axis::Column, vec![]);
                root.floating.push(float);
                root.floating[0].content = Content::Mod { name:"board".into(), props:serde_json::json!({"value":2}) };
                root.floating[0].peek = Some(60.0);
                doc.spaces[0].tabs[0].root = root;
            } else {
                doc.spaces[0].tabs[0].root.size = Some(crate::layout::Size::Px { px:250.0 });
            }
            doc.replace(&mut m, Some("mac"));
            assert!(m.tab("mac/t").is_none(), "floating={floating}");
        }
    }

    #[test]
    fn reused_terminal_pane_id_does_not_keep_old_dismissal() {
        let (mut m, mut doc) = remote();
        assert!(m.dismiss_tab("mac/t"));
        doc.spaces[0].tabs[0].root = Node::block("p", Content::Terminal { terminal: "new-terminal".into() });
        doc.replace(&mut m, Some("mac"));
        assert!(m.tab("mac/t").is_some());
        assert!(m.dismissed.is_empty());
    }

    #[test]
    fn focus_zoom_and_settle_are_private() {
        let mut m = model();
        let before = Document::capture(&m, None);
        m.tab_mut("t").unwrap().zoomed = Some("p".into());
        m.settle("t", 12);
        assert_eq!(before, Document::capture(&m, None));
        let view = View::capture(&m);
        let mut other = Model::default();
        before.replace(&mut other, None);
        assert_eq!(other.tab("t").unwrap().zoomed, None);
        view.apply(&mut other);
        assert_eq!(other.tab("t").unwrap().zoomed.as_deref(), Some("p"));
        assert_eq!(other.tab("t").unwrap().settled, Some(12));
    }
    #[test]
    fn remote_roundtrip_qualifies_only_identity_not_mod_props() {
        let mut m = model();
        m.split_with("p", Axis::Row, Node::block("mod", Content::Mod { name: "test".into(), props: serde_json::json!({"terminal":"t1"}) }));
        let doc = Document::capture(&m, None);
        doc.replace(&mut m, Some("mac"));
        assert_eq!(m.address_of("mac/p").as_deref(), Some("mac/t1"));
        assert_eq!(doc, Document::capture(&m, Some("mac")));
        assert!(m.space("s").is_some());
        assert!(m.space("mac/s").is_some());
    }
    #[test]
    fn replacement_preserves_valid_view_and_falls_back_when_deleted() {
        let mut m = model();
        m.split("p", Axis::Row, "p2", "t2");
        let doc = Document::capture(&m, None);
        m.focus_pane("p");
        doc.replace(&mut m, None);
        assert_eq!(m.focused_pane().as_deref(), Some("p"));
        let mut other = model();
        other.remove_pane("p");
        Document::capture(&other, None).replace(&mut m, None);
        assert!(m.active_space().is_none());
    }
    #[test]
    fn late_remote_load_restores_only_that_machines_view() {
        let mut m = model();
        Document::capture(&m, None).replace(&mut m, Some("mac"));
        m.tab_mut("mac/t").unwrap().zoomed = Some("mac/p".into());
        m.active = Some("mac/s".into());
        let saved = View::capture(&m);
        let doc = Document::capture(&m, Some("mac"));
        m.spaces.retain(|s| s.machine.is_none());
        m.active = Some("s".into());
        m.tab_mut("t").unwrap().settled = Some(42);
        doc.replace(&mut m, Some("mac"));
        saved.apply_machine(&mut m, Some("mac"));
        assert_eq!(m.tab("t").unwrap().settled, Some(42));
        assert_eq!(m.tab("mac/t").unwrap().zoomed.as_deref(), Some("mac/p"));
        assert_eq!(m.active.as_deref(), Some("mac/s"));
    }
    #[test]
    fn seen_attention_survives_a_screen_restart_but_not_a_new_word() {
        let mut m = model();
        m.set_word("t1", Some("done".into()), None, false);
        m.agents.get_mut("t1").unwrap().seen = false;
        let saved = View::capture(&m);
        m.agents.clear();
        saved.apply_machine(&mut m, None);
        assert!(!m.agents["t1"].seen);
        m.set_word("t1", Some("working".into()), None, false);
        saved.apply_machine(&mut m, None);
        assert!(m.agents["t1"].seen);
    }
    /// A newer build may add fields to a space or tab (#718's `floating`): this
    /// build writes them back as they were, so a mixed-version machine keeps them.
    #[test]
    fn unknown_space_and_tab_fields_survive_a_read_and_write() {
        let text = serde_json::json!({"spaces":[{"id":"s","name":"work","folder":"/w","color":"teal","tabs":[{"id":"t","name":"","root":
            {"id":"t","type":"row","children":[{"id":"p","type":"block","content":{"terminal":"t1"}}]},
            "floating":[{"pane":{"id":"q","type":"block","content":{"terminal":"t2"}},"rect":[0.2,0.2,0.6,0.6]}]}]}]});
        let read: Document = serde_json::from_value(text.clone()).unwrap();
        for machine in [None, Some("mac")] {
            let mut m = Model::default();
            read.replace(&mut m, machine);
            let id = |s: &str| qualify(s, machine);
            m.split(&id("p"), Axis::Row, &id("p2"), &id("t3"));
            let written = serde_json::to_value(Document::capture(&m, machine)).unwrap();
            assert_eq!(written["spaces"][0]["color"], "teal", "{machine:?}");
            assert_eq!(written["spaces"][0]["tabs"][0]["floating"], text["spaces"][0]["tabs"][0]["floating"], "{machine:?}");
        }
        // Read and written alone: the same document.
        let mut m = Model::default();
        read.replace(&mut m, None);
        assert_eq!(serde_json::to_value(Document::capture(&m, None)).unwrap(), text);
        assert!(!serde_json::to_string(&Document::capture(&model(), None)).unwrap().contains("rest"), "nothing extra: no field");
    }

    #[test]
    fn shared_fields_can_be_omitted() {
        let doc: Document = serde_json::from_value(serde_json::json!({"spaces":[{"id":"s","tabs":[{"id":"t"}]}]})).unwrap();
        assert_eq!(doc.spaces[0].name, "");
        assert_eq!(doc.spaces[0].folder, "");
        assert_eq!(doc.spaces[0].tabs[0].name, "");
        assert!(doc.spaces[0].tabs[0].root.children().is_empty());
        assert_eq!(serde_json::from_value::<Document>(serde_json::json!({})).unwrap(), Document::default());
    }
    #[test]
    fn machine_blocks_are_deterministic_and_moves_stay_inside_them() {
        let doc = Document::capture(&model(), None);
        let mut m = Model::default();
        doc.replace(&mut m, Some("z"));
        doc.replace(&mut m, Some("a"));
        doc.replace(&mut m, None);
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s", "a/s", "z/s"]);
        m.add_space("s2", "", "", Tab::new("t2", "", "p2", "t2"));
        m.order_spaces();
        assert!(m.move_space("s", usize::MAX));
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s", "a/s", "z/s"]);
        assert!(m.move_space("z/s", 1));
        assert_eq!(m.spaces.last().unwrap().id, "z/s");
        doc.replace(&mut m, Some("a"));
        assert_eq!(m.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s", "a/s", "z/s"]);
    }
    #[test]
    fn fresh_ids_are_not_a_per_screen_counter() {
        assert_ne!(Model::default().fresh_id("p"), Model::default().fresh_id("p"));
    }
}

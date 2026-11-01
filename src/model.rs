//! The app's shared world, the same for every layout: spaces, each with
//! tabs (named pieces of work), each tab holding its terminals and
//! how they're split. Plus which pane has focus and agent attention (done vs
//! seen) with Herdr's rollup. Pure data; the window drives it.

use crate::layout::{Axis, Content, Node};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// A named piece of work in a space: its terminals and how they're split.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: String,
    pub name: String,
    /// The arrangement: a row/column tree of terminal blocks. Its id is the tab's.
    pub root: Node,
    pub focused: Option<String>,
    /// Panes focused before, most recent last: closing the focused pane goes back.
    pub before: Vec<String>,
    pub zoomed: Option<String>,
    /// Put away (ms since the epoch): out of the lists, still running. Comes
    /// back when it's shown or one of its agents needs the person.
    pub settled: Option<u64>,
}

/// A place to work, plus the folder its new terminals start in.
#[derive(Clone, Debug)]
pub struct Space {
    pub id: String,
    /// The name someone gave it; empty: named after where its first
    /// terminal is (Herdr's way, `Model::space_name`).
    pub name: String,
    pub folder: String,
    /// The machine its terminals run on (`hc machine ls`); None: this one.
    pub machine: Option<String>,
    pub tabs: Vec<Tab>,
    /// The selected tab.
    pub active: Option<String>,
}

/// A closed space, so opening it again doesn't ask for its name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub name: String,
    pub folder: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alert {
    /// Needs the person (blocked or error).
    Blocked,
    /// Finished while the person wasn't looking.
    Done,
}

/// What core and the user's attention say about one terminal.
#[derive(Clone, Debug, Default)]
pub struct Agent {
    /// The program's 7501 word (working, idle, done, blocked, error), if it reports.
    pub word: Option<String>,
    /// The agent's name from its 7501 report (`app`), e.g. "claude-code".
    pub app: Option<String>,
    /// Whether the person has seen it since it last stopped working.
    pub seen: bool,
    /// Core's turn counter: goes up each time it stops working.
    pub turn: u64,
    /// The last turn a person looked at on any client (`herdr.seen`, docs/labels.md).
    /// None: no client shares it yet, so `seen` decides.
    pub seen_turn: Option<u64>,
    /// Bumped on every state change (for "most recent" ordering).
    pub seq: u64,
    /// When the state last changed, ms since the epoch (0: not since the app started).
    pub since: u64,
}

#[derive(Default)]
pub struct Model {
    pub spaces: Vec<Space>,
    /// The selected space.
    pub active: Option<String>,
    pub recent: Vec<Recent>,
    /// Each pane's live folder, for its display name.
    pub pane_folders: BTreeMap<String, String>,
    /// Each terminal's live title, for its tab's display name.
    pub titles: BTreeMap<String, String>,
    pub agents: HashMap<String, Agent>,
    seq: u64,
    /// Where each terminal works now: its repo (worktrees included) and branch.
    pub folder_info: HashMap<String, FolderInfo>,
}

/// Herdr's attention priority: blocked > done > working > idle > nothing.
pub fn priority(status: Option<&str>) -> u8 {
    match status {
        Some("blocked") => 4,
        Some("done") => 3,
        Some("working") => 2,
        Some("idle") => 1,
        _ => 0,
    }
}

impl Tab {
    /// A tab with one terminal pane.
    pub fn new(id: &str, name: &str, pane_id: &str, address: &str) -> Tab {
        Tab::with_pane(id, name, pane(pane_id, address))
    }

    /// A tab with one pane (a terminal or a mod).
    pub fn with_pane(id: &str, name: &str, pane: Node) -> Tab {
        Tab {
            id: id.to_owned(),
            name: name.to_owned(),
            focused: Some(pane.id.clone()),
            before: Vec::new(),
            root: Node::container(id, Axis::Row, vec![pane]),
            zoomed: None,
            settled: None,
        }
    }

    /// Focuses a pane and remembers the one it leaves.
    pub fn focus(&mut self, pane: &str) {
        if let Some(old) = self.focused.take().filter(|o| o != pane) {
            self.before.retain(|p| *p != old && p != pane);
            self.before.push(old);
            if self.before.len() > 32 {
                self.before.remove(0);
            }
        }
        self.focused = Some(pane.to_owned());
    }

    /// The focused pane, else the first one.
    pub fn focused_pane(&self) -> Option<String> {
        self.focused.clone().filter(|p| self.root.find(p).is_some()).or_else(|| self.root.panes().first().map(|n| n.id.clone()))
    }

    /// What's drawn: only the zoomed pane when one is zoomed.
    pub fn shown(&self) -> &Node {
        self.zoomed.as_deref().and_then(|z| self.root.find(z)).unwrap_or(&self.root)
    }
}

impl Space {
    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|c| c.id == id)
    }

    /// The selected tab, else the first.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.as_deref().and_then(|a| self.tab(a)).or(self.tabs.first())
    }

    fn select(&mut self, ctx: &str) {
        if self.tab(ctx).is_some() {
            self.active = Some(ctx.to_owned());
        }
    }
}

impl Model {
    pub fn fresh_id(&mut self, prefix: &str) -> String {
        use std::hash::{BuildHasher, Hasher};
        let random = || std::collections::hash_map::RandomState::new().build_hasher().finish();
        format!("{prefix}{:016x}{:016x}", random(), random())
    }

    pub fn fresh_id_on(&mut self, prefix: &str, machine: Option<&str>) -> String {
        crate::mirror::qualify(&self.fresh_id(prefix), machine)
    }

    /// Machine blocks have one order on every screen; order inside each stays shared.
    pub fn order_spaces(&mut self) {
        self.spaces.sort_by(|a, b| a.machine.cmp(&b.machine));
    }

    pub fn move_space(&mut self, id: &str, index: usize) -> bool {
        self.order_spaces();
        let Some(from) = self.spaces.iter().position(|s| s.id == id) else { return false };
        let machine = self.spaces[from].machine.clone();
        let first = self.spaces.iter().position(|s| s.machine == machine).unwrap();
        let last = self.spaces.iter().rposition(|s| s.machine == machine).unwrap();
        let to = index.saturating_sub(1).clamp(first, last);
        let space = self.spaces.remove(from);
        self.spaces.insert(to, space);
        true
    }

    pub fn active_index(&self) -> Option<usize> {
        let id = self.active.as_deref()?;
        self.spaces.iter().position(|p| p.id == id).or((!self.spaces.is_empty()).then_some(0))
    }

    pub fn active_space(&self) -> Option<&Space> {
        self.active_index().map(|i| &self.spaces[i])
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_space()?.active_tab()
    }

    pub fn space(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|p| p.id == id)
    }

    pub fn space_mut(&mut self, id: &str) -> Option<&mut Space> {
        self.spaces.iter_mut().find(|p| p.id == id)
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.spaces.iter().flat_map(|p| p.tabs.iter())
    }

    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs().find(|c| c.id == id)
    }

    pub fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.spaces.iter_mut().flat_map(|p| p.tabs.iter_mut()).find(|c| c.id == id)
    }

    /// The space holding a tab or a node inside one.
    pub fn space_of(&self, node: &str) -> Option<&Space> {
        self.spaces.iter().find(|p| p.tabs.iter().any(|c| c.id == node || c.root.find(node).is_some()))
    }

    /// The tab holding a node (pane, container).
    pub fn tab_of(&self, node: &str) -> Option<&Tab> {
        self.tabs().find(|c| c.root.find(node).is_some())
    }

    fn tab_of_mut(&mut self, node: &str) -> Option<&mut Tab> {
        self.spaces.iter_mut().flat_map(|p| p.tabs.iter_mut()).find(|c| c.root.find(node).is_some())
    }

    /// Adds a space at the end and selects it.
    /// What a space is called: its name, else its first terminal's checkout
    /// (a worktree's own folder; outside git its folder), as Herdr names workspaces.
    pub fn space_name(&self, p: &Space, home: &str) -> String {
        if !p.name.is_empty() {
            return p.name.clone();
        }
        let first = p.tabs.iter().flat_map(|c| c.root.terminals()).next();
        let folder = first
            .and_then(|n| n.terminal().and_then(|a| self.folder_info.get(a)).map(|f| f.top.clone()).or_else(|| self.pane_folders.get(&n.id).cloned()))
            .unwrap_or_else(|| p.folder.clone());
        folder_name(&folder, home)
    }

    /// What a tab is called: its name, else the title its first terminal
    /// reports (an agent session titles itself, `pi -n testname`), else its
    /// place in its space (1, 2, ...).
    pub fn tab_name(&self, p: &Space, c: &Tab) -> String {
        if !c.name.is_empty() {
            return c.name.clone();
        }
        let title = c.root.terminals().iter().find_map(|n| n.terminal().and_then(|a| self.titles.get(a)));
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            return title.clone();
        }
        (p.tabs.iter().position(|x| x.id == c.id).unwrap_or(0) + 1).to_string()
    }

    pub fn add_space(&mut self, id: &str, name: &str, folder: &str, ctx: Tab) {
        let active = Some(ctx.id.clone());
        self.spaces.push(Space { id: id.to_owned(), name: name.to_owned(), folder: folder.to_owned(), machine: None, tabs: vec![ctx], active });
        self.recent.retain(|r| r.name != name);
        self.active = Some(id.to_owned());
    }

    /// Adds a tab after the space's selected one and selects it (not the space).
    pub fn add_tab(&mut self, space: &str, ctx: Tab) -> bool {
        let Some(p) = self.space_mut(space) else { return false };
        let at = p.active.as_deref().and_then(|a| p.tabs.iter().position(|c| c.id == a)).map_or(p.tabs.len(), |i| i + 1);
        let id = ctx.id.clone();
        p.tabs.insert(at, ctx);
        p.active = Some(id);
        true
    }

    /// Splits `target` (a pane) with a new terminal pane and focuses it.
    pub fn split(&mut self, target: &str, axis: Axis, new_pane: &str, address: &str) -> bool {
        self.split_with(target, axis, pane(new_pane, address))
    }

    /// Splits a pane with any block: a terminal, or a mod (a viewer beside the shell).
    pub fn split_with(&mut self, target: &str, axis: Axis, block: Node) -> bool {
        let machine = self.space_of(target).and_then(|s| s.machine.clone());
        let container = self.fresh_id_on("s", machine.as_deref());
        let new_pane = block.id.clone();
        let Some(c) = self.tab_of_mut(target) else { return false };
        if !c.root.split(target, axis, block, container) {
            return false;
        }
        c.zoomed = None;
        c.focus(&new_pane);
        true
    }

    /// Rearranges existing panes in one tab without changing their terminal addresses.
    /// No edge swaps their places; an edge places the source beside the target.
    pub fn move_pane(&mut self, source: &str, target: &str, edge: Option<crate::layout::Edge>) -> bool {
        if source == target {
            return false;
        }
        let Some(tab) = self.tab_of(source) else { return false };
        if !tab.root.find(source).is_some_and(Node::is_pane) || !tab.root.find(target).is_some_and(Node::is_pane) {
            return false;
        }
        let mut root = tab.root.clone();
        if let Some(edge) = edge {
            use crate::layout::Edge;
            let mut pane = root.remove(source).unwrap();
            pane.size = None;
            let axis = if matches!(edge, Edge::Left | Edge::Right) { Axis::Row } else { Axis::Column };
            let machine = self.space_of(source).and_then(|s| s.machine.clone());
            let container = self.fresh_id_on("s", machine.as_deref());
            if !root.split(target, axis, pane, container) {
                return false;
            }
            if matches!(edge, Edge::Left | Edge::Top) {
                root.swap(source, target);
            }
        } else if !root.swap(source, target) {
            return false;
        }
        let tab = self.tab_of_mut(source).unwrap();
        tab.root = root;
        tab.focus(source);
        true
    }

    /// Removes a pane. Returns what else closed: the tab, and the space if it was its last.
    pub fn remove_pane(&mut self, pane_id: &str) -> (bool, bool) {
        let Some(c) = self.tab_of_mut(pane_id) else { return (false, false) };
        c.root.remove(pane_id);
        if c.zoomed.as_deref() == Some(pane_id) {
            c.zoomed = None;
        }
        if !c.root.panes().is_empty() {
            c.before.retain(|p| p != pane_id);
            if c.focused.as_deref() == Some(pane_id) {
                // Back to the pane focused before it that's still here, else the first.
                let here: Vec<String> = c.root.panes().iter().map(|n| n.id.clone()).collect();
                c.before.retain(|p| here.contains(p));
                c.focused = c.before.pop().or_else(|| here.first().cloned());
            }
            return (false, false);
        }
        let id = c.id.clone();
        let (_, space_closed) = self.remove_tab(&id);
        (true, space_closed)
    }

    /// Removes a tab. Returns its terminal addresses and whether its space closed.
    pub fn remove_tab(&mut self, ctx: &str) -> (Vec<String>, bool) {
        let Some(pi) = self.spaces.iter().position(|p| p.tab(ctx).is_some()) else { return (vec![], false) };
        let p = &mut self.spaces[pi];
        let ci = p.tabs.iter().position(|c| c.id == ctx).unwrap();
        let removed = p.tabs.remove(ci);
        if p.active.as_deref() == Some(ctx) {
            // The tab that slides into the slot, or the last one.
            p.active = p.tabs.get(ci).or(p.tabs.last()).map(|c| c.id.clone());
        }
        let closed = p.tabs.is_empty();
        if closed {
            let id = p.id.clone();
            self.remove_space(&id);
        }
        (addresses(&removed.root), closed)
    }

    /// Removes a space (remembered in `recent`). Returns its terminal addresses.
    pub fn remove_space(&mut self, id: &str) -> Vec<String> {
        let Some(pi) = self.spaces.iter().position(|p| p.id == id) else { return vec![] };
        let was_active = self.active_index() == Some(pi);
        let p = self.spaces.remove(pi);
        self.recent.retain(|r| r.name != p.name);
        self.recent.insert(0, Recent { name: p.name.clone(), folder: p.folder.clone() });
        self.recent.truncate(20);
        if was_active {
            self.active = self.spaces.get(pi).or(self.spaces.last()).map(|p| p.id.clone());
        }
        p.tabs.iter().flat_map(|c| addresses(&c.root)).collect()
    }

    /// The focused pane of the selected space's selected tab.
    pub fn focused_pane(&self) -> Option<String> {
        self.active_tab()?.focused_pane()
    }

    /// Selects a tab and its space. Shown again, a settled tab is back.
    pub fn select_tab(&mut self, ctx: &str) -> bool {
        let Some(p) = self.spaces.iter_mut().find(|p| p.tab(ctx).is_some()) else { return false };
        p.select(ctx);
        self.active = Some(p.id.clone());
        self.unsettle(ctx);
        true
    }

    /// Puts a tab away. The shown one hands over to the next unsettled
    /// tab of its space, if there is one.
    pub fn settle(&mut self, ctx: &str, now: u64) -> bool {
        let Some(c) = self.tab_mut(ctx) else { return false };
        c.settled = Some(now);
        let Some(p) = self.spaces.iter_mut().find(|p| p.tab(ctx).is_some()) else { return true };
        if p.active_tab().is_some_and(|c| c.id == ctx) {
            let i = p.tabs.iter().position(|c| c.id == ctx).unwrap_or(0);
            let n = p.tabs.len();
            if let Some(next) = (1..n).map(|k| &p.tabs[(i + k) % n]).find(|c| c.settled.is_none()) {
                p.active = Some(next.id.clone());
            }
        }
        true
    }

    pub fn unsettle(&mut self, ctx: &str) -> bool {
        self.tab_mut(ctx).is_some_and(|c| c.settled.take().is_some())
    }

    /// Focuses a pane anywhere: selects its space and tab.
    pub fn focus_pane(&mut self, pane_id: &str) -> bool {
        let Some(c) = self.tab_of_mut(pane_id) else { return false };
        c.focus(pane_id);
        let ctx = c.id.clone();
        self.select_tab(&ctx)
    }

    /// The address shown in a pane block.
    pub fn address_of(&self, pane_id: &str) -> Option<String> {
        self.tabs().find_map(|c| c.root.find(pane_id)).and_then(|n| n.terminal()).map(str::to_owned)
    }

    pub fn pane_of(&self, address: &str) -> Option<String> {
        self.tabs().flat_map(|c| c.root.terminals()).find(|n| n.terminal() == Some(address)).map(|n| n.id.clone())
    }

    /// Every terminal in every tab, in space → tab → pane order.
    pub fn all_addresses(&self) -> Vec<String> {
        self.tabs().flat_map(|c| addresses(&c.root)).collect()
    }

    /// Applies a status word from core (Herdr's seen rules).
    pub fn set_word(&mut self, address: &str, word: Option<String>, app: Option<String>, looking: bool) -> bool {
        let a = self.agents.entry(address.to_owned()).or_insert_with(|| Agent { seen: true, ..Default::default() });
        a.app = app.or(a.app.take());
        if a.word == word {
            return false;
        }
        let was_busy = matches!(a.word.as_deref(), Some("working" | "blocked"));
        match word.as_deref() {
            Some("working" | "blocked") => a.seen = true,
            // A finish you didn't watch is "done" until you look at it.
            Some(_) if was_busy => a.seen = looking,
            _ => {}
        }
        a.word = word;
        self.seq += 1;
        a.seq = self.seq;
        a.since = now_ms();
        true
    }

    /// Herdr's attention rule for a change of 7501 word: needing you always
    /// alerts; finishing alerts only if the person wasn't looking.
    pub fn alert(prev: Option<&str>, new: Option<&str>, looking: bool) -> Option<Alert> {
        let blocked = |w: Option<&str>| matches!(w, Some("blocked" | "error"));
        let busy = |w: Option<&str>| matches!(w, Some("working" | "blocked" | "error"));
        if blocked(new) && !blocked(prev) {
            Some(Alert::Blocked)
        } else if busy(prev) && new.is_some() && !busy(new) && !looking {
            Some(Alert::Done)
        } else {
            None
        }
    }

    /// `alert` for one terminal: only ours ring. A sub-agent reports to its
    /// parent; a terminal in none of the spaces (another machine's, another
    /// app's) isn't the person's business here.
    pub fn alert_for(&self, addr: &str, sub_agent: bool, prev: Option<&str>, new: Option<&str>, looking: bool) -> Option<Alert> {
        if sub_agent || self.pane_of(addr).is_none() {
            return None;
        }
        Self::alert(prev, new, looking)
    }

    /// Marks what the person is looking at as seen.
    pub fn mark_seen(&mut self, addresses: &[String]) -> bool {
        let mut changed = false;
        for addr in addresses {
            if let Some(a) = self.agents.get_mut(addr)
                && !a.seen
            {
                a.seen = true;
                changed = true;
            }
        }
        changed
    }

    /// Core's turn and the shared seen turn for an agent; true when either changed.
    pub fn set_turn(&mut self, address: &str, turn: u64, seen_turn: Option<u64>) -> bool {
        let Some(a) = self.agents.get_mut(address) else { return false };
        let changed = (a.turn, a.seen_turn) != (turn, seen_turn);
        (a.turn, a.seen_turn) = (turn, seen_turn);
        changed
    }

    /// What the UI shows: working, blocked, done (finished, unseen), idle, or None (not an agent).
    pub fn status(&self, address: &str) -> Option<&'static str> {
        let a = self.agents.get(address)?;
        Some(match a.word.as_deref()? {
            "working" => "working",
            "blocked" | "error" => "blocked",
            _ if a.seen_turn.map_or(!a.seen, |s| a.turn > s) => "done",
            _ => "idle",
        })
    }

    /// When something in a tab last changed state, ms (None: nothing since the app started).
    pub fn changed_at(&self, c: &Tab) -> Option<u64> {
        addresses(&c.root).iter().filter_map(|a| self.agents.get(a)).map(|a| a.since).filter(|t| *t > 0).max()
    }

    /// Since when an agent in a tab has been working, ms (the earliest, if several).
    pub fn working_since(&self, c: &Tab) -> Option<u64> {
        addresses(&c.root)
            .iter()
            .filter_map(|a| self.agents.get(a))
            .filter(|a| a.word.as_deref() == Some("working") && a.since > 0)
            .map(|a| a.since)
            .min()
    }

    /// The most urgent status among some terminals.
    pub fn rollup<'a>(&self, addresses: impl IntoIterator<Item = &'a String>) -> Option<&'static str> {
        addresses.into_iter().filter_map(|a| self.status(a)).max_by_key(|s| priority(Some(s)))
    }


}

pub fn pane(id: &str, address: &str) -> Node {
    Node::block(id, Content::Terminal { terminal: address.to_owned() })
}

/// A folder as a name: its last part, `~` for the home folder (`/` and `\` both).
pub fn folder_name(path: &str, home: &str) -> String {
    let full = path;
    let path = path.trim_end_matches(['/', '\\']);
    let home = home.trim_end_matches(['/', '\\']);
    let same = if cfg!(windows) { path.eq_ignore_ascii_case(home) } else { path == home };
    if full.is_empty() || same {
        return "~".into();
    }
    if path.is_empty() {
        return full[..1].to_owned();
    }
    path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(path).to_owned()
}

/// Terminal addresses in a subtree, in tree order.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn addresses(node: &Node) -> Vec<String> {
    node.terminals().iter().filter_map(|n| n.terminal().map(str::to_owned)).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderInfo {
    /// The checkout's top folder (a worktree's own), else the folder itself.
    pub top: String,
    pub branch: Option<String>,
}

/// A space as mods see it. `name` is what to call it; `named`: someone gave it
/// (else it's Herdr's automatic name, which a mod may show its own way).
pub fn space_json(m: &Model, p: &Space, index: usize, branch: Option<&str>, home: &str) -> Value {
    let active = p.active_tab().map(|c| c.id.clone());
    let tabs: Vec<Value> = p
        .tabs
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "id": c.id,
                "index": i + 1,
                "name": m.tab_name(p, c),
                "named": !c.name.is_empty(),
                "active": Some(&c.id) == active.as_ref(),
                "status": m.rollup(&addresses(&c.root)),
                "panes": c.root.panes().len(),
                "zoomed": c.zoomed.is_some(),
                "settled": c.settled.is_some(),
                "settled_at": c.settled,
                "changed_at": m.changed_at(c),
                "working_since": m.working_since(c),
            })
        })
        .collect();
    let all: Vec<String> = p.tabs.iter().flat_map(|c| addresses(&c.root)).collect();
    json!({
        "id": p.id,
        "index": index + 1,
        "name": m.space_name(p, home),
        "named": !p.name.is_empty(),
        "folder": p.folder,
        "machine": p.machine,
        "branch": branch,
        "active": m.active.as_deref() == Some(&p.id),
        "status": m.rollup(&all),
        "tabs": tabs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_name_as_in_herdr_on_every_system() {
        assert_eq!(folder_name("/home/can", "/home/can"), "~");
        assert_eq!(folder_name("/home/can/", "/home/can"), "~");
        assert_eq!(folder_name("/Users/can/Projects/herdr", "/Users/can"), "herdr");
        assert_eq!(folder_name("/", "/home/can"), "/");
        assert_eq!(folder_name("", "/home/can"), "~");
        assert_eq!(folder_name(r"C:\Users\can\src\api", r"C:\Users\can"), "api");
        assert_eq!(folder_name(r"\\?\C:\Users\can\src\api\", r"C:\Users\can"), "api");
        assert_eq!(folder_name("C:/Users/can/src/web", r"C:\Users\can"), "web");
    }

    #[test]
    fn unnamed_spaces_follow_their_first_terminal_and_tabs_count() {
        let mut m = Model::default();
        m.add_space("p1", "", "/home/me", Tab::new("c1", "", "x1", "t1"));
        m.add_tab("p1", Tab::new("c2", "review", "x2", "t2"));
        m.add_tab("p1", Tab::new("c3", "", "x3", "t3"));
        let p = m.space("p1").unwrap().clone();
        assert_eq!(m.space_name(&p, "/home/me"), "~");
        m.pane_folders.insert("x1".into(), "/src/notes".into());
        assert_eq!(m.space_name(&p, "/home/me"), "notes");
        m.folder_info.insert("t1".into(), FolderInfo { top: "/src/herdr".into(), branch: Some("main".into()) });
        assert_eq!(m.space_name(&p, "/home/me"), "herdr");
        let names: Vec<String> = p.tabs.iter().map(|c| m.tab_name(&p, c)).collect();
        assert_eq!(names.iter().map(String::as_str).collect::<Vec<_>>(), ["1", "review", "3"]);
        m.space_mut("p1").unwrap().name = "api".into();
        assert_eq!(m.space_name(m.space("p1").unwrap(), "/home/me"), "api");
    }

    #[test]
    fn an_unnamed_tab_is_called_by_its_terminals_title() {
        let mut m = Model::default();
        m.add_space("p1", "", "/home/me", Tab::new("c1", "", "x1", "t1"));
        m.add_tab("p1", Tab::new("c2", "review", "x2", "t2"));
        let p = m.space("p1").unwrap().clone();
        let tab = |m: &Model, id: &str| m.tab_name(&p, m.tab(id).unwrap());
        assert_eq!(tab(&m, "c1"), "1", "no title yet: its place");
        m.titles.insert("t1".into(), "π - testname - notes".into());
        assert_eq!(tab(&m, "c1"), "π - testname - notes");
        m.titles.insert("t1".into(), "π - foobar - notes".into());
        assert_eq!(tab(&m, "c1"), "π - foobar - notes", "it follows the title");
        m.titles.insert("t2".into(), "x".into());
        assert_eq!(tab(&m, "c2"), "review", "a tab someone named keeps its name");
        m.titles.insert("t1".into(), String::new());
        assert_eq!(tab(&m, "c1"), "1", "an empty title says nothing");
    }

    #[test]
    fn alerts_follow_herdr() {
        let alert = Model::alert;
        assert_eq!(alert(Some("working"), Some("blocked"), true), Some(Alert::Blocked));
        assert_eq!(alert(Some("blocked"), Some("error"), false), None);
        assert_eq!(alert(Some("working"), Some("idle"), false), Some(Alert::Done));
        assert_eq!(alert(Some("blocked"), Some("done"), false), Some(Alert::Done));
        assert_eq!(alert(Some("working"), Some("idle"), true), None);
        assert_eq!(alert(None, Some("idle"), false), None);
        assert_eq!(alert(Some("working"), None, false), None);
    }

    #[test]
    fn only_our_own_agents_alert() {
        let m = model_with_one();
        // In a space: it rings.
        assert_eq!(m.alert_for("t1", false, Some("working"), Some("done"), false), Some(Alert::Done));
        // Another machine's, or another app's, in none of the spaces: quiet.
        assert_eq!(m.alert_for("vps/t7", false, Some("working"), Some("done"), false), None);
        assert_eq!(m.alert_for("t9", false, Some("working"), Some("blocked"), false), None);
        // A sub-agent reports to its parent, not to the person.
        assert_eq!(m.alert_for("t1", true, Some("working"), Some("blocked"), false), None);
    }

    fn model_with_one() -> Model {
        let mut m = Model::default();
        m.add_space("p1", "api", "/src/api", Tab::new("c1", "fix login", "x1", "t1"));
        m
    }

    #[test]
    fn settling_the_shown_tab_hands_over_and_showing_it_brings_it_back() {
        let mut m = model_with_one();
        m.add_tab("p1", Tab::new("c2", "docs", "x2", "t2"));
        m.select_tab("c1");
        assert!(m.settle("c1", 5));
        assert_eq!(m.active_tab().map(|c| c.id.as_str()), Some("c2"));
        assert_eq!(m.tab("c1").unwrap().settled, Some(5));
        // The last unsettled one stays shown, settled.
        assert!(m.settle("c2", 6));
        assert_eq!(m.active_tab().map(|c| c.id.as_str()), Some("c2"));
        m.select_tab("c1");
        assert_eq!(m.tab("c1").unwrap().settled, None);
    }

    #[test]
    fn closing_the_last_pane_closes_tab_then_space_and_remembers_it() {
        let mut m = model_with_one();
        m.add_tab("p1", Tab::new("c2", "docs", "x2", "t2"));
        m.add_space("p2", "web", "/src/web", Tab::new("c3", "deploy", "x3", "t3"));
        assert_eq!(m.active.as_deref(), Some("p2"));
        assert_eq!(m.remove_pane("x3"), (true, true));
        assert_eq!(m.active.as_deref(), Some("p1"));
        assert_eq!(m.recent, [Recent { name: "web".into(), folder: "/src/web".into() }]);
        // A tab closes with its last pane; the space stays while it has another.
        assert_eq!(m.remove_pane("x2"), (true, false));
        assert_eq!(m.active_tab().map(|c| c.id.as_str()), Some("c1"));
        // Opening it again takes it off the recent list.
        m.add_space("p4", "web", "/src/web", Tab::new("c4", "x", "x4", "t4"));
        assert!(m.recent.is_empty());
    }

    #[test]
    fn a_mod_is_a_pane_beside_terminals() {
        let mut m = model_with_one();
        let viewer = Node::block("x2", Content::Mod { name: "markdown".into(), props: json!({ "file": "README.md" }) });
        assert!(m.split_with("x1", Axis::Row, viewer));
        let c = m.tab("c1").unwrap();
        assert_eq!(c.root.panes().len(), 2);
        assert_eq!(c.focused.as_deref(), Some("x2"));
        // It has no terminal: nothing to restart or kill, and the tab survives
        // its terminal pane closing.
        assert_eq!(m.address_of("x2"), None);
        assert_eq!(m.remove_pane("x1"), (false, false));
        assert_eq!(m.tab("c1").unwrap().root.panes().len(), 1);
    }

    #[test]
    fn pane_moves_preserve_terminals_and_focus_for_swap_and_every_edge() {
        use crate::layout::Edge;
        for edge in [None, Some(Edge::Left), Some(Edge::Right), Some(Edge::Top), Some(Edge::Bottom)] {
            let mut m = model_with_one();
            m.split("x1", Axis::Row, "x2", "t2");
            m.split("x2", Axis::Column, "x3", "t3");
            assert!(m.move_pane("x1", "x3", edge));
            assert_eq!(m.focused_pane().as_deref(), Some("x1"));
            for (pane, address) in [("x1", "t1"), ("x2", "t2"), ("x3", "t3")] {
                assert_eq!(m.address_of(pane).as_deref(), Some(address));
            }
            let root = &m.active_tab().unwrap().root;
            assert_eq!(root.panes().len(), 3);
            if let Some(edge) = edge {
                let parent = root.parent_of("x1").unwrap();
                let expected_axis = if matches!(edge, Edge::Left | Edge::Right) { Axis::Row } else { Axis::Column };
                assert_eq!(parent.axis(), Some(expected_axis));
                let ids: Vec<_> = parent.children().iter().map(|n| n.id.as_str()).collect();
                let source = ids.iter().position(|id| *id == "x1").unwrap();
                let target = ids.iter().position(|id| *id == "x3").unwrap();
                assert_eq!(source < target, matches!(edge, Edge::Left | Edge::Top));
                assert_eq!(source.abs_diff(target), 1);
            }
        }
    }

    #[test]
    fn cross_axis_pane_moves_split_equally_not_with_the_old_slot_size() {
        use crate::layout::{Edge, Size};
        for old_size in [Some(Size::Weight { weight: 0.5 }), Some(Size::Px { px: 200. })] {
            let mut m = model_with_one();
            m.split("x1", Axis::Row, "x2", "t2");
            m.tab_of_mut("x2").unwrap().root.find_mut("x2").unwrap().size = old_size;
            assert!(m.move_pane("x2", "x1", Some(Edge::Bottom)));
            let parent = m.active_tab().unwrap().root.parent_of("x2").unwrap();
            assert_eq!(parent.children()[0].size, None);
            assert_eq!(parent.children()[1].size, None);
        }
    }

    #[test]
    fn invalid_pane_moves_leave_the_model_alone() {
        let mut m = model_with_one();
        m.add_tab("p1", Tab::new("c2", "docs", "x2", "t2"));
        let before = format!("{:?}", m.spaces);
        for (source, target) in [("x1", "x1"), ("x1", "missing"), ("missing", "x1"), ("x1", "x2"), ("c1", "x1")] {
            assert!(!m.move_pane(source, target, None));
            assert_eq!(format!("{:?}", m.spaces), before);
        }
    }

    #[test]
    fn new_tabs_go_after_the_selected_one() {
        let mut m = model_with_one();
        m.add_tab("p1", Tab::new("c2", "b", "x2", "t2"));
        m.select_tab("c1");
        m.add_tab("p1", Tab::new("c3", "c", "x3", "t3"));
        let order: Vec<&str> = m.spaces[0].tabs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(order, ["c1", "c3", "c2"]);
        assert_eq!(m.active_tab().map(|c| c.id.as_str()), Some("c3"));
    }

    #[test]
    fn splits_focus_the_new_pane_and_closing_it_moves_focus() {
        let mut m = model_with_one();
        assert!(m.split("x1", Axis::Row, "x9", "t9"));
        assert_eq!(m.focused_pane().as_deref(), Some("x9"));
        assert_eq!(m.remove_pane("x9"), (false, false));
        assert_eq!(m.focused_pane().as_deref(), Some("x1"));
        // The arrangement keeps the tab's id with one pane left.
        assert_eq!(m.active_tab().unwrap().root.id, "c1");
    }

    #[test]
    fn closing_the_focused_pane_goes_back_to_the_one_before() {
        let mut m = model_with_one();
        m.split("x1", Axis::Row, "x2", "t2");
        m.split("x2", Axis::Row, "x3", "t3");
        // From x2 to x3; closing x3 lands on x2, not on the first pane.
        m.remove_pane("x3");
        assert_eq!(m.focused_pane().as_deref(), Some("x2"));
        // Clicked around: x1, then x2; close x2 and it's x1.
        m.split("x2", Axis::Row, "x4", "t4");
        m.focus_pane("x1");
        m.focus_pane("x4");
        m.remove_pane("x4");
        assert_eq!(m.focused_pane().as_deref(), Some("x1"));
        // The one before is gone too: the next one back that's still there.
        m.split("x2", Axis::Row, "x5", "t5");
        m.focus_pane("x2");
        m.remove_pane("x5");
        m.remove_pane("x2");
        assert_eq!(m.focused_pane().as_deref(), Some("x1"));
    }

    #[test]
    fn finishes_you_did_not_watch_are_done_until_seen() {
        let mut m = model_with_one();
        m.set_word("t1", Some("working".into()), Some("claude".into()), false);
        assert_eq!(m.status("t1"), Some("working"));
        m.set_word("t1", Some("idle".into()), None, false);
        assert_eq!(m.status("t1"), Some("done"));
        assert_eq!(m.rollup(&m.all_addresses()), Some("done"));
        m.mark_seen(&["t1".into()]);
        assert_eq!(m.status("t1"), Some("idle"));
        // Watching it finish: straight to idle.
        m.set_word("t1", Some("working".into()), None, true);
        m.set_word("t1", Some("done".into()), None, true);
        assert_eq!(m.status("t1"), Some("idle"));
        m.set_word("t1", Some("blocked".into()), None, true);
        assert_eq!(m.rollup(&m.all_addresses()), Some("blocked"));
    }

    #[test]
    fn a_shared_seen_turn_decides_over_this_screen() {
        let mut m = model_with_one();
        m.set_word("t1", Some("working".into()), None, true);
        m.set_word("t1", Some("idle".into()), None, true);
        assert_eq!(m.status("t1"), Some("idle"));
        // Another client hasn't seen turn 3 (marked unread): done, though we watched.
        assert!(m.set_turn("t1", 3, Some(2)));
        assert_eq!(m.status("t1"), Some("done"));
        assert!(!m.set_turn("t1", 3, Some(2)));
        // Seen elsewhere: idle, though we didn't watch.
        m.set_word("t1", Some("working".into()), None, false);
        m.set_word("t1", Some("idle".into()), None, false);
        m.set_turn("t1", 4, Some(4));
        assert_eq!(m.status("t1"), Some("idle"));
        // Needing you or working beats seen.
        m.set_word("t1", Some("blocked".into()), None, false);
        assert_eq!(m.status("t1"), Some("blocked"));
        // No label: this screen's own rule.
        m.set_word("t1", Some("idle".into()), None, false);
        m.set_turn("t1", 5, None);
        assert_eq!(m.status("t1"), Some("done"));
    }

    #[test]
    fn focus_follows_panes_across_tabs_and_spaces() {
        let mut m = model_with_one();
        m.add_tab("p1", Tab::new("c2", "b", "x2", "t2"));
        assert_eq!(m.focused_pane().as_deref(), Some("x2"));
        m.add_space("p2", "web", "/w", Tab::new("c3", "c", "x3", "t3"));
        assert!(m.focus_pane("x1"));
        assert_eq!(m.active.as_deref(), Some("p1"));
        assert_eq!(m.focused_pane().as_deref(), Some("x1"));
        assert_eq!(m.active_tab().map(|c| addresses(c.shown())).unwrap_or_default(), ["t1"]);
    }
}

//! Space views: tab blocks that show a given space (`{ "tab": "active", "space": id }`)
//! beside the one that shows the selected space, so two spaces' tabs are on screen at once.
//!
//! The selected space is the one the keyboard is in. A block without `space` shows it,
//! unless a pinned block already does: then it keeps showing the space it showed before,
//! so focusing a pinned view never changes what the main view shows.

use super::*;

impl AppWindow {
    /// The visible tab blocks, in layout order.
    pub(super) fn tab_blocks(&self) -> Vec<&Node> {
        self.chrome.visible_blocks().into_iter().filter(|n| matches!(n.content(), Some(Content::Context { .. }))).collect()
    }

    /// Spaces visible tab blocks are pinned to (those that exist).
    pub(super) fn pinned_spaces(&self) -> Vec<String> {
        self.tab_blocks()
            .into_iter()
            .filter_map(|n| match n.content() {
                Some(Content::Context { space: Some(s), .. }) if self.model.space(s).is_some() => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// The space a tab block without `space` shows: the selected one unless a pinned block
    /// shows it, else the one it showed last, else the first space no block is pinned to.
    pub(super) fn main_space(&self) -> Option<String> {
        let pinned = self.pinned_spaces();
        let free = |id: &String| !pinned.contains(id) && self.model.space(id).is_some();
        if let Some(active) = self.model.active_space().map(|s| s.id.clone()).filter(free) {
            *self.main_space.borrow_mut() = Some(active.clone());
            return Some(active);
        }
        let last = self.main_space.borrow().clone().filter(free);
        last.or_else(|| self.model.spaces.iter().map(|s| s.id.clone()).find(free))
    }

    fn block_space(&self, n: &Node, main: Option<&String>) -> Option<String> {
        match n.content() {
            Some(Content::Context { space: Some(s), .. }) => Some(s.clone()),
            Some(Content::Context { space: None, .. }) => main.cloned(),
            _ => None,
        }
    }

    /// Every tab on screen and the block it is drawn in. A space shows once: a second
    /// block for the same space stays empty.
    pub(super) fn shown_tabs(&self) -> Vec<(&Node, &Tab)> {
        let main = self.main_space();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for n in self.tab_blocks() {
            let Some(id) = self.block_space(n, main.as_ref()) else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(tab) = self.model.space(&id).and_then(|s| s.active_tab()) {
                out.push((n, tab));
            }
        }
        out
    }

    /// The tab a tab block draws, if any.
    pub(super) fn block_tab(&self, block: &str) -> Option<&Tab> {
        self.shown_tabs().into_iter().find(|(n, _)| n.id == block).map(|(_, t)| t)
    }

    /// The block a tab is drawn in, if it is on screen.
    pub(super) fn tab_block(&self, tab: &str) -> Option<&Node> {
        self.shown_tabs().into_iter().find(|(_, t)| t.id == tab).map(|(n, _)| n)
    }

    /// The block that draws `tab`, or would if its space showed it (a background tab).
    pub(super) fn tab_home(&self, tab: &str) -> Option<&Node> {
        self.tab_block(tab).or_else(|| {
            let space = self.model.spaces.iter().find(|s| s.tab(tab).is_some())?;
            self.shown_tabs().into_iter().find(|(_, t)| space.tab(&t.id).is_some()).map(|(n, _)| n)
        })
    }

    pub(super) fn tab_shown(&self, tab: &str) -> bool {
        self.tab_block(tab).is_some()
    }

    /// The block that shows the selected space (no `space`), else the first tab block.
    pub(super) fn main_block(&self) -> Option<&Node> {
        let blocks = self.tab_blocks();
        blocks.iter().copied().find(|n| matches!(n.content(), Some(Content::Context { space: None, .. }))).or(blocks.first().copied())
    }

    /// How `block` frames `tab`'s panes now: when it is split, or always with `lone`.
    pub(super) fn frame_of<'a>(&self, block: &'a Node, tab: &Tab) -> Option<&'a PaneFrame> {
        match block.content() {
            Some(Content::Context { pane: Some(f), .. }) if f.lone || tab.root.panes().len() > 1 => Some(f),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::layout::{Content, Node};

    #[test]
    fn a_tab_block_takes_an_optional_space() {
        let n: Node = serde_json::from_str(r#"{ "id": "side", "type": "block", "content": { "tab": "active", "space": "sp1" } }"#).unwrap();
        assert_eq!(n.content(), Some(&Content::Context { tab: "active".into(), space: Some("sp1".into()), pane: None }));
        let text = serde_json::to_string(&n).unwrap();
        assert!(text.contains(r#""space":"sp1""#));
        let main: Node = serde_json::from_str(r#"{ "id": "main", "type": "block", "content": { "tab": "active" } }"#).unwrap();
        assert!(!serde_json::to_string(&main).unwrap().contains("space"));
    }
}

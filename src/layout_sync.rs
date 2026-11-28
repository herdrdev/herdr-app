//! Layout clicks live apart from the layout file in layouts/.state/.

use crate::layout::{Align, Background, Content, Edge, Kind, Line, Node, PaneFrame, Size, ThemeChoice};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---- clicks ------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes: BTreeMap<String, NodeState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub floats: BTreeMap<String, FloatState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeChoice>,
    /// The layout file as it was when these clicks were saved: a field the
    /// file changed since (an edit while the app was closed, an update)
    /// drops its click on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// A stack's shown child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Background>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneClick>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneClick {
    pub header_height: Option<f32>,
    pub border: Option<Line>,
    pub focus: Option<Line>,
}

fn pane(n: &Node) -> Option<&PaneFrame> {
    match n.content() {
        Some(Content::Context { pane, .. }) => pane.as_ref(),
        _ => None,
    }
}

fn pane_mut(n: &mut Node) -> Option<&mut PaneFrame> {
    match &mut n.kind {
        Kind::Block { content: Content::Context { pane, .. } } => pane.as_mut(),
        _ => None,
    }
}

impl PaneClick {
    fn of(n: &Node) -> Option<Self> {
        let p = pane(n)?;
        Some(Self { header_height: p.header.as_ref().and_then(|h| h.height), border: p.border.clone(), focus: p.focus.clone() })
    }

    fn apply(&self, n: &mut Node) {
        if let Some(p) = pane_mut(n) {
            if let Some(h) = &mut p.header {
                h.height = self.header_height;
            }
            p.border = self.border.clone();
            p.focus = self.focus.clone();
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FloatState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

impl NodeState {
    fn is_empty(&self) -> bool {
        self.size.is_none() && self.hidden.is_none() && self.active.is_none() && self.background.is_none() && self.font_size.is_none() && self.pane.is_none()
    }
}

impl FloatState {
    fn is_empty(&self) -> bool {
        self.edge.is_none() && self.align.is_none() && self.hidden.is_none()
    }
}

fn active(n: &Node) -> Option<&String> {
    match &n.kind {
        crate::layout::Kind::Stack { active, .. } => active.as_ref(),
        _ => None,
    }
}

fn set_active(n: &mut Node, to: Option<String>) {
    if let crate::layout::Kind::Stack { active, .. } = &mut n.kind {
        *active = to;
    }
}

impl State {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.floats.is_empty() && self.theme.is_none()
    }

    /// Drops the clicks on fields `file` changed since they were saved.
    pub fn forget_since_saved(&mut self, file: &Node) {
        if let Some(then) = self.file.take().and_then(|v| serde_json::from_value::<Node>(v).ok()) {
            self.forget_edited(&then, file);
        }
    }

    /// The clicks: where `now` differs from `file` in the fields clicks change.
    pub fn between(file: &Node, now: &Node) -> State {
        let mut s = State::default();
        for n in now.walk() {
            let Some(f) = file.find(&n.id) else { continue };
            let mut ns = NodeState::default();
            if n.size != f.size {
                ns.size = n.size;
            }
            if n.hidden != f.hidden {
                ns.hidden = Some(n.hidden);
            }
            if active(n) != active(f) {
                ns.active = active(n).cloned();
            }
            if n.background != f.background {
                ns.background = n.background.clone();
            }
            if n.look.font_size != f.look.font_size {
                ns.font_size = n.look.font_size;
            }
            if PaneClick::of(n) != PaneClick::of(f) {
                ns.pane = PaneClick::of(n);
            }
            if !ns.is_empty() {
                s.nodes.insert(n.id.clone(), ns);
            }
            for fl in &n.floating {
                let Some(ff) = f.floating.iter().find(|x| x.id == fl.id) else { continue };
                let fs = FloatState {
                    edge: (fl.edge != ff.edge).then_some(fl.edge),
                    align: (fl.align != ff.align).then_some(fl.align),
                    hidden: (fl.hidden != ff.hidden).then_some(fl.hidden),
                };
                if !fs.is_empty() {
                    s.floats.insert(fl.id.clone(), fs);
                }
            }
        }
        if now.look.theme != file.look.theme {
            s.theme = now.look.theme.clone();
        }
        s
    }

    /// Puts the clicks onto a tree (ids it doesn't have are left alone).
    pub fn apply(&self, root: &mut Node) {
        for (id, ns) in &self.nodes {
            let Some(n) = root.find_mut(id) else { continue };
            if let Some(sz) = ns.size {
                n.size = Some(sz);
            }
            if let Some(h) = ns.hidden {
                n.hidden = h;
            }
            if let Some(background) = &ns.background {
                n.background = Some(background.clone());
            }
            if let Some(font_size) = ns.font_size {
                n.look.font_size = Some(font_size);
            }
            if let Some(pane) = &ns.pane {
                pane.apply(n);
            }
            if let Some(a) = &ns.active
                && n.children().iter().any(|c| &c.id == a)
            {
                set_active(n, Some(a.clone()));
            }
        }
        for (id, fs) in &self.floats {
            let Some(f) = root.find_float_mut(id) else { continue };
            if let Some(e) = fs.edge {
                f.edge = e;
            }
            if let Some(a) = fs.align {
                f.align = a;
            }
            if let Some(h) = fs.hidden {
                f.hidden = h;
            }
        }
        if let Some(t) = &self.theme {
            root.look.theme = Some(t.clone());
        }
    }

    /// The file changed from `old` to `new`: a field it now sets on purpose
    /// wins over the click on that field.
    pub fn forget_edited(&mut self, old: &Node, new: &Node) {
        for (id, ns) in self.nodes.iter_mut() {
            let (Some(o), Some(n)) = (old.find(id), new.find(id)) else { continue };
            if o.size != n.size {
                ns.size = None;
            }
            if o.hidden != n.hidden {
                ns.hidden = None;
            }
            if active(o) != active(n) {
                ns.active = None;
            }
            if o.background != n.background {
                ns.background = None;
            }
            if o.look.font_size != n.look.font_size {
                ns.font_size = None;
            }
            if PaneClick::of(o) != PaneClick::of(n) {
                ns.pane = None;
            }
        }
        self.nodes.retain(|_, ns| !ns.is_empty());
        let floats = |root: &Node| root.walk().into_iter().flat_map(|n| n.floating.clone()).collect::<Vec<_>>();
        let (of, nf) = (floats(old), floats(new));
        for (id, fs) in self.floats.iter_mut() {
            let (Some(o), Some(n)) = (of.iter().find(|f| &f.id == id), nf.iter().find(|f| &f.id == id)) else { continue };
            if o.edge != n.edge {
                fs.edge = None;
            }
            if o.align != n.align {
                fs.align = None;
            }
            if o.hidden != n.hidden {
                fs.hidden = None;
            }
        }
        self.floats.retain(|_, fs| !fs.is_empty());
        if old.look.theme != new.look.theme {
            self.theme = None;
        }
    }
}

/// `now` with the click fields put back as `file` has them: what's left
/// differs from `file` only by deliberate changes (a mod docked, `layout.set`).
pub fn without_clicks(file: &Node, now: &Node) -> Node {
    let mut out = now.clone();
    let ids: Vec<String> = out.walk().iter().map(|n| n.id.clone()).collect();
    for id in ids {
        let (Some(f), Some(n)) = (file.find(&id), out.find_mut(&id)) else { continue };
        n.size = f.size;
        n.hidden = f.hidden;
        n.background = f.background.clone();
        n.look.font_size = f.look.font_size;
        if let Some(pane) = PaneClick::of(f) {
            pane.apply(n);
        }
        let a = active(f).cloned();
        set_active(n, a);
        let ff = f.floating.clone();
        for fl in n.floating.iter_mut() {
            if let Some(o) = ff.iter().find(|x| x.id == fl.id) {
                fl.edge = o.edge;
                fl.align = o.align;
                fl.hidden = o.hidden;
            }
        }
    }
    out.look.theme = file.look.theme.clone();
    out
}

/// `set` (a tree handed back, often what `shown` was) with every click field
/// it left as `shown` has it put back as `file` has it: only what was changed
/// on purpose reaches the file; the clicks stay clicks.
pub fn keep_clicks_out(file: &Node, shown: &Node, set: &Node) -> Node {
    let mut out = set.clone();
    let ids: Vec<String> = out.walk().iter().map(|n| n.id.clone()).collect();
    for id in ids {
        let (Some(f), Some(s), Some(n)) = (file.find(&id), shown.find(&id), out.find_mut(&id)) else { continue };
        if n.size == s.size {
            n.size = f.size;
        }
        if n.hidden == s.hidden {
            n.hidden = f.hidden;
        }
        if n.background == s.background {
            n.background = f.background.clone();
        }
        if n.look.font_size == s.look.font_size {
            n.look.font_size = f.look.font_size;
        }
        if PaneClick::of(n) == PaneClick::of(s)
            && let Some(pane) = PaneClick::of(f)
        {
            pane.apply(n);
        }
        if active(n) == active(s) {
            set_active(n, active(f).cloned());
        }
        for fl in n.floating.iter_mut() {
            let (Some(o), Some(sh)) = (f.floating.iter().find(|x| x.id == fl.id), s.floating.iter().find(|x| x.id == fl.id)) else { continue };
            if fl.edge == sh.edge {
                fl.edge = o.edge;
            }
            if fl.align == sh.align {
                fl.align = o.align;
            }
            if fl.hidden == sh.hidden {
                fl.hidden = o.hidden;
            }
        }
    }
    if out.look.theme == shown.look.theme {
        out.look.theme = file.look.theme.clone();
    }
    out
}

// ---- updates -----------------------------------------------------------

/// Whole numbers written as `268.0` (the app's own saves) read as `268`, so
/// a number nobody changed never counts as a change.
pub fn normalize(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15)
                && n.as_i64().is_none()
            {
                *v = Value::from(f as i64);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize),
        Value::Object(m) => m.values_mut().for_each(normalize),
        _ => {}
    }
}

/// A layout as its file holds it: pretty, whole numbers without `.0`.
pub fn text_of(n: &Node) -> String {
    let mut v = serde_json::to_value(n).unwrap_or_default();
    normalize(&mut v);
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(j: Value) -> Node {
        serde_json::from_value(j).unwrap()
    }

    #[test]
    fn desktop_wallpaper_and_pixel_fit_are_clicks_not_structure_edits() {
        let file = node(json!({
            "id":"root", "type":"column", "font_size":16, "background":"#008080",
            "children":[{"id":"main", "type":"block", "content":{"tab":"active","pane":{
                "header":{"content":{"mod":"win98"},"height":18},
                "border":{"width":3,"color":"#c0c0c0"}, "focus":{"width":3,"color":"#c0c0c0"}
            }}}]
        }));
        let mut now = file.clone();
        now.background = Some(Background::Color("#123456".into()));
        now.look.font_size = Some(20.0);
        let p = pane_mut(now.find_mut("main").unwrap()).unwrap();
        p.header.as_mut().unwrap().height = Some(25.0);
        p.border.as_mut().unwrap().width = 4.0;
        p.focus.as_mut().unwrap().width = 4.0;
        assert_eq!(without_clicks(&file, &now), file);
        let state = State::between(&file, &now);
        let mut shown = file.clone();
        state.apply(&mut shown);
        assert_eq!(shown, now);
        shown.find_mut("main").unwrap().kind = Kind::Block { content: Content::Mod { name: "other".into(), props: Value::Null } };
        assert_ne!(without_clicks(&file, &shown), file);
    }

    #[test]
    fn a_tree_read_back_from_the_screen_keeps_its_clicks_out_of_the_file() {
        let file = node(json!({ "id": "root", "type": "row", "theme": "mytheme", "font_size": 16, "children": [
            { "id": "side", "type": "block", "size": { "px": 240 }, "content": { "mod": "spaces" } },
            { "id": "work", "type": "block", "content": { "tab": "active" } } ] }));
        // On screen: a dragged sidebar and the theme picker's choice.
        let mut shown = file.clone();
        shown.find_mut("side").unwrap().size = Some(Size::Px { px: 300.0 });
        shown.look.theme = Some(serde_json::from_value(json!({ "light": "paper", "dark": "graphite" })).unwrap());
        // A mod reads that back and changes only the font size.
        let mut set = shown.clone();
        set.look.font_size = Some(11.429);
        let out = keep_clicks_out(&file, &shown, &set);
        let mut want = file.clone();
        want.look.font_size = Some(11.429);
        assert_eq!(out, want, "only the font size reaches the file");
        // The clicks it kept apart still show.
        let mut s = State::between(&file, &shown);
        s.forget_edited(&shown, &set);
        let mut again = out.clone();
        s.apply(&mut again);
        assert_eq!(again, set);
        // Changing a click field on purpose writes it.
        let mut wider = shown.clone();
        wider.find_mut("side").unwrap().size = Some(Size::Px { px: 180.0 });
        assert_eq!(keep_clicks_out(&file, &shown, &wider).find("side").unwrap().size, Some(Size::Px { px: 180.0 }));
    }

    #[test]
    fn clicks_are_kept_apart_and_an_edit_wins() {
        let file = node(json!({ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "block", "size": { "px": 240 }, "content": { "mod": "spaces" } },
            { "id": "panel", "type": "stack", "hidden": true, "active": "a", "children": [
                { "id": "a", "type": "block", "content": { "mod": "files" } },
                { "id": "b", "type": "block", "content": { "mod": "changes" } } ] } ] }));
        let mut now = file.clone();
        now.find_mut("side").unwrap().size = Some(Size::Px { px: 300.0 });
        now.find_mut("panel").unwrap().hidden = false;
        assert!(now.select("panel", "b"));
        let s = State::between(&file, &now);
        assert_eq!(s.nodes.len(), 2);
        assert_eq!(without_clicks(&file, &now), file);
        let mut again = file.clone();
        s.apply(&mut again);
        assert_eq!(again, now);
        // The file now sets the sidebar's width itself: that wins.
        let mut edited = file.clone();
        edited.find_mut("side").unwrap().size = Some(Size::Px { px: 200.0 });
        let mut s2 = s.clone();
        s2.forget_edited(&file, &edited);
        assert!(s2.nodes.get("side").is_none());
        assert!(s2.nodes.get("panel").is_some());
    }

    #[test]
    fn an_edit_made_while_closed_beats_an_older_click() {
        let file = node(json!({ "id": "root", "type": "row", "children": [
            { "id": "side", "type": "block", "size": { "px": 240 }, "content": { "mod": "spaces" } },
            { "id": "main", "type": "block", "size": { "px": 500 }, "content": { "tab": "active" } } ] }));
        let mut now = file.clone();
        now.find_mut("side").unwrap().size = Some(Size::Px { px: 300.0 });
        now.find_mut("main").unwrap().size = Some(Size::Px { px: 600.0 });
        let mut s = State::between(&file, &now);
        s.file = serde_json::to_value(&file).ok();
        // Quit; someone sets the sidebar to 200 in the file; start.
        let mut edited = file.clone();
        edited.find_mut("side").unwrap().size = Some(Size::Px { px: 200.0 });
        s.forget_since_saved(&edited);
        let mut shown = edited.clone();
        s.apply(&mut shown);
        assert_eq!(shown.find("side").unwrap().size, Some(Size::Px { px: 200.0 }));
        assert_eq!(shown.find("main").unwrap().size, Some(Size::Px { px: 600.0 }));
    }

    #[test]
    fn shipped_layouts_name_every_node() {
        fn missing(v: &Value, at: &str, out: &mut Vec<String>) {
            if v.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
                out.push(at.to_owned());
            }
            for (k, list) in [("children", v.get("children")), ("floating", v.get("floating"))] {
                for (i, c) in list.and_then(Value::as_array).into_iter().flatten().enumerate() {
                    missing(c, &format!("{at}/{k}[{i}]"), out);
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/layouts");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut out = vec![];
            missing(&v, "root", &mut out);
            assert!(out.is_empty(), "{}: nodes without an id: {out:?}", path.display());
        }
    }

}

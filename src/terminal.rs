//! A pane showing one core terminal: bytes in from the core, frames drawn
//! with GPUI, keys and mouse encoded by libghostty-vt and sent back.

use crate::hc::{Core, Stream};
use crate::vt::{self, CursorShape, Rgb, Select, Vt};
use ghostty_sys::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use crate::links;
use gpui_base::input::{InputEvent, InputState};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

fn pad() -> (f32, f32) {
    let p = crate::settings::get().terminal_padding;
    let zoom = crate::settings::zoom();
    (p[0] * zoom, p[1] * zoom)
}

pub fn hsla(c: Rgb) -> Hsla {
    Rgba { r: c.0 as f32 / 255.0, g: c.1 as f32 / 255.0, b: c.2 as f32 / 255.0, a: 1.0 }.into()
}

#[derive(Clone, Debug, PartialEq)]
enum LinkTarget {
    External(links::Target),
    Terminal(String),
}

pub enum TerminalEvent {
    Peek(String),
    TitleChanged,
    /// The program ended.
    Ended,
    /// Right-click where the program doesn't take the mouse: the pane menu.
    ContextMenu(Point<Pixels>),
}

/// What the canvas closures need, shared outside the entity so prepaint can resize.
struct Grid {
    vt: Vt,
    core: Arc<Core>,
    channel: Option<u32>,
    cell: Size<Pixels>,
    /// The window's scale factor, from the last prepaint. Programs see cells,
    /// images and pixel mouse reports in device pixels, as in Ghostty.
    scale: f32,
    /// The cell in device pixels the local screen was sized with.
    vt_cell: (u32, u32),
    /// The colors and device-pixel cell size the core was last told.
    told_display: Option<DisplayFacts>,
    /// Top-left of the cell grid in window coordinates, from the last paint.
    origin: Point<Pixels>,
    buttons_down: Vec<MouseButton>,
    /// Kitty images uploaded for drawing, by (image, generation, source rect).
    images: HashMap<(u32, u64, [u32; 4]), Arc<RenderImage>>,
    /// Where the pointer is over the pane.
    hover: Option<Point<Pixels>>,
    /// The link under the pointer to underline: (row, first column, last column).
    link_spans: vt::Spans,
    /// Text an input method is still composing ("nihao" before "\u{4f60}\u{597d}"),
    /// drawn at the cursor; the program sees only the finished text.
    marked: Option<String>,
    /// The cursor's cell in window coordinates, from the last paint: where the
    /// input method puts its candidate window.
    cursor_at: Option<Bounds<Pixels>>,
    /// Each row's drawing by position, kept while the row is the same (`Rc`).
    painted: Vec<(Rc<Vec<vt::Cell>>, Vec<RowOp>)>,
    /// The font size, cell size, font and fonts loaded the drawings were built with.
    painted_with: (u32, Size<Pixels>, String, u64),
    /// What the find bar's count shows, as of the last paint.
    find_status: vt::FindStatus,
}

pub struct TerminalView {
    pub address: String,
    pub title: String,
    pub ended: Option<String>,
    /// The core connection dropped; waiting to reattach.
    pub lost: bool,
    /// A left-button selection drag is in progress.
    selecting: bool,
    selection_pointer: Point<Pixels>,
    selection_scroll: Option<Task<()>>,
    /// The terminal's folder, for links to relative paths.
    pub cwd: Option<PathBuf>,
    /// The link under the pointer while the link modifier is held.
    link: Option<LinkTarget>,
    /// Right-clicks go to the program when it takes the mouse (Shift: the menu).
    pub right_click_to_program: bool,
    grid: Rc<RefCell<Grid>>,
    focus: FocusHandle,
    was_focused: bool,
    /// The window frame this terminal was last put on screen in (`FRAME`).
    shown_in: Cell<u64>,
    /// When the person last typed here: output right after is shown at once.
    typed_at: Cell<Option<Instant>>,
    /// Scrolled this gesture, in lines, toward the next whole one.
    scrolled: Point<f32>,
    /// The find bar's text field, while the bar is open.
    find: Option<Entity<InputState>>,
    /// The find bar had the keys last (not the terminal): focusing the pane
    /// again (a dialog closed, back from another tab) gives them back to it.
    find_keys: bool,
    _find_events: Option<Subscription>,
    _reader: Option<Task<()>>,
}

thread_local! {
    /// Terminals with output waiting for the next tick of the output clock.
    static OUTPUT_CLOCK: RefCell<Vec<WeakEntity<TerminalView>>> = const { RefCell::new(Vec::new()) };
}

/// The window's frame number, counted where the window renders.
static FRAME: AtomicU64 = AtomicU64::new(0);

pub fn next_frame() {
    FRAME.fetch_add(1, Ordering::Relaxed);
}

impl EventEmitter<TerminalEvent> for TerminalView {}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DisplayFacts {
    fg: Rgb,
    bg: Rgb,
    cursor: Rgb,
    /// In device pixels, what the core reports to programs (CSI 14t, 16t).
    cell: (u32, u32),
}

/// Only send changed facts; a theme switch need not change the cell size.
fn display_update(told: &mut Option<DisplayFacts>, theme: &vt::Theme, cell: (u32, u32)) -> Option<serde_json::Value> {
    let facts = DisplayFacts { fg: theme.fg, bg: theme.bg, cursor: theme.cursor, cell };
    if *told == Some(facts) {
        return None;
    }
    *told = Some(facts);
    let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    Some(json!({"fg": hex(facts.fg), "bg": hex(facts.bg), "cursor": hex(facts.cursor), "cell": [cell.0, cell.1]}))
}

impl Grid {
    fn refresh_theme(&mut self, theme: &vt::Theme, cell: (u32, u32)) {
        self.vt.set_theme(theme.clone());
        if let Some(chan) = self.channel
            && let Some(facts) = display_update(&mut self.told_display, theme, cell)
        {
            self.core.facts(chan, &facts);
        }
    }
}

/// A cell in device pixels.
fn device_cell(cell: Size<Pixels>, scale: f32) -> (u32, u32) {
    let px = |v: Pixels| (f32::from(v) * scale).round().max(1.0) as u32;
    (px(cell.width), px(cell.height))
}

// Held-button events belong to the pane that received the press.
fn track_mouse_button(buttons: &mut Vec<MouseButton>, action: GhosttyMouseAction, button: Option<MouseButton>) -> bool {
    if let Some(button) = button {
        if action == GHOSTTY_MOUSE_ACTION_PRESS {
            if !buttons.contains(&button) {
                buttons.push(button);
            }
        } else if !buttons.contains(&button) {
            return false;
        } else if action == GHOSTTY_MOUSE_ACTION_RELEASE {
            buttons.retain(|b| *b != button);
        }
    }
    true
}

impl TerminalView {
    /// Attaches at `size` (cols, rows): the terminal's current size, so attaching
    /// a hidden terminal doesn't resize its program.
    pub fn attach(core: Arc<Core>, address: String, size_hint: (u16, u16), cx: &mut Context<Self>) -> Self {
        let mut view = TerminalView {
            address: address.clone(),
            title: address.clone(),
            ended: None,
            lost: false,
            selecting: false,
            selection_pointer: Point::default(),
            selection_scroll: None,
            cwd: None,
            link: None,
            right_click_to_program: false,
            grid: Rc::new(RefCell::new(Grid {
                vt: Vt::new(size_hint.0.max(2), size_hint.1.max(1)),
                core: core.clone(),
                channel: None,
                cell: size(px(9.0), px(18.0)),
                scale: 1.0,
                vt_cell: (9, 18),
                told_display: None,
                origin: Point::default(),
                buttons_down: Vec::new(),
                images: HashMap::new(),
                hover: None,
                link_spans: Vec::new(),
                marked: None,
                cursor_at: None,
                painted: Vec::new(),
                painted_with: (0, Size::default(), String::new(), 0),
                find_status: vt::FindStatus::default(),
            })),
            focus: cx.focus_handle(),
            was_focused: false,
            shown_in: Cell::new(0),
            typed_at: Cell::new(None),
            scrolled: point(0.0, 0.0),
            find: None,
            find_keys: false,
            _find_events: None,
            _reader: None,
        };
        view.connect(core, false, cx);
        view
    }

    /// Attaches to the same terminal on a (new) core connection.
    pub fn reattach(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if matches!(self.ended.as_deref(), Some("exited" | "killed" | "finished")) {
            return;
        }
        self.ended = None;
        self.lost = false;
        self.connect(core, true, cx);
        cx.notify();
    }

    /// A kept terminal can restart without its core connection changing.
    pub fn reattach_if_needed(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if self.lost || self.ended.is_some() { self.reattach(core, cx); }
    }

    /// Attaches off the UI thread: a terminal on a machine still dialing can
    /// take seconds, and the window must keep drawing meanwhile.
    /// `announce_end`: emit Ended if the terminal turns out to be gone.
    fn connect(&mut self, core: Arc<Core>, announce_end: bool, cx: &mut Context<Self>) {
        let needle = self.find.as_ref().map(|f| f.read(cx).value().to_string());
        let (cols, rows, facts) = {
            let mut g = self.grid.borrow_mut();
            let (cols, rows) = (g.vt.cols, g.vt.rows);
            // The core repaints the whole screen (and history) on attach.
            let cell = device_cell(g.cell, g.scale);
            g.vt = Vt::new(cols, rows);
            // An open find bar goes on finding in what the core sends.
            if let Some(needle) = &needle {
                g.vt.find(needle, false);
            }
            g.vt.resize(cols, rows, cell.0, cell.1);
            g.vt_cell = cell;
            g.told_display = None;
            let facts = display_update(&mut g.told_display, &vt::theme(), cell).unwrap();
            g.core = core.clone();
            g.channel = None;
            (cols, rows, facts)
        };
        let address = self.address.clone();
        // Replacing the task drops an attach still in flight for an older connection.
        self._reader = Some(cx.spawn(async move |this, cx| {
            let attached = cx.background_executor().spawn(async move { core.attach(&address, cols, rows, facts) }).await;
            let Ok(Some(rx)) = this.update(cx, |this, cx| this.attached(attached, (cols, rows), announce_end, cx)) else {
                return;
            };
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| this.ingest(batch, cx));
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn attached(
        &mut self,
        attached: anyhow::Result<crate::hc::Attached>,
        attached_size: (u16, u16),
        announce_end: bool,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<Stream>> {
        cx.notify();
        match attached {
            Ok(attached) => {
                self.lost = false;
                let mut g = self.grid.borrow_mut();
                g.channel = Some(attached.channel);
                // The theme or cell may have changed while the attach was in flight.
                let cell = device_cell(g.cell, g.scale);
                g.refresh_theme(&vt::theme(), cell);
                // The pane may have been laid out while the attach was in flight.
                let now = (g.vt.cols, g.vt.rows);
                if now != attached_size {
                    g.core.resize(attached.channel, now.0, now.1);
                }
                drop(g);
                if let Some(t) = attached.terminal.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                    self.set_title(t, cx);
                }
                Some(attached.stream)
            }
            // Gone for good (removed, finished, unknown) versus a connection problem.
            Err(e) if crate::hc::error_kind(&e) == Some("unavailable") || format!("{e:#}").contains("lost the core") || format!("{e:#}").contains("no reply") => {
                self.lost = true;
                None
            }
            // It ended between being listed and being attached: its pane goes,
            // like one whose program exits while shown.
            Err(e) if crate::hc::error_kind(&e) == Some("finished") => {
                self.ended = Some("finished".into());
                cx.emit(TerminalEvent::Ended);
                None
            }
            Err(e) => {
                self.ended = Some(format!("{e:#}"));
                if announce_end {
                    cx.emit(TerminalEvent::Ended);
                }
                None
            }
        }
    }

    /// Attached tabs follow the window's theme even when they are not drawn.
    pub fn refresh_theme(&self) {
        let mut g = self.grid.borrow_mut();
        let cell = device_cell(g.cell, g.scale);
        g.refresh_theme(&vt::theme(), cell);
    }

    /// Its title as shown (see `shown_title`); news only when that changed.
    fn set_title(&mut self, title: &str, cx: &mut Context<Self>) {
        let shown = shown_title(title);
        if shown != self.title {
            self.title = shown.to_owned();
            cx.emit(TerminalEvent::TitleChanged);
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn detach(&self) {
        let g = self.grid.borrow();
        if let Some(ch) = g.channel {
            g.core.detach(ch);
        }
    }

    /// Feeds relayed bytes in; returns false once the stream is over.
    fn ingest(&mut self, batch: Vec<Stream>, cx: &mut Context<Self>) -> bool {
        let mut open = true;
        let title = {
            let mut g = self.grid.borrow_mut();
            for item in batch {
                match item {
                    Stream::Data(bytes) => {
                        g.vt.allow_clipboard_write(crate::settings::get().clipboard_write);
                        g.vt.write(&bytes);
                        let [clipboard, primary] = g.vt.take_clipboard();
                        if let Some(text) = clipboard {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                        #[cfg(target_os = "linux")]
                        if let Some(text) = primary {
                            cx.write_to_primary(ClipboardItem::new_string(text));
                        }
                        #[cfg(not(target_os = "linux"))]
                        let _ = primary;
                        if let Some(ch) = g.channel {
                            g.core.credit(ch, bytes.len());
                            // What the core leaves to viewers (e.g. kitty graphics replies).
                            let replies = g.vt.take_replies();
                            g.core.input(ch, &replies);
                        }
                    }
                    Stream::Closed(why) => {
                        open = false;
                        g.channel = None;
                        self.ended = Some(why.get("reason").and_then(|r| r.as_str()).unwrap_or("ended").to_owned());
                    }
                    Stream::Lost => {
                        g.channel = None;
                        self.lost = true;
                        cx.notify();
                        return false;
                    }
                }
            }
            g.vt.take_title()
        };
        if let Some(title) = title {
            let title = if title.is_empty() { self.address.clone() } else { title };
            self.set_title(&title, cx);
        }
        if !open {
            cx.emit(TerminalEvent::Ended);
        }
        // Output in a terminal nobody sees doesn't redraw the window; it's
        // drawn fresh when shown again. Nor does output that changed nothing
        // on screen (an agent spinning its title).
        if self.shown_in.get() == FRAME.load(Ordering::Relaxed) && self.grid.borrow_mut().vt.shows_change() {
            self.redraw_soon(cx);
        }
        open
    }

    /// Output is drawn on a shared clock (~30 a second), so panes printing
    /// out of step share window frames; right after typing, at once.
    fn redraw_soon(&self, cx: &mut Context<Self>) {
        if self.typed_at.get().is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
            cx.notify();
            return;
        }
        let me = cx.entity().downgrade();
        let start = OUTPUT_CLOCK.with(|c| {
            let mut c = c.borrow_mut();
            if !c.iter().any(|w| w.entity_id() == me.entity_id()) {
                c.push(me);
            }
            c.len() == 1
        });
        if start {
            cx.spawn(async |_, cx| {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                for view in OUTPUT_CLOCK.with(|c| std::mem::take(&mut *c.borrow_mut())) {
                    let _ = view.update(cx, |_, cx| cx.notify());
                }
            })
            .detach();
        }
    }

    /// The window puts this terminal on screen in the frame it's building.
    pub fn mark_shown(&self) {
        self.shown_in.set(FRAME.load(Ordering::Relaxed));
    }

    fn send(&self, bytes: &[u8]) {
        self.typed_at.set(Some(Instant::now()));
        let mut g = self.grid.borrow_mut();
        if let Some(ch) = g.channel
            && !bytes.is_empty()
        {
            g.vt.scroll_to_bottom();
            g.core.input(ch, bytes);
        }
    }

    /// Files dropped on the terminal: their paths typed in, quoted, as a paste
    /// (agents read a dropped image that way). A terminal on another machine
    /// gets each file copied there first (`hc put`), and its path there.
    fn dropped(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        if !self.address.contains('/') {
            let text = quote_paths(paths.iter().map(|p| p.to_string_lossy().into_owned()));
            let bytes = self.grid.borrow().vt.paste_bytes(&text);
            return self.send(&bytes);
        }
        self.put_there(paths, false, cx);
    }

    /// Copies files to this terminal's machine (`hc put`) and pastes their
    /// paths there; `copies`: the local files were only for sending.
    fn put_there(&self, paths: Vec<std::path::PathBuf>, copies: bool, cx: &mut Context<Self>) {
        let addr = self.address.clone();
        cx.spawn(async move |this, cx| {
            let there = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .filter_map(|p| {
                            let out = crate::spawn::command(crate::core_bin::path()).args(["put", "--json", &addr]).arg(p).output();
                            if copies {
                                let _ = std::fs::remove_file(p);
                            }
                            let out = out.ok()?;
                            let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
                            v.get("path").and_then(|p| p.as_str()).map(str::to_owned)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            if !there.is_empty() {
                let _ = this.update(cx, |this, _| {
                    let bytes = this.grid.borrow().vt.paste_bytes(&quote_paths(there));
                    this.send(&bytes);
                });
            }
        })
        .detach();
    }

    /// Text as a paste; copied files (Finder, a clipboard manager's image) as
    /// if dropped; an image alone (a screenshot) as Ctrl+V pastes it.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        if let Some(paths) = copied_files(&item) {
            self.dropped(paths, cx);
        } else if let Some(text) = item.text() {
            let bytes = self.grid.borrow().vt.paste_bytes(&text);
            self.send(&bytes);
        } else if item.entries().iter().any(|e| matches!(e, ClipboardEntry::Image(_))) {
            self.paste_image(cx);
        }
    }

    /// Ctrl+V: a remote image, clipboard text on Windows, or the key itself.
    pub fn paste_image(&mut self, cx: &mut Context<Self>) {
        if self.send_image(cx) {
            return;
        }
        let text = if cfg!(windows) { cx.read_from_clipboard().and_then(|c| c.text()) } else { None };
        let bytes = paste_image_fallback(&mut self.grid.borrow_mut().vt, text.as_deref());
        if let Some(bytes) = bytes {
            self.send(&bytes);
        }
    }

    /// An agent on another machine reads that machine's clipboard, so this
    /// one's image goes there as a file, and its path is pasted. A local
    /// program reads the clipboard itself: nothing to do.
    fn send_image(&self, cx: &mut Context<Self>) -> bool {
        if !self.address.contains('/') {
            return false;
        }
        let Some(path) = cx.read_from_clipboard().and_then(|item| image_file(&item)) else { return false };
        self.put_there(vec![path], true, cx);
        true
    }

    /// Copies the selection to the clipboard (and the primary selection on Linux).
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.grid.borrow().vt.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            #[cfg(target_os = "linux")]
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
    }

    /// Opens the find bar; `text`: find that. `take_keys`: the bar gets the
    /// keys, its text selected (not when an agent opens it behind a dialog).
    pub fn find_open(&mut self, text: Option<String>, take_keys: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = match &self.find {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
                self._find_events = Some(cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, _window, cx| match ev {
                    InputEvent::Change => {
                        let needle = input.read(cx).value().to_string();
                        this.grid.borrow_mut().vt.find(&needle, true);
                        cx.notify();
                    }
                    // Enter: the next match up toward older output; Shift+Enter: back down.
                    InputEvent::PressEnter { shift, .. } => this.find_step(!shift, cx),
                    InputEvent::Focus => this.find_keys = true,
                    InputEvent::PasteWithoutText | InputEvent::Blur => {}
                }));
                self.find = Some(input.clone());
                input
            }
        };
        input.update(cx, |state, cx| {
            if let Some(text) = &text {
                state.set_value(text.clone(), window, cx);
            }
            if take_keys {
                state.focus(window, cx);
                state.select_all(window, cx);
            }
        });
        if let Some(text) = text {
            self.grid.borrow_mut().vt.find(&text, true);
        }
        cx.notify();
    }

    /// The next match toward older output (`older`) or newer, scrolled into view.
    pub fn find_step(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.find.is_some() {
            self.grid.borrow_mut().vt.find_step(older);
            cx.notify();
        }
    }

    /// Closes the find bar: its marks go, the keys go back to the terminal.
    /// The view stays where the find took it.
    pub fn find_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            self._find_events = None;
            self.find_keys = false;
            self.grid.borrow_mut().vt.find("", false);
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// What to focus to give this pane the keys: the terminal, or its find bar
    /// if that had them last.
    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        match &self.find {
            Some(input) if self.find_keys => input.read(cx).focus_handle(cx),
            _ => self.focus.clone(),
        }
    }

    /// The find bar has the keys (not the terminal it's in).
    fn finding(&self, window: &Window, cx: &App) -> bool {
        self.find.is_some() && !self.focus.is_focused(window) && self.focus.contains_focused(window, cx)
    }

    fn find_bar(&self, input: &Entity<InputState>, cx: &mut Context<Self>) -> AnyElement {
        let status = self.grid.borrow().find_status;
        let has_text = !input.read(cx).value().is_empty();
        let count = match (has_text, status.total, status.selected) {
            (false, ..) => String::new(),
            (true, 0, _) if status.busy => "…".into(),
            (true, 0, _) => "No results".into(),
            (true, total, Some(i)) => format!("{}/{total}{}", i + 1, if status.busy { "+" } else { "" }),
            (true, total, None) => format!("{total}{}", if status.busy { "+" } else { "" }),
        };
        let muted = crate::settings::color("muted");
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_color(muted)
                .hover(|s| s.bg(crate::settings::color("hover")).text_color(crate::settings::color("foreground")))
                .child(label)
        };
        div()
            .id(SharedString::from(format!("find-{}", self.address)))
            .absolute()
            .top_2()
            .right_3()
            .flex()
            .items_center()
            .gap_1p5()
            .pl_2p5()
            .pr_1()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(crate::settings::color("border"))
            .bg(crate::settings::color("popover"))
            .text_color(crate::settings::color("foreground"))
            // The interface's own text size (Settings → Look), like the sidebars'.
            .text_size(rems(1.0))
            .cursor_default()
            // Presses here are the bar's: they don't select text, paste, open the
            // pane menu or reach the program. Releases and moves pass on, for
            // gestures that started elsewhere (a program's drag, a divider).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.find_close(window, cx);
                }
            }))
            .child(div().w(rems(15.0)).child(gpui_base::Input::new(input)))
            .child(div().min_w(rems(4.0)).text_color(muted).child(count))
            .child(button("find-older", "↑").on_click(cx.listener(|this, _, _, cx| this.find_step(true, cx))))
            .child(button("find-newer", "↓").on_click(cx.listener(|this, _, _, cx| this.find_step(false, cx))))
            .child(button("find-close", "×").on_click(cx.listener(|this, _, window, cx| this.find_close(window, cx))))
            .into_any_element()
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the find bar are the bar's, never the program's.
        if !self.focus.is_focused(window) {
            return;
        }
        let k = &ev.keystroke;
        // Typing clears a selection, as in Herdr.
        if !k.key.starts_with("shift") && !k.key.starts_with("control") && !k.key.starts_with("alt") {
            self.grid.borrow_mut().vt.clear_selection();
        }
        let bytes = key_bytes(&mut self.grid.borrow_mut().vt, ev);
        if let Some(bytes) = bytes {
            self.send(&bytes);
            cx.stop_propagation();
        }
    }

    fn scroll(&mut self, ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let mut g = self.grid.borrow_mut();
        let steps = scroll_delta_steps(&mut self.scrolled, ev.delta, ev.touch_phase, g.cell, g.vt.cols, g.vt.rows);
        let n = steps.y.unsigned_abs() as usize;
        if steps.x == 0 && n == 0 {
            return;
        }
        let up = steps.y > 0;
        if g.vt.mouse_tracking() {
            let ((x, y), size) = mouse_at(&g, ev.position);
            let out = scroll_mouse_bytes(&mut g.vt, steps, x, y, size);
            if let Some(ch) = g.channel {
                g.core.input(ch, &out);
            }
        } else if n == 0 {
            return;
        } else if g.vt.alt_screen() {
            let key = if up { GHOSTTY_KEY_ARROW_UP } else { GHOSTTY_KEY_ARROW_DOWN };
            let mut out = Vec::new();
            for _ in 0..n {
                out.extend(g.vt.encode_key(key, 0, 0, None, None));
            }
            if let Some(ch) = g.channel {
                g.core.input(ch, &out);
            }
        } else {
            g.vt.scroll_viewport(if up { -(n as isize) } else { n as isize });
        }
        cx.notify();
    }

    /// The program tracks the mouse and Shift isn't held (Shift keeps the mouse Herdr's).
    fn takes_mouse(&self, mods: Modifiers) -> bool {
        self.grid.borrow().vt.mouse_tracking() && !mods.shift
    }

    /// Sends a mouse event to a program that tracks the mouse. Returns whether it did
    /// (shift keeps the mouse for Herdr: selecting, the menu).
    fn mouse(&mut self, action: GhosttyMouseAction, button: Option<MouseButton>, pos: Point<Pixels>, mods: Modifiers) -> bool {
        let mut g = self.grid.borrow_mut();
        if !track_mouse_button(&mut g.buttons_down, action, button) {
            return false;
        }
        if !g.vt.mouse_tracking() || mods.shift {
            return false;
        }
        let b = button.map(|b| match b {
            MouseButton::Left => GHOSTTY_MOUSE_BUTTON_LEFT,
            MouseButton::Right => GHOSTTY_MOUSE_BUTTON_RIGHT,
            MouseButton::Middle => GHOSTTY_MOUSE_BUTTON_MIDDLE,
            _ => GHOSTTY_MOUSE_BUTTON_UNKNOWN,
        });
        let ((x, y), size) = mouse_at(&g, pos);
        let any = !g.buttons_down.is_empty();
        let bytes = g.vt.encode_mouse(action, b, 0, x, y, size, any);
        if let Some(ch) = g.channel {
            g.core.input(ch, &bytes);
        }
        true
    }

    /// The pointer is over the pane: pressing or letting go of the link key redraws it.
    pub fn hovered(&self) -> bool {
        self.grid.borrow().hover.is_some()
    }

    /// The link at a window position, and where it shows.
    fn link_at(&self, pos: Point<Pixels>) -> Option<(LinkTarget, vt::Spans)> {
        let g = self.grid.borrow();
        let (x, y) = local(&g, pos);
        let (col, row) = (x / f32::from(g.cell.width), y / f32::from(g.cell.height));
        let mut target = None;
        let hit = g.vt.link_at(col as u16, row as u16, |text, at, joined| {
            // In lines joined back together only a file that exists proves the join right.
            if let Some(range) = links::find(text, at)
                && let Some(link) = links::resolve(&text[range.clone()], self.cwd.as_deref())
                && (!joined || matches!(link, links::Target::File(_)))
            {
                target = Some(LinkTarget::External(link));
                return Some(range);
            }
            if joined {
                return None;
            }
            let range = links::word(text, at)?;
            let address = crate::mods::terminal_address(&text[range.clone()], &self.address)?;
            target = Some(LinkTarget::Terminal(address));
            Some(range)
        })?;
        // OSC 8 links bypass text detection.
        let target = target.or_else(|| links::resolve(&hit.target, self.cwd.as_deref()).map(LinkTarget::External))?;
        Some((target, hit.spans))
    }

    fn start_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        self.selecting = true;
        self.selection_pointer = pos;
        self.select(Select::Press, pos, cx);
        self.selection_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let keep = this.update(cx, |this, cx| {
                    if !this.selecting { return false; }
                    if this.shown_in.get() != FRAME.load(Ordering::Relaxed) {
                        this.finish_selection(None, cx);
                        return false;
                    }
                    let mut g = this.grid.borrow_mut();
                    let y = f32::from(this.selection_pointer.y - g.origin.y);
                    let lines = selection_scroll_lines(y, f32::from(g.cell.height), g.vt.rows);
                    if lines != 0 {
                        g.vt.scroll_viewport(lines);
                        drop(g);
                        this.select(Select::Drag, this.selection_pointer, cx);
                    }
                    true
                }).unwrap_or(false);
                if !keep { break; }
            }
        }));
    }

    fn finish_selection(&mut self, pos: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut g = self.grid.borrow_mut();
        g.buttons_down.retain(|b| *b != MouseButton::Left);
        if let Some(pos) = pos {
            drop(g);
            self.select(Select::Release, pos, cx);
        } else {
            g.vt.clear_selection();
            cx.notify();
        }
    }

    /// Selecting with the mouse (when the program doesn't take it).
    fn select(&mut self, step: Select, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let mut g = self.grid.borrow_mut();
        let (x, y) = local(&g, pos);
        let (cw, ch) = (f32::from(g.cell.width), f32::from(g.cell.height));
        let (col, row) = ((x / cw) as u16, (y / ch) as u16);
        let height = ch * g.vt.rows as f32;
        g.vt.select(step, col, row, x, y, cw, height);
        drop(g);
        if matches!(step, Select::Release) {
            // Copy on select, as in Herdr.
            self.copy(cx);
        }
        cx.notify();
    }
}

fn selection_scroll_lines(y: f32, cell_height: f32, rows: u16) -> isize {
    let height = cell_height * rows as f32;
    let outside = if y < 0.0 { y } else if y >= height { y - height + 1.0 } else { return 0 };
    (outside / cell_height).abs().ceil().clamp(1.0, 5.0) as isize * if outside < 0.0 { -1 } else { 1 }
}

/// A title as it's shown: an agent's spinner in front of it (`\u{2733} Fix the
/// tests`, braille dots) is no part of it. Herdr draws its own, and a title
/// that spins would redraw every mod that shows titles, 10 times a second.
pub(crate) fn shown_title(title: &str) -> &str {
    // Frames agents spin, not status icons (\u{2713} build stays): braille dots,
    // Claude Code's stars and its half circles.
    let spinner = |c: char| {
        matches!(c, '\u{2800}'..='\u{28ff}' | '\u{00b7}' | '\u{2722}' | '\u{2733}' | '\u{2736}' | '\u{273b}' | '\u{273d}' | '\u{25d0}'..='\u{25d3}')
    };
    let rest = title.trim_start_matches(|c: char| spinner(c) || c.is_whitespace());
    let led = title.len() - rest.len();
    if rest.is_empty() || !title[..led].chars().any(spinner) || !title[..led].ends_with(char::is_whitespace) { title } else { rest }
}

/// Paths as a shell reads them, space-separated with a trailing space: bare
/// when plain, else single-quoted (`'` escaped as the shell wants it).
fn quote_paths(paths: impl IntoIterator<Item = String>) -> String {
    let quote = |p: String| {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=,:@%~".contains(c)) {
            return p;
        }
        if cfg!(windows) { format!("'{}'", p.replace('\'', "''")) } else { format!("'{}'", p.replace('\'', "'\\''")) }
    };
    let mut out: String = paths.into_iter().map(quote).collect::<Vec<_>>().join(" ");
    out.push(' ');
    out
}

/// Files on the clipboard: their text is only the bare path, which an agent
/// reads as words, not as an image to attach.
fn copied_files(item: &ClipboardItem) -> Option<Vec<std::path::PathBuf>> {
    item.entries().iter().find_map(|e| match e {
        ClipboardEntry::ExternalPaths(p) if !p.paths().is_empty() => Some(p.paths().to_vec()),
        _ => None,
    })
}

/// The clipboard's image saved to a file of its own, for its path to be pasted.
fn image_file(item: &ClipboardItem) -> Option<std::path::PathBuf> {
    let image = item.entries().iter().find_map(|e| match e {
        ClipboardEntry::Image(i) => Some(i),
        _ => None,
    })?;
    let (format, bytes) = paste_image_bytes(image.format, &image.bytes)?;
    let ext = match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::Webp => "webp",
        ImageFormat::Svg => "svg",
        _ => "png",
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let path = std::env::temp_dir().join(format!("herdr-paste-{stamp}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn paste_image_bytes(format: ImageFormat, bytes: &[u8]) -> Option<(ImageFormat, std::borrow::Cow<'_, [u8]>)> {
    let source = match format {
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        _ => return Some((format, std::borrow::Cow::Borrowed(bytes))),
    };
    let image = image::load_from_memory_with_format(bytes, source).ok()?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some((ImageFormat::Png, std::borrow::Cow::Owned(png.into_inner())))
}

fn link_mods(m: &Modifiers) -> bool {
    let key = if cfg!(target_os = "macos") { m.platform && !m.control } else { m.control && !m.platform };
    key && !m.alt && !m.shift
}

fn local(g: &Grid, p: Point<Pixels>) -> (f32, f32) {
    (f32::from(p.x - g.origin.x).max(0.0), f32::from(p.y - g.origin.y).max(0.0))
}

fn scroll_delta_steps(scrolled: &mut Point<f32>, delta: ScrollDelta, phase: TouchPhase, cell: Size<Pixels>, cols: u16, rows: u16) -> Point<i32> {
    let lines = match delta {
        // As in Ghostty, a sideways notch moves one column; a vertical one, three lines.
        ScrollDelta::Lines(p) if cfg!(target_os = "macos") => return point(macos_wheel_steps(p.x) / 3, macos_wheel_steps(p.y)),
        ScrollDelta::Lines(p) => p,
        ScrollDelta::Pixels(p) => point(p.x / cell.width, p.y / cell.height),
    };
    point(scroll_steps(&mut scrolled.x, phase, lines.x, cols as f32), scroll_steps(&mut scrolled.y, phase, lines.y, rows as f32))
}

fn scroll_mouse_bytes(vt: &mut Vt, steps: Point<i32>, x: f32, y: f32, size: (u32, u32, f32, f32)) -> Vec<u8> {
    let mut out = Vec::new();
    for (steps, positive, negative) in [
        (steps.y, GHOSTTY_MOUSE_BUTTON_FOUR, GHOSTTY_MOUSE_BUTTON_FIVE),
        (steps.x, GHOSTTY_MOUSE_BUTTON_SIX, GHOSTTY_MOUSE_BUTTON_SEVEN),
    ] {
        let button = if steps > 0 { positive } else { negative };
        for _ in 0..steps.unsigned_abs() {
            out.extend(vt.encode_mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(button), 0, x, y, size, false));
        }
    }
    out
}

/// Whole lines to scroll for one scroll event. A touchpad sends many small
/// moves: they add up to a line before one scrolls (as in Zed), and each
/// gesture starts from nothing. Kept under a screen, so turning back is quick.
fn scroll_steps(scrolled: &mut f32, phase: TouchPhase, lines: f32, rows: f32) -> i32 {
    match phase {
        TouchPhase::Started => {
            *scrolled = 0.0;
            0
        }
        TouchPhase::Moved => {
            let before = scrolled.trunc() as i32;
            *scrolled += lines;
            let after = scrolled.trunc() as i32;
            *scrolled %= rows.max(1.0);
            after - before
        }
        _ => 0,
    }
}

/// Lines for a macOS mouse wheel event, as Ghostty scrolls them. macOS reports
/// wheel ticks, a slow notch as a fraction of one (often 0.1), so an event counts
/// as at least one tick, times Ghostty's default wheel multiplier of 3, with the
/// fraction dropped.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_wheel_steps(ticks: f32) -> i32 {
    if ticks == 0.0 {
        return 0;
    }
    (ticks.signum() * ticks.abs().max(1.0) * 3.0).trunc() as i32
}

/// A pointer position and the grid for mouse reports, in device pixels
/// (SGR-pixels mode reports pixels, as in Ghostty).
fn mouse_at(g: &Grid, p: Point<Pixels>) -> ((f32, f32), (u32, u32, f32, f32)) {
    let ((x, y), s) = (local(g, p), g.scale);
    ((x * s, y * s), (g.vt.cols as u32, g.vt.rows as u32, f32::from(g.cell.width) * s, f32::from(g.cell.height) * s))
}

/// An icon's glyph in the bundled symbols font, with its outline's bounds in
/// font units (y up).
struct IconGlyph {
    id: u32,
    units_per_em: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Read from the font file itself, not through text shaping: shaping may
/// take the character from another installed font (old Nerd Fonts put a
/// squirrel where the markdown icon is now).
fn icon_glyph(c: char) -> Option<IconGlyph> {
    thread_local! {
        static FACE: Option<ttf_parser::Face<'static>> = ttf_parser::Face::parse(crate::settings::SYMBOLS_FONT_FILE, 0).ok();
    }
    FACE.with(|face| {
        let face = face.as_ref()?;
        let id = face.glyph_index(c)?;
        let b = face.glyph_bounding_box(id)?;
        Some(IconGlyph { id: id.0 as u32, units_per_em: face.units_per_em() as f32, x0: b.x_min as f32, y0: b.y_min as f32, x1: b.x_max as f32, y1: b.y_max as f32 })
    })
}

/// A Nerd Font icon (Unicode private use areas). Drawn from the bundled
/// symbols font and fitted to the cell, whatever the terminal font.
fn is_icon(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFD))
}

fn paste_image_fallback(vt: &mut vt::Vt, text: Option<&str>) -> Option<Vec<u8>> {
    // Windows console programs expect the terminal to paste clipboard text.
    if cfg!(windows) && let Some(text) = text {
        return Some(vt.paste_bytes(text));
    }
    let keystroke = Keystroke::parse("ctrl-v").ok()?;
    key_bytes(vt, &KeyDownEvent { keystroke, is_held: false, prefer_character_input: false })
}

/// What a key press sends to the program: the key described to libghostty,
/// which encodes it once, in the mode the program asked for (as Ghostty does).
fn key_bytes(vt: &mut vt::Vt, ev: &KeyDownEvent) -> Option<Vec<u8>> {
    // GPUI sets this preference on replayed shortcut prefixes too; only
    // Windows' native composition previews defer to the text input handler.
    #[cfg(windows)]
    if ev.prefer_character_input && gpui_windows::is_native_character_input(ev) {
        return None;
    }
    let k = &ev.keystroke;
    let m = k.modifiers;
    if vt.win32.on && vt.kitty_keyboard_flags() == 0 {
        if k.key == "enter" && (m.shift || m.control || m.alt) {
            return Some(crate::win32_keys::enter(m.shift, m.control, m.alt));
        }
        // ConPTY combines a bare Escape with the next key as an Alt chord.
        if k.key == "escape" {
            return Some(crate::win32_keys::escape(m.shift, m.control, m.alt));
        }
    }
    let Some((key, shifted, unshifted)) = map_key(&k.key) else {
        return k.key_char.as_ref().map(|text| text.as_bytes().to_vec());
    };
    let mut mods: GhosttyMods = 0;
    if m.shift || shifted {
        mods |= GHOSTTY_MODS_SHIFT as GhosttyMods;
    }
    if m.control {
        mods |= GHOSTTY_MODS_CTRL as GhosttyMods;
    }
    if m.alt {
        mods |= GHOSTTY_MODS_ALT as GhosttyMods;
        #[cfg(target_os = "macos")]
        if k.option_side == Some(gpui::OptionSide::Right)
            || (k.option_side == Some(gpui::OptionSide::Both) && gpui_macos::option_as_alt() == gpui_macos::OptionAsAlt::Right)
        {
            mods |= GHOSTTY_MODS_ALT_SIDE as GhosttyMods;
        }
    }
    if m.platform {
        mods |= GHOSTTY_MODS_SUPER as GhosttyMods;
    }
    let text = match k.key.as_str() {
        "space" => Some(" ".to_owned()),
        _ => k.key_char.clone(),
    };
    // macOS translates Option into text unless it is configured as Alt.
    // That text can be ASCII too (Slovenian Shift-Option-2 -> "@").
    // Other platforms keep Alt shortcuts unless they produced non-ASCII text.
    let mut consumed = mods & GHOSTTY_MODS_SHIFT as GhosttyMods;
    let option_as_alt = {
        #[cfg(target_os = "macos")]
        { gpui_macos::option_as_alt().uses_alt(k.option_side) }
        #[cfg(not(target_os = "macos"))]
        { false }
    };
    if m.alt && !option_as_alt && text.as_deref().is_some_and(|t| !t.is_empty() && (cfg!(target_os = "macos") || !t.is_ascii())) {
        consumed |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    Some(vt.encode_key(key, mods, consumed, text.as_deref(), unshifted))
}

fn map_key(key: &str) -> Option<(GhosttyKey, bool, Option<char>)> {
    let named = match key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "space" => return Some((GHOSTTY_KEY_SPACE, false, Some(' '))),
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    if named != GHOSTTY_KEY_UNIDENTIFIED {
        return Some((named, false, None));
    }
    // C enums are int on Linux and unsigned on macOS, so offsets take the constants' own type.
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&n)
    {
        return Some((GHOSTTY_KEY_F1 + (n - 1) as GhosttyKey, false, None));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    const SHIFTED: &[(char, char)] = &[
        ('~', '`'), ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'), ('&', '7'),
        ('*', '8'), ('(', '9'), (')', '0'), ('_', '-'), ('+', '='), ('{', '['), ('}', ']'), ('|', '\\'),
        (':', ';'), ('"', '\''), ('<', ','), ('>', '.'), ('?', '/'),
    ];
    let (base, shifted) = match SHIFTED.iter().find(|(s, _)| *s == c) {
        Some((_, b)) => (*b, true),
        None => (c, false),
    };
    let code = match base {
        'a'..='z' => GHOSTTY_KEY_A + (base as u8 - b'a') as GhosttyKey,
        '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (base as u8 - b'0') as GhosttyKey,
        '`' => GHOSTTY_KEY_BACKQUOTE,
        '\\' => GHOSTTY_KEY_BACKSLASH,
        '[' => GHOSTTY_KEY_BRACKET_LEFT,
        ']' => GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => GHOSTTY_KEY_COMMA,
        '=' => GHOSTTY_KEY_EQUAL,
        '-' => GHOSTTY_KEY_MINUS,
        '.' => GHOSTTY_KEY_PERIOD,
        '\'' => GHOSTTY_KEY_QUOTE,
        ';' => GHOSTTY_KEY_SEMICOLON,
        '/' => GHOSTTY_KEY_SLASH,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    Some((code, shifted, Some(base)))
}

fn cell_font(bold: bool, italic: bool) -> Font {
    let mut f = font(crate::settings::terminal_font());
    // Ligatures merge several cells into one glyph, which breaks the grid.
    f.features = FontFeatures(std::sync::Arc::new(vec![("calt".into(), 0), ("liga".into(), 0), ("dlig".into(), 0)]));
    f.weight = if bold { FontWeight::BOLD } else { FontWeight::MEDIUM };
    f.style = if italic { FontStyle::Italic } else { FontStyle::Normal };
    f
}

pub fn cell_size(window: &Window) -> Size<Pixels> {
    let ts = window.text_system();
    let f = cell_font(false, false);
    let id = ts.resolve_font(&f);
    let fs = px(crate::settings::font_size());
    let w = ts.advance(id, fs, 'M').map(|s| s.width).unwrap_or(fs * 0.6);
    let h = ts.ascent(id, fs) + ts.descent(id, fs).abs();
    // The width stays the font's own advance: GPUI only moves a glyph onto the
    // grid once it drifts over 1px, so a rounded width made text wobble.
    // Boxes snap their edges instead (`snap_box`).
    let scale = window.scale_factor();
    size(w, px((f32::from(h) * 1.15 * scale).ceil() / scale))
}

/// One piece of a row's drawing, in cells from the row's start.
enum RowOp {
    Bg { from: usize, to: usize, color: Hsla },
    Sprite { x: usize, text: String, color: Hsla },
    Icon { x: usize, offset: Point<Pixels>, font: FontId, glyph: GlyphId, size: Pixels, color: Hsla },
    Text { x: usize, line: ShapedLine },
}

/// A row's drawing: its backgrounds, merged into runs of one color, then its
/// text in runs of one style (wide characters, sprites and icons on their own).
fn row_ops(row: &[vt::Cell], cell: Size<Pixels>, window: &mut Window) -> Vec<RowOp> {
    crate::bench::count(&crate::bench::ROWS);
    let mut ops = Vec::new();
    let mut x = 0;
    while x < row.len() {
        let Some(bg) = row[x].bg else {
            x += 1;
            continue;
        };
        let start = x;
        while x < row.len() && row[x].bg == Some(bg) {
            x += 1;
        }
        ops.push(RowOp::Bg { from: start, to: x, color: hsla(bg) });
    }
    let mut x = 0;
    while x < row.len() {
        let start = x;
        let c = &row[start];
        // Consume the first cell before dispatch: every path must make progress,
        // even when a private-use character has no bundled icon glyph.
        x += 1;
        if c.text.is_empty() && !c.underline && !c.strike {
            continue;
        }
        let mut color = hsla(c.fg);
        if c.faint {
            color.a = 0.6;
        }
        if crate::sprites::is_sprite(&c.text) {
            ops.push(RowOp::Sprite { x: start, text: c.text.clone(), color });
            continue;
        }
        if let Some(icon) = c.text.chars().next().filter(|_| is_icon(&c.text)).and_then(icon_glyph) {
            // Fitted to the cell by its real outline, as Ghostty does:
            // centered, as tall as fits, two cells wide when a space
            // follows (prompts, statuslines, file trees).
            let room = if row.get(x).is_none_or(|d| d.text.trim().is_empty()) { 2.0 } else { 1.0 };
            let (w, h) = (f32::from(cell.width) * room, f32::from(cell.height));
            let (gw, gh) = ((icon.x1 - icon.x0).max(1.0), (icon.y1 - icon.y0).max(1.0));
            // Up to ~1.5 cells of two, so it doesn't touch the next word.
            let scale = (w * if room > 1.0 { 0.75 } else { 0.9 } / gw).min(h * 0.72 / gh);
            // Centered in its own cell; a wider icon starts at the cell
            // and spills into the space after it.
            let cw = f32::from(cell.width);
            let left = ((cw - gw * scale) / 2.0).max(0.0) - icon.x0 * scale;
            let baseline = (h - gh * scale) / 2.0 + icon.y1 * scale;
            let font = window.text_system().resolve_font(&font(crate::settings::SYMBOLS_FONT));
            let size = px(scale * icon.units_per_em);
            ops.push(RowOp::Icon { x: start, offset: point(px(left), px(baseline)), font, glyph: GlyphId(icon.id), size, color });
            continue;
        }
        let mut text = if c.text.is_empty() { " ".to_owned() } else { c.text.clone() };
        if !c.wide {
            while x < row.len() {
                let d = &row[x];
                let same = !d.wide
                    && d.fg == c.fg
                    && d.bold == c.bold
                    && d.italic == c.italic
                    && d.faint == c.faint
                    && d.underline == c.underline
                    && d.strike == c.strike;
                // A missing icon breaks the run too; the next outer pass draws it as text.
                if !same || (d.text.is_empty() && !d.underline && !d.strike) || crate::sprites::is_sprite(&d.text) || is_icon(&d.text) {
                    break;
                }
                text.push_str(if d.text.is_empty() { " " } else { &d.text });
                x += 1;
            }
        }
        if text.trim().is_empty() && !c.underline && !c.strike {
            continue;
        }
        let run = TextRun {
            len: text.len(),
            font: cell_font(c.bold, c.italic),
            color,
            background_color: None,
            underline: c.underline.then(|| UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
            strikethrough: c.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
        };
        let force = (!c.wide).then_some(cell.width);
        let line = window.text_system().shape_line(text.into(), px(crate::settings::font_size()), &[run], force);
        ops.push(RowOp::Text { x: start, line });
    }
    ops
}

fn paint_grid(bounds: Bounds<Pixels>, grid: &Rc<RefCell<Grid>>, focused: bool, window: &mut Window, cx: &mut App) {
    crate::bench::count(&crate::bench::GRID);
    let mut g = grid.borrow_mut();
    let cell = g.cell;
    let (pad_x, pad_y) = pad();
    // Rows rarely fill the height exactly: split what's left above and below.
    let spare = (f32::from(bounds.size.height) - 2.0 * pad_y - f32::from(cell.height) * g.vt.rows as f32).max(0.0);
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let origin = bounds.origin + point(px(pad_x), px(pad_y + (spare / 2.0).floor()));
    let origin = point(snap(origin.x), snap(origin.y));
    g.origin = origin;
    let frame = g.vt.frame();
    // A find reads the scrollback a frame at a time: draw again until it's
    // done, and whenever its count changed (the bar shows it).
    let found = g.vt.find_status();
    if found.busy || found != g.find_status {
        g.find_status = found;
        window.request_animation_frame();
    }
    let placements = g.vt.placements();
    let mut images: Vec<(bool, Bounds<Pixels>, Arc<RenderImage>)> = Vec::new();
    let mut keep = std::collections::HashSet::new();
    for p in &placements {
        let key = (p.image, p.generation, p.source);
        keep.insert(key);
        if !g.images.contains_key(&key)
            && let Some((w, h, bgra)) = g.vt.image_bgra(p.image, p.source)
            && let Some(buf) = image::RgbaImage::from_raw(w, h, bgra)
        {
            g.images.insert(key, Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])));
        }
        if let Some(img) = g.images.get(&key) {
            let [x, y, mut width, mut height] = p.pixel_geometry(f32::from(cell.width), f32::from(cell.height));
            if p.cell_pixels.is_none() {
                // Direct placements are in device pixels.
                width /= scale;
                height /= scale;
            }
            let pos = origin + point(px(x), px(y));
            // Placeholder runs contain no text and must cover explicit cell backgrounds.
            let below = p.z < 0 && p.cell_pixels.is_none();
            images.push((below, Bounds::new(pos, size(px(width), px(height))), img.clone()));
        }
    }
    g.images.retain(|k, _| keep.contains(k));
    let link_spans = g.link_spans.clone();
    let marked = g.marked.clone();
    drop(g);

    window.paint_quad(fill(bounds, hsla(frame.bg).opacity(crate::settings::get().terminal_opacity)));
    let at = |x: usize, y: usize| origin + point(cell.width * x as f32, cell.height * y as f32);
    // Edges round to device pixels, so cells side by side meet without seams.
    let snap_box = |b: Bounds<Pixels>| {
        let (x0, x1) = (snap(b.origin.x), snap(b.origin.x + b.size.width));
        let (y0, y1) = (snap(b.origin.y), snap(b.origin.y + b.size.height));
        Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
    };
    let paint_images = |below: bool, window: &mut Window| {
        for (image_below, b, img) in &images {
            if *image_below == below {
                let _ = window.paint_image(*b, *b, Corners::default(), img.clone(), 0, false);
            }
        }
    };
    paint_images(true, window);

    // Each row's drawing is built once and painted again while the row is the
    // same (`Vt::frame` keeps a row libghostty didn't change): an agent's spinner
    // line rebuilds one row, not the whole screen.
    let with = (crate::settings::font_size().to_bits(), cell, crate::settings::terminal_font(), crate::settings::fonts_added());
    // When every row changed (output scrolling), nothing is kept: the next
    // frame would find none of them the same.
    let mut painted = std::mem::take(&mut grid.borrow_mut().painted);
    if grid.borrow().painted_with != with || frame.all {
        painted.clear();
    }
    painted.truncate(frame.rows.len());
    for (y, row) in frame.rows.iter().enumerate() {
        let fresh;
        let ops = if frame.all {
            fresh = row_ops(row, cell, window);
            &fresh
        } else {
            if painted.get(y).is_none_or(|(r, _)| !Rc::ptr_eq(r, row)) {
                let ops = row_ops(row, cell, window);
                if y < painted.len() {
                    painted[y] = (row.clone(), ops);
                } else {
                    painted.push((row.clone(), ops));
                }
            }
            &painted[y].1
        };
        for op in ops {
            match op {
                RowOp::Bg { from, to, color } => {
                    let b = Bounds::new(at(*from, y), size(cell.width * (to - from) as f32, cell.height));
                    window.paint_quad(fill(snap_box(b), *color));
                }
                RowOp::Sprite { x, text, color } => crate::sprites::paint(text, Bounds::new(at(*x, y), cell), *color, window),
                RowOp::Icon { x, offset, font, glyph, size, color } => {
                    let _ = window.paint_glyph(at(*x, y) + *offset, *font, *glyph, *size, *color);
                }
                RowOp::Text { x, line } => {
                    let _ = line.paint(at(*x, y), cell.height, TextAlign::Left, None, window, cx);
                }
            }
        }
    }
    {
        let mut g = grid.borrow_mut();
        (g.painted, g.painted_with) = (painted, with);
    }

    for &(y, x0, x1) in &link_spans {
        let Some(row) = frame.rows.get(y as usize) else { continue };
        let color = row.get(x0 as usize).map_or(hsla(vt::theme().fg), |c| hsla(c.fg));
        let (x0, x1) = (x0 as usize, (x1 as usize + 1).min(row.len()));
        let line = Bounds::new(at(x0, y as usize) + point(px(0.0), cell.height - px(2.0)), size(cell.width * (x1 - x0) as f32, px(1.0)));
        window.paint_quad(fill(snap_box(line), color));
    }

    paint_images(false, window);

    if let Some(cur) = frame.cursor {
        let (x, y) = (cur.x as usize, cur.y as usize);
        let wide = frame.rows.get(y).and_then(|r| r.get(x)).is_some_and(|c| c.wide);
        let w = cell.width * if wide { 2.0 } else { 1.0 };
        let color = hsla(cur.color);
        let p = at(x, y);
        match (focused, cur.shape) {
            (true, CursorShape::Block) => {
                window.paint_quad(fill(snap_box(Bounds::new(p, size(w, cell.height))), color));
                if let Some(c) = frame.rows.get(y).and_then(|r| r.get(x)).filter(|c| !c.text.is_empty()) {
                    let run = TextRun {
                        len: c.text.len(),
                        font: cell_font(c.bold, c.italic),
                        color: hsla(frame.bg),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(c.text.clone().into(), px(crate::settings::font_size()), &[run], None);
                    let _ = line.paint(p, cell.height, TextAlign::Left, None, window, cx);
                }
            }
            (true, CursorShape::Bar) => window.paint_quad(fill(snap_box(Bounds::new(p, size(px(2.0), cell.height))), color)),
            (true, CursorShape::Underline) => window.paint_quad(fill(
                snap_box(Bounds::new(p + point(px(0.0), cell.height - px(2.0)), size(w, px(2.0)))),
                color,
            )),
            _ => window.paint_quad(outline(snap_box(Bounds::new(p, size(w, cell.height))), color, BorderStyle::Solid)),
        }
    }
    // What an input method is composing, at the cursor (hidden or not):
    // underlined, on the terminal's background so the cells under it don't
    // show through.
    if let (Some((cx_, cy_)), Some(text)) = (frame.cursor_cell, marked.filter(|t| !t.is_empty())) {
        let p = at(cx_ as usize, cy_ as usize);
        let fg = hsla(vt::theme().fg);
        let run = TextRun {
            len: text.len(),
            font: cell_font(false, false),
            color: fg,
            background_color: None,
            underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(fg), wavy: false }),
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text.into(), px(crate::settings::font_size()), &[run], None);
        window.paint_quad(fill(snap_box(Bounds::new(p, size(line.width, cell.height))), hsla(frame.bg)));
        let _ = line.paint(p, cell.height, TextAlign::Left, None, window, cx);
    }
    grid.borrow_mut().cursor_at = frame.cursor_cell.map(|(x, y)| Bounds::new(at(x as usize, y as usize), cell));
}

/// OS text input (AltGr, Pinyin, Japanese, Korean, dead keys) types here.
/// Native composition previews reach this handler; other keys go through
/// `key_bytes`. The program gets the finished text as it is.
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, range: std::ops::Range<usize>, adjusted: &mut Option<std::ops::Range<usize>>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<String> {
        let marked = self.grid.borrow().marked.clone().unwrap_or_default();
        let units: Vec<u16> = marked.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _ignore_disabled: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.grid.borrow().marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.grid.borrow().marked.as_ref().filter(|m| !m.is_empty()).map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<std::ops::Range<usize>>, text: &str, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        self.send(text.as_bytes());
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _selected: Option<std::ops::Range<usize>>,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grid.borrow_mut().marked = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: std::ops::Range<usize>, _element: Bounds<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let g = self.grid.borrow();
        let at = g.cursor_at?;
        Some(Bounds::new(at.origin + point(g.cell.width * range.start as f32, px(0.0)), at.size))
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::bench::count(&crate::bench::TERMINAL);
        // Typing in its find bar, the terminal still counts as focused (for its
        // program and its cursor); only the input methods stay with the bar.
        let typing = self.focus.is_focused(window);
        if typing {
            self.find_keys = false;
        }
        // The bar's own field counts at once: until a frame shows it, the
        // terminal's tree doesn't know it's inside.
        let in_bar = self.find.as_ref().is_some_and(|f| f.read(cx).focus_handle(cx).is_focused(window));
        let focused = typing || in_bar || self.focus.contains_focused(window, cx);
        if focused != self.was_focused {
            self.was_focused = focused;
            let g = self.grid.borrow();
            if let Some(ch) = g.channel {
                g.core.focus(ch, focused);
            }
        }
        let cell = cell_size(window);
        let hover = self.grid.borrow().hover;
        let found = hover.filter(|_| link_mods(&window.modifiers())).and_then(|pos| self.link_at(pos));
        let (link, spans) = found.unzip();
        self.link = link;
        self.grid.borrow_mut().link_spans = spans.unwrap_or_default();
        let grid = self.grid.clone();
        let grid_paint = self.grid.clone();
        let selection_view = cx.entity().downgrade();
        let (input_focus, input_view) = (self.focus.clone(), cx.entity());
        let ended = self.ended.clone().or_else(|| self.lost.then(|| "reconnecting to core…".to_owned()));
        let find_bar = self.find.clone().map(|input| self.find_bar(&input, cx));

        div()
            .id(SharedString::from(format!("term-{}", self.address)))
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .relative()
            // Below 1, the layout's background shows through (cells a program colors stay solid).
            .bg(hsla(vt::theme().bg).opacity(crate::settings::get().terminal_opacity))
            .on_action(cx.listener(|this, a: &gpui_shell::action::ShellAction, window, cx| {
                if this.finding(window, cx) {
                    // The terminal's keys don't type into its program from the find
                    // bar; pasting there pastes into the bar.
                    match a.id() {
                        id if id.starts_with("send:") => {}
                        "terminal.paste" | "terminal.paste_image" => {
                            let text = cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.replace(['\r', '\n'], " "));
                            if let (Some(text), Some(input)) = (text, this.find.clone()) {
                                input.update(cx, |state, cx| state.insert(text, window, cx));
                                let needle = input.read(cx).value().to_string();
                                this.grid.borrow_mut().vt.find(&needle, true);
                                cx.notify();
                            }
                        }
                        _ => cx.propagate(),
                    }
                    return;
                }
                // `send:<text>` bindings from keys.json type into this terminal.
                match a.id().strip_prefix("send:") {
                    Some(text) => this.send(text.as_bytes()),
                    None => cx.propagate(),
                }
            }))
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.dropped(paths.paths().to_vec(), cx)))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .when(self.link.is_some(), |d| d.cursor_pointer())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.grid.borrow_mut().hover = None;
                    if this.link.is_some() {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                this.focus.focus(window, cx);
                if link_mods(&ev.modifiers)
                    && let Some((target, _)) = this.link_at(ev.position)
                {
                    match target {
                        LinkTarget::External(target) => links::open(&target),
                        LinkTarget::Terminal(address) => cx.emit(TerminalEvent::Peek(address)),
                    }
                    return;
                }
                if !this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers) {
                    this.start_selection(ev.position, cx);
                }
            }))
            // Right-click is Herdr's menu, even when the program tracks the mouse
            // (agent TUIs do, and then a pane had no menu at all), unless the pane
            // sends right-clicks to its program.
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if this.right_click_to_program && this.takes_mouse(ev.modifiers) {
                    this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers);
                } else {
                    cx.emit(TerminalEvent::ContextMenu(ev.position));
                }
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if !this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers) {
                    this.paste(cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.grid.borrow_mut().hover = Some(ev.position);
                if link_mods(&ev.modifiers) || this.link.is_some() {
                    cx.notify();
                }
                if !this.selecting && ev.pressed_button.is_none() {
                    this.mouse(GHOSTTY_MOUSE_ACTION_MOTION, None, ev.position, ev.modifiers);
                }
            }))
            .child(
                canvas(
                    move |bounds, window, _cx| {
                        let mut g = grid.borrow_mut();
                        g.cell = cell;
                        g.scale = window.scale_factor();
                        let dev = device_cell(cell, g.scale);
                        // Before the size, so a program woken by it asks for the new cell.
                        g.refresh_theme(&vt::theme(), dev);
                        let (px_, py_) = pad();
                        let cols = ((f32::from(bounds.size.width) - 2.0 * px_) / f32::from(cell.width)).floor().max(2.0) as u16;
                        let rows = ((f32::from(bounds.size.height) - 2.0 * py_) / f32::from(cell.height)).floor().max(1.0) as u16;
                        if (cols, rows) != (g.vt.cols, g.vt.rows) || dev != g.vt_cell {
                            g.vt.resize(cols, rows, dev.0, dev.1);
                            g.vt_cell = dev;
                            if let Some(chan) = g.channel {
                                g.core.resize(chan, cols, rows);
                                // Mode 2048 programs wait for this report, not SIGWINCH.
                                let replies = g.vt.take_replies();
                                g.core.input(chan, &replies);
                            }
                        }
                    },
                    move |bounds, _, window, cx| {
                        paint_grid(bounds, &grid_paint, focused, window, cx);
                        let view = selection_view.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = view.update(cx, |this, cx| {
                                    if !this.selecting && ev.pressed_button.is_some() && !this.grid.borrow().buttons_down.is_empty() {
                                        this.mouse(GHOSTTY_MOUSE_ACTION_MOTION, ev.pressed_button, ev.position, ev.modifiers);
                                    }
                                    if this.selecting {
                                        this.selection_pointer = ev.position;
                                        if ev.pressed_button == Some(MouseButton::Left) {
                                            this.select(Select::Drag, ev.position, cx);
                                        } else {
                                            this.selection_scroll = None;
                                            this.finish_selection(Some(ev.position), cx);
                                        }
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = selection_view.update(cx, |this, cx| {
                                    if (!this.selecting || ev.button != MouseButton::Left) && this.grid.borrow().buttons_down.contains(&ev.button) {
                                        this.mouse(GHOSTTY_MOUSE_ACTION_RELEASE, Some(ev.button), ev.position, ev.modifiers);
                                    }
                                    if this.selecting && ev.button == MouseButton::Left {
                                        this.selection_scroll = None;
                                        this.finish_selection(Some(ev.position), cx);
                                        cx.stop_propagation();
                                    }
                                });
                            }
                        });
                        // While focused, the OS input methods (Pinyin, Japanese,
                        // dead keys) type into this terminal.
                        if typing {
                            window.handle_input(&input_focus, ElementInputHandler::new(bounds, input_view), cx);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(find_bar, |d, bar| d.child(bar))
            .when_some(ended, |d, why| {
                d.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .right_3()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(crate::settings::color("raised"))
                        .text_color(hsla(vt::theme().fg))
                        .text_xs()
                        .child(format!("[{why}]")),
                )
            })
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mouse_buttons_only_follow_the_pane_that_took_each_press() {
        use super::*;
        let mut buttons = Vec::new();
        let left = Some(MouseButton::Left);
        let right = Some(MouseButton::Right);
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, None));
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, left));
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_RELEASE, left));
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_PRESS, left));
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, left));
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_RELEASE, right));
        assert_eq!(buttons, vec![MouseButton::Left]);
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_PRESS, right));
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_RELEASE, right));
        assert_eq!(buttons, vec![MouseButton::Left]);
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, right));
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_RELEASE, left));
        assert!(buttons.is_empty());
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, left));
        assert!(!track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_RELEASE, left));
        assert!(track_mouse_button(&mut buttons, GHOSTTY_MOUSE_ACTION_MOTION, None));
    }

    #[test]
    fn selection_autoscroll_only_runs_outside_the_grid_and_caps_speed() {
        use super::selection_scroll_lines as lines;
        assert_eq!(lines(0.0, 20.0, 40), 0);
        assert_eq!(lines(799.0, 20.0, 40), 0);
        assert_eq!(lines(-1.0, 20.0, 40), -1);
        assert_eq!(lines(800.0, 20.0, 40), 1);
        assert_eq!(lines(-41.0, 20.0, 40), -3);
        assert_eq!(lines(841.0, 20.0, 40), 3);
        assert_eq!(lines(-10_000.0, 20.0, 40), -5);
        assert_eq!(lines(10_000.0, 20.0, 40), 5);
    }

    struct Empty;
    impl gpui::Render for Empty {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    #[gpui::test]
    fn missing_icons_render_as_text_at_every_position(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, _| {
            for missing in ['\u{f557}', '\u{f85a}'] {
                assert!(super::icon_glyph(missing).is_none());
                assert!(super::icon_glyph('\u{f10d}').is_some());
                for (underline, strike) in [(false, false), (true, false), (false, true), (true, true)] {
                    for text in [format!("{missing}a\u{f10d}"), format!("\u{f10d}{missing}a"), format!("a\u{f10d}{missing}")] {
                        let row: Vec<_> = text.chars().map(|c| crate::vt::Cell {
                            text: c.to_string(), underline, strike, ..Default::default()
                        }).collect();
                        let ops = super::row_ops(&row, gpui::size(gpui::px(8.0), gpui::px(16.0)), window);
                        let rendered: String = ops.iter().filter_map(|op| match op {
                            super::RowOp::Text { line, .. } => Some(line.text.as_ref()),
                            _ => None,
                        }).collect();
                        assert!(rendered.contains(missing), "{text:?}, underline={underline}, strike={strike}");
                        assert!(ops.iter().any(|op| matches!(op, super::RowOp::Icon { .. })));
                        let positions: Vec<_> = ops.iter().filter_map(|op| match op {
                            super::RowOp::Text { x, line } => { assert!(!line.text.is_empty()); Some(*x) }
                            super::RowOp::Icon { x, .. } => Some(*x),
                            _ => None,
                        }).collect();
                        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
                    }
                }
            }
        }).unwrap();
    }

    #[gpui::test]
    fn private_use_rows_always_advance(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, _| {
            for code in (0xe000..=0xf8ff).chain(0xf0000..=0xf0fff) {
                let text = char::from_u32(code).unwrap().to_string();
                for decorated in [false, true] {
                    let row = [crate::vt::Cell { text: text.clone(), underline: decorated, strike: decorated, ..Default::default() }];
                    let ops = super::row_ops(&row, gpui::size(gpui::px(8.0), gpui::px(16.0)), window);
                    assert_eq!(ops.len(), 1, "U+{code:X}");
                    match &ops[0] {
                        super::RowOp::Text { x, line } => { assert_eq!(*x, 0); assert_eq!(line.text.as_ref(), text); }
                        super::RowOp::Icon { x, .. } => assert_eq!(*x, 0),
                        super::RowOp::Sprite { x, text: rendered, .. } => { assert_eq!(*x, 0); assert_eq!(*rendered, text); }
                        _ => panic!("PUA cell was not rendered: U+{code:X}"),
                    }
                }
            }
        }).unwrap();
    }

    #[test]
    fn clipboard_tiff_and_bmp_become_png() {
        use super::{paste_image_bytes, ImageFormat};
        let original = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 3, image::Rgba([12, 34, 56, 255])));
        for (source, format) in [(image::ImageFormat::Tiff, ImageFormat::Tiff), (image::ImageFormat::Bmp, ImageFormat::Bmp)] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            original.write_to(&mut encoded, source).unwrap();
            let (format, bytes) = paste_image_bytes(format, encoded.get_ref()).unwrap();
            assert_eq!(format, ImageFormat::Png);
            assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::Png);
            assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8(), original.to_rgba8());
        }
        assert!(paste_image_bytes(ImageFormat::Tiff, b"broken").is_none());
        let (_, bytes) = paste_image_bytes(ImageFormat::Jpeg, b"unchanged").unwrap();
        assert!(matches!(bytes, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*bytes, b"unchanged");
    }

    #[test]
    fn a_titles_spinner_is_not_part_of_it() {
        use super::shown_title;
        assert_eq!(shown_title("\u{2733} Fix the tests"), "Fix the tests", "Claude Code's");
        assert_eq!(shown_title("\u{280b} working"), "working", "braille");
        assert_eq!(shown_title("\u{25d0} \u{2733}  two of them"), "two of them");
        assert_eq!(shown_title("~/Projects/herdr"), "~/Projects/herdr", "a folder keeps its ~");
        assert_eq!(shown_title("\u{03c0} - pi"), "\u{03c0} - pi", "a letter is no spinner");
        assert_eq!(shown_title("\u{2733}"), "\u{2733}", "nothing after it: kept");
        assert_eq!(shown_title("\u{2713} build"), "\u{2713} build", "a status icon stays");
        assert_eq!(shown_title("\u{2717} build"), "\u{2717} build");
    }

    use super::key_bytes;
    use crate::vt::Vt;
    use gpui::{KeyDownEvent, Keystroke, Modifiers};

    #[test]
    fn touchpad_moves_add_up_to_whole_lines() {
        use gpui::TouchPhase::*;
        let mut s = 0.0;
        assert_eq!(super::scroll_steps(&mut s, Started, 0.0, 40.0), 0);
        let total: i32 = (0..12).map(|_| super::scroll_steps(&mut s, Moved, 0.25, 40.0)).sum();
        assert_eq!(total, 3, "twelve quarter lines: not twelve lines");
        assert_eq!(super::scroll_steps(&mut s, Started, 0.0, 40.0), 0);
        assert_eq!(super::scroll_steps(&mut s, Moved, -0.5, 40.0), 0, "a new gesture forgets the last one's leftover");
        assert_eq!(super::scroll_steps(&mut s, Moved, -0.6, 40.0), -1);
    }

    #[test]
    fn horizontal_and_vertical_scroll_report_sgr_wheel_buttons() {
        use gpui::{point, px, size, ScrollDelta, TouchPhase};
        let mut vt = Vt::new(80, 24);
        vt.write(b"\x1b[?1000h\x1b[?1006h");
        let cell = size(px(10.0), px(20.0));
        let mut scrolled = point(0.0, 0.0);
        for (delta, expected) in [
            (point(10.0, 0.0), b"\x1b[<66;2;2M".as_slice()),
            (point(-10.0, 0.0), b"\x1b[<67;2;2M".as_slice()),
            (point(0.0, 20.0), b"\x1b[<64;2;2M".as_slice()),
            (point(0.0, -20.0), b"\x1b[<65;2;2M".as_slice()),
            (point(-10.0, 20.0), b"\x1b[<64;2;2M\x1b[<67;2;2M".as_slice()),
        ] {
            let steps = super::scroll_delta_steps(&mut scrolled, ScrollDelta::Pixels(point(px(delta.x), px(delta.y))), TouchPhase::Moved, cell, 80, 24);
            assert_eq!(super::scroll_mouse_bytes(&mut vt, steps, 15.0, 25.0, (80, 24, 10.0, 20.0)), expected);
        }
        let steps = super::scroll_delta_steps(&mut scrolled, ScrollDelta::Lines(point(1.0, -1.0)), TouchPhase::Moved, cell, 80, 24);
        let count = if cfg!(target_os = "macos") { 3 } else { 1 };
        assert_eq!(steps, point(1, -count));
        assert_eq!(super::scroll_mouse_bytes(&mut vt, steps, 15.0, 25.0, (80, 24, 10.0, 20.0)), [b"\x1b[<65;2;2M".repeat(count as usize), b"\x1b[<66;2;2M".to_vec()].concat());
    }

    #[test]
    fn scroll_axes_accumulate_independently_and_reset_together() {
        use gpui::{point, px, size, ScrollDelta, TouchPhase::*};
        let mut scrolled = point(0.0, 0.0);
        let mut steps = |phase, x, y| super::scroll_delta_steps(&mut scrolled, ScrollDelta::Pixels(point(px(x), px(y))), phase, size(px(10.0), px(20.0)), 80, 24);
        assert_eq!(steps(Moved, 6.0, 8.0), point(0, 0));
        assert_eq!(steps(Moved, 5.0, 8.0), point(1, 0));
        assert_eq!(steps(Moved, 0.0, 8.0), point(0, 1));
        assert_eq!(steps(Started, 0.0, 0.0), point(0, 0));
        assert_eq!(steps(Moved, -9.0, -19.0), point(0, 0));
        assert_eq!(steps(Moved, -2.0, -2.0), point(-1, -1));
        assert_eq!(steps(Ended, 100.0, 100.0), point(0, 0));
    }

    #[test]
    fn macos_wheel_events_scroll_as_in_ghostty() {
        use super::macos_wheel_steps as steps;
        assert_eq!(steps(0.1), 3, "a slow notch");
        assert_eq!(steps(-0.1), -3);
        assert_eq!(steps(1.5), 4, "a fast spin's tick, its half line dropped");
        assert_eq!(steps(0.0), 0);
    }

    #[test]
    #[cfg(not(windows))]
    fn dropped_paths_are_typed_as_the_shell_reads_them() {
        let q = |v: &[&str]| super::quote_paths(v.iter().map(|s| s.to_string()));
        assert_eq!(q(&["/tmp/shot.png"]), "/tmp/shot.png ");
        assert_eq!(q(&["/tmp/my shot.png", "/a/b"]), "'/tmp/my shot.png' /a/b ");
        assert_eq!(q(&["/tmp/it's.png"]), "'/tmp/it'\\''s.png' ");
    }

    #[test]
    fn copied_files_paste_as_files_not_their_bare_path_text() {
        use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths};
        let shot = std::path::PathBuf::from("/Users/me/Library/Application Support/clip/file-1.png");
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(smallvec::smallvec![shot.clone()])),
                ClipboardItem::new_string(shot.to_string_lossy().into_owned()).entries.remove(0),
            ],
        };
        assert_eq!(super::copied_files(&item), Some(vec![shot]));
        assert_eq!(super::copied_files(&ClipboardItem::new_string("hello".into())), None);
    }

    /// A key as the OS hands it over: "shift-;" with what it typed (":"), as
    /// GPUI reports it on Linux and macOS.
    fn press(spec: &str, typed: Option<&str>) -> KeyDownEvent {
        let mut parts: Vec<&str> = spec.split('-').collect();
        let key = parts.pop().unwrap_or_default();
        let key = if key.is_empty() { "-" } else { key };
        let has = |m: &str| parts.contains(&m);
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers { control: has("ctrl"), alt: has("alt"), shift: has("shift"), platform: has("cmd"), function: false },
                key: key.into(),
                key_char: typed.map(str::to_owned),
                #[cfg(target_os = "macos")]
                unshifted_key: None,
                #[cfg(target_os = "macos")]
                option_side: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn ctrl_v_clipboard_text_follows_platform_paste_convention() {
        let mut vt = Vt::new(20, 3);
        assert_eq!(super::paste_image_fallback(&mut vt, None).unwrap(), b"\x16");
        for bracketed in [false, true] {
            vt.write(if bracketed { b"\x1b[?2004h" } else { b"\x1b[?2004l" });
            let bytes = super::paste_image_fallback(&mut vt, Some("paste-probe")).unwrap();
            let expected = if cfg!(windows) { vt.paste_bytes("paste-probe") } else { vec![0x16] };
            assert_eq!(bytes, expected);
        }
    }

    #[cfg(target_os = "macos")]
    static OPTION_AS_ALT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    #[cfg(target_os = "macos")]
    fn option_as_alt_encodes_legacy_and_kitty_shortcuts() {
        let _guard = OPTION_AS_ALT_TEST_LOCK.lock().unwrap();
        gpui_macos::set_option_as_alt(Some(gpui_macos::OptionAsAlt::Both));
        let mut vt = Vt::new(20, 3);
        for (key, text, bytes) in [("alt-p", "p", b"\x1bp"), ("alt-e", "e", b"\x1be"), ("alt-b", "b", b"\x1bb")] {
            assert_eq!(key_bytes(&mut vt, &press(key, Some(text))).unwrap(), bytes);
        }
        vt.write(b"\x1b[>1u");
        assert_eq!(key_bytes(&mut vt, &press("alt-e", Some("e"))).unwrap(), b"\x1b[101;3u");
        vt.write(b"\x1b[<u");
        gpui_macos::set_option_as_alt(Some(gpui_macos::OptionAsAlt::None));
        assert_eq!(key_bytes(&mut vt, &press("alt-p", Some("π"))).unwrap(), "π".as_bytes());
        for setup in [b"".as_slice(), b"\x1b[>1u"] {
            let mut vt = Vt::new(20, 3);
            vt.write(setup);
            for (key, text) in [
                ("alt-shift-2", "@"), ("alt-l", "@"), ("alt-5", "["),
                ("alt-6", "]"), ("alt-7", "|"), ("alt-8", "{"),
                ("alt-9", "}"), ("alt-shift-7", "\\"), ("alt-n", "~"),
            ] {
                assert_eq!(key_bytes(&mut vt, &press(key, Some(text))).unwrap(), text.as_bytes(), "{key}, mode={setup:?}");
            }
            assert_eq!(key_bytes(&mut vt, &press("alt-left", None)).unwrap(), b"\x1b[1;3D");
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn option_sides_encode_shortcuts_and_native_text() {
        use gpui::OptionSide;
        use gpui_macos::OptionAsAlt;
        let _guard = OPTION_AS_ALT_TEST_LOCK.lock().unwrap();
        for setting in [OptionAsAlt::Left, OptionAsAlt::Right] {
            gpui_macos::set_option_as_alt(Some(setting));
            for side in [OptionSide::Left, OptionSide::Right, OptionSide::Both] {
                let as_alt = side == OptionSide::Both || matches!((setting, side), (OptionAsAlt::Left, OptionSide::Left) | (OptionAsAlt::Right, OptionSide::Right));
                for kitty in [false, true] {
                    let mut vt = Vt::new(20, 3);
                    if kitty { vt.write(b"\x1b[>1u"); }
                    for native in ["π", "@"] {
                        let mut event = press("alt-p", Some(if as_alt { "p" } else { native }));
                        event.keystroke.option_side = Some(side);
                        let expected = if as_alt {
                            if kitty { b"\x1b[112;3u".as_slice() } else { b"\x1bp".as_slice() }
                        } else { native.as_bytes() };
                        assert_eq!(key_bytes(&mut vt, &event).unwrap(), expected);
                        // Shortcut-prefix replay must retain the original Option side.
                        let mut replay = event.clone();
                        replay.prefer_character_input = true;
                        assert_eq!(key_bytes(&mut vt, &replay).unwrap(), expected);
                    }
                    let mut arrow = press("alt-left", None);
                    arrow.keystroke.option_side = Some(side);
                    assert_eq!(key_bytes(&mut vt, &arrow).unwrap(), b"\x1b[1;3D");
                }
            }
        }
        gpui_macos::set_option_as_alt(Some(OptionAsAlt::None));
    }

    #[test]
    fn replayed_shortcut_prefixes_keep_terminal_encoding() {
        let modes: [(&[u8], &[u8], &[u8]); 6] = [
            (b"", b"\x0b", b"a"),
            (b"\x1b[>1u", b"\x1b[107;5u", b"a"),
            (b"\x1b[>9u", b"\x1b[107;5u", b"\x1b[97u"),
            (b"\x1b[?9001h", b"\x0b", b"a"),
            (b"\x1b[?9001h\x1b[>1u", b"\x1b[107;5u", b"a"),
            (b"\x1b[?9001h\x1b[>9u", b"\x1b[107;5u", b"\x1b[97u"),
        ];
        for (setup, control_k, letter_a) in modes {
            let mut vt = Vt::new(20, 3);
            vt.write(setup);
            // replay_pending_input clones the original keystroke and sets
            // prefer_character_input, even for non-character shortcuts.
            for (spec, text, expected) in [
                ("ctrl-k", None, control_k),
                ("ctrl-k", Some("k"), control_k),
                ("a", Some("a"), letter_a),
            ] {
                let mut replay = press(spec, text);
                replay.prefer_character_input = true;
                assert_eq!(key_bytes(&mut vt, &replay).as_deref(), Some(expected), "{spec}");
            }
        }
    }

    /// Every row is what Ghostty sends for that key in that mode. A change in
    /// how we describe keys to libghostty shows up here, not in someone's
    /// editor.
    #[test]
    fn keys_send_what_ghostty_sends() {
        #[cfg(target_os = "macos")]
        let _guard = OPTION_AS_ALT_TEST_LOCK.lock().unwrap();
        #[cfg(target_os = "macos")]
        gpui_macos::set_option_as_alt(Some(gpui_macos::OptionAsAlt::None));
        // Herdr keeps native Option text on macOS (libghostty's default
        // option-as-alt=false); other platforms prefix an unconsumed Alt.
        const ALT_B: &[u8] = if cfg!(target_os = "macos") { b"b" } else { b"\x1bb" };
        const KITTY_ALT_B: &[u8] = if cfg!(target_os = "macos") { b"b" } else { b"\x1b[98;3u" };
        const KEYS: &[(&str, Option<&str>, &[u8], &[u8], &[u8])] = &[
            // key, typed, plain, kitty disambiguate (>1u, Neovim), kitty report-all (>9u)
            ("a", Some("a"), b"a", b"a", b"\x1b[97u"),
            ("shift-a", Some("A"), b"A", b"A", b"\x1b[97;2u"),
            ("shift-;", Some(":"), b":", b":", b"\x1b[59;2u"),
            ("shift-:", Some(":"), b":", b":", b"\x1b[59;2u"),
            ("shift-1", Some("!"), b"!", b"!", b"\x1b[49;2u"),
            ("space", Some(" "), b" ", b" ", b"\x1b[32u"),
            ("ctrl-c", None, b"\x03", b"\x1b[99;5u", b"\x1b[99;5u"),
            ("ctrl-space", None, b"\x00", b"\x1b[32;5u", b"\x1b[32;5u"),
            ("alt-b", Some("b"), ALT_B, KITTY_ALT_B, b"\x1b[98;3u"),
            ("alt-b", Some("∫"), "∫".as_bytes(), "∫".as_bytes(), b"\x1b[98;3u"),
            ("alt-s", Some("\u{df}"), "\u{df}".as_bytes(), "\u{df}".as_bytes(), b"\x1b[115;3u"),
            ("escape", None, b"\x1b", b"\x1b[27u", b"\x1b[27u"),
            ("enter", None, b"\r", b"\r", b"\x1b[13u"),
            ("shift-enter", None, b"\x1b[27;2;13~", b"\x1b[13;2u", b"\x1b[13;2u"),
            ("tab", None, b"\t", b"\t", b"\x1b[9u"),
            ("shift-tab", None, b"\x1b[Z", b"\x1b[9;2u", b"\x1b[9;2u"),
            ("backspace", None, b"\x7f", b"\x7f", b"\x1b[127u"),
            ("up", None, b"\x1b[A", b"\x1b[A", b"\x1b[A"),
            ("ctrl-left", None, b"\x1b[1;5D", b"\x1b[1;5D", b"\x1b[1;5D"),
            ("f1", None, b"\x1bOP", b"\x1b[P", b"\x1b[P"),
            ("f5", None, b"\x1b[15~", b"\x1b[15~", b"\x1b[15~"),
        ];
        let modes: [(&str, &[u8]); 3] = [("plain", b""), ("kitty >1u", b"\x1b[>1u"), ("kitty >9u", b"\x1b[>9u")];
        let mut wrong = Vec::new();
        for (m, (name, setup)) in modes.iter().enumerate() {
            for (spec, typed, plain, dis, all) in KEYS {
                let mut vt = Vt::new(20, 3);
                vt.write(setup);
                let want: &[u8] = [*plain, *dis, *all][m];
                let got = key_bytes(&mut vt, &press(spec, *typed)).unwrap_or_default();
                if got != want {
                    wrong.push(format!("{name:10} {spec:12} sent {:?}, Ghostty sends {:?}", String::from_utf8_lossy(&got), String::from_utf8_lossy(want)));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    /// Without Kitty, Windows' ConPTY drops Shift+Enter as Ghostty sends it;
    /// a win32-input-mode request (even split across reads) sends modified
    /// Enter as a native key event instead.
    #[test]
    fn enter_with_a_modifier_goes_as_a_windows_key_event_when_the_program_asks() {
        let mut vt = Vt::new(20, 3);
        let send = |vt: &mut Vt, spec| String::from_utf8(key_bytes(vt, &press(spec, None)).unwrap()).unwrap();
        assert_eq!(send(&mut vt, "shift-enter"), "\x1b[27;2;13~");
        vt.write(b"\x1b[?90");
        vt.write(b"01h");
        assert_eq!(send(&mut vt, "shift-enter"), "\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_");
        assert_eq!(send(&mut vt, "ctrl-enter"), "\x1b[13;28;10;1;8;1_\x1b[13;28;10;0;8;1_");
        assert_eq!(send(&mut vt, "enter"), "\r");
        vt.write(b"\x1b[?9001l");
        assert_eq!(send(&mut vt, "shift-enter"), "\x1b[27;2;13~");
    }

    #[test]
    fn escape_keeps_the_next_key_separate_in_each_keyboard_mode() {
        for flags in [0, 1, 9] {
            for win32 in [false, true] {
                let mut vt = Vt::new(20, 3);
                if win32 { vt.write(b"\x1b[?9001h"); }
                vt.write(format!("\x1b[>{flags}u").as_bytes());
                let escape: &[u8] = if flags != 0 {
                    b"\x1b[27u".as_slice()
                } else if win32 {
                    b"\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_"
                } else {
                    b"\x1b"
                };
                for (next, text, expected) in [
                    ("ctrl-c", None, if flags == 0 { b"\x03".as_slice() } else { b"\x1b[99;5u" }),
                    ("ctrl-c", Some("c"), if flags == 0 { b"\x03".as_slice() } else { b"\x1b[99;5u" }),
                    ("a", Some("a"), if flags & 8 == 0 { b"a".as_slice() } else { b"\x1b[97u" }),
                    ("space", Some(" "), if flags & 8 == 0 { b" ".as_slice() } else { b"\x1b[32u" }),
                    ("escape", None, escape),
                ] {
                    assert_eq!(key_bytes(&mut vt, &press("escape", None)).as_deref(), Some(escape), "flags={flags}, win32={win32}");
                    assert_eq!(key_bytes(&mut vt, &press(next, text)).as_deref(), Some(expected), "flags={flags}, win32={win32}, {next}");
                }
                vt.write(b"\x1b[<u");
                let restored = if win32 { b"\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_".as_slice() } else { b"\x1b" };
                assert_eq!(key_bytes(&mut vt, &press("escape", None)).as_deref(), Some(restored));
                vt.write(b"\x1b[?9001l");
                assert_eq!(key_bytes(&mut vt, &press("escape", None)).unwrap(), b"\x1b");
            }
        }
        let mut vt = Vt::new(20, 3);
        vt.write(b"\x1b[?9001h");
        for (spec, held) in [("shift-escape", 16), ("ctrl-escape", 8), ("alt-escape", 2)] {
            let expected = format!("\x1b[27;1;27;1;{held};1_\x1b[27;1;27;0;{held};1_");
            assert_eq!(key_bytes(&mut vt, &press(spec, None)).unwrap(), expected.as_bytes());
        }
    }

    #[test]
    fn kitty_input_takes_precedence_over_win32() {
        for flags in 1..=31 {
            for win32_first in [false, true] {
                let mut vt = Vt::new(20, 3);
                if win32_first { vt.write(b"\x1b[?9001h"); }
                vt.write(format!("\x1b[>{flags}u").as_bytes());
                if !win32_first { vt.write(b"\x1b[?9001h"); }
                for (spec, expected) in [
                    ("escape", b"\x1b[27u".as_slice()),
                    ("shift-enter", b"\x1b[13;2u".as_slice()),
                    ("ctrl-enter", b"\x1b[13;5u"),
                    ("alt-enter", b"\x1b[13;3u"),
                ] {
                    assert_eq!(key_bytes(&mut vt, &press(spec, None)).as_deref(), Some(expected), "flags={flags}, win32_first={win32_first}, {spec}");
                }
            }
        }
    }

    #[test]
    fn kitty_input_tracks_terminal_mode_changes() {
        let mut vt = Vt::new(20, 3);
        vt.write(b"\x1b[?9001h");
        let win32 = b"\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_".as_slice();
        let kitty = b"\x1b[13;2u".as_slice();
        for (output, expected) in [
            (b"\x1b[>5u".as_slice(), kitty),
            (b"\x1b[>0u", win32),
            (b"\x1b[<u", kitty),
            (b"\x1b[<u", win32),
            (b"\x1b[=0;1u\x1b[=5;1u\x1b[?9001h", kitty),
            (b"\x1b[?1049h", win32),
            (b"\x1b[?1049l", kitty),
            (b"\x1b[=0;1u", win32),
        ] {
            vt.write(output);
            assert_eq!(key_bytes(&mut vt, &press("shift-enter", None)).as_deref(), Some(expected), "output={output:?}");
        }
    }

    #[test]
    fn programs_are_told_the_cell_in_device_pixels() {
        let cell = gpui::size(gpui::px(8.4), gpui::px(21.5));
        assert_eq!(super::device_cell(cell, 1.0), (8, 22));
        assert_eq!(super::device_cell(cell, 2.0), (17, 43));
        assert_eq!(super::device_cell(gpui::size(gpui::px(0.0), gpui::px(0.0)), 2.0), (1, 1));
        assert_eq!(super::display_update(&mut None, &crate::vt::theme(), (17, 43)).unwrap()["cell"], serde_json::json!([17, 43]));
    }

    #[test]
    fn theme_changes_update_core_facts_without_resizing() {
        use crate::vt::{theme, Rgb};
        let mut theme = theme();
        let cell = (9, 18);
        let mut told = None;
        let first = super::display_update(&mut told, &theme, cell).unwrap();
        assert_eq!(first["cell"], serde_json::json!([9, 18]));
        assert!(super::display_update(&mut told, &theme, cell).is_none());

        for background in [Rgb(255, 255, 255), Rgb(23, 25, 29)] {
            theme.bg = background;
            let update = super::display_update(&mut told, &theme, cell)
                .expect("a theme-only change must reach the core");
            assert_eq!(update["bg"], format!("#{:02x}{:02x}{:02x}", background.0, background.1, background.2));
            assert_eq!(update["cell"], first["cell"]);
            assert!(super::display_update(&mut told, &theme, cell).is_none());
        }

        theme.fg = Rgb(1, 2, 3);
        assert_eq!(super::display_update(&mut told, &theme, cell).unwrap()["fg"], "#010203");
        theme.cursor = Rgb(4, 5, 6);
        assert_eq!(super::display_update(&mut told, &theme, cell).unwrap()["cursor"], "#040506");
        assert_eq!(super::display_update(&mut told, &theme, (17, 43)).unwrap()["cell"], serde_json::json!([17, 43]));
        theme.selection_bg = Rgb(7, 8, 9);
        assert!(super::display_update(&mut told, &theme, (17, 43)).is_none(), "local selection colors are not core display facts");
    }
}

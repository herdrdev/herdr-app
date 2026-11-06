//! A pane showing one core terminal: bytes in from the core, frames drawn
//! with GPUI, keys and mouse encoded by libghostty-vt and sent back.

use crate::hc::{Core, Stream};
use crate::vt::{self, CursorShape, Rgb, Select, Vt};
use ghostty_sys::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::{Input, InputEditorStyle, InputEvent, InputState};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use crate::links;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

fn pad() -> (f32, f32) {
    let p = crate::settings::get().terminal_padding;
    let zoom = crate::settings::zoom();
    (p[0] * zoom, p[1] * zoom)
}

pub(crate) fn grid_size(bounds: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let (px_, py_) = pad();
    let cols = ((f32::from(bounds.width) - 2.0 * px_) / f32::from(cell.width)).floor().max(2.0) as u16;
    let rows = ((f32::from(bounds.height) - 2.0 * py_) / f32::from(cell.height)).floor().max(1.0) as u16;
    (cols, rows)
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
    /// The scroll thumb's grabbable lane from the last paint, in window
    /// coordinates (wider than the few px drawn).
    thumb: Option<Bounds<Pixels>>,
    track: Option<Bounds<Pixels>>,
    /// Each row's drawing by position, kept while the row is the same (`Rc`).
    painted: Vec<(Rc<Vec<vt::Cell>>, Vec<RowOp>, Option<usize>)>,
    /// Font size, cell size, font, loaded fonts and ligature setting used to draw.
    painted_with: (u32, Size<Pixels>, String, u64, bool),
}

pub struct TerminalView {
    pub address: String,
    pub title: String,
    pub ended: Option<String>,
    /// The core connection dropped; waiting to reattach.
    pub lost: bool,
    /// A left-button selection drag is in progress.
    selecting: bool,
    /// A scroll thumb drag is: the px into the thumb it was grabbed at.
    scrollbar_grab: Option<f32>,
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
    /// Search UI and query belong to this terminal view, not the window. Each
    /// terminal in a tab therefore keeps its own query and match position.
    search_open: bool,
    /// Restore the last keyboard target when returning to this pane.
    search_keys: bool,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    pending_search_nav: Option<bool>,
    search_task: Option<Task<()>>,
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

pub fn initial_display(window: &Window) -> serde_json::Value {
    display_update(&mut None, &vt::theme(), device_cell(cell_size(window), window.scale_factor())).unwrap()
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

    /// Sizes the screen to `cols` x `rows` of `cell` at `scale`; the program hears of a change.
    fn fit(&mut self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) {
        self.cell = cell;
        self.scale = scale;
        let dev = device_cell(cell, scale);
        // Before the size, so a program woken by it asks for the new cell.
        self.refresh_theme(&vt::theme(), dev);
        if (cols, rows) == (self.vt.cols, self.vt.rows) && dev == self.vt_cell {
            return;
        }
        self.vt.resize(cols, rows, dev.0, dev.1);
        self.vt_cell = dev;
        if let Some(chan) = self.channel {
            self.core.resize(chan, cols, rows);
            // Mode 2048 programs wait for this report, not SIGWINCH.
            let replies = self.vt.take_replies();
            self.core.automatic_input(chan, &replies);
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
            scrollbar_grab: None,
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
                thumb: None,
                track: None,
                painted: Vec::new(),
                painted_with: (0, Size::default(), String::new(), 0, false),
            })),
            focus: cx.focus_handle(),
            was_focused: false,
            shown_in: Cell::new(0),
            typed_at: Cell::new(None),
            scrolled: point(0.0, 0.0),
            search_open: false,
            search_keys: false,
            search_query: String::new(),
            search_input: None,
            pending_search_nav: None,
            search_task: None,
            _reader: None,
        };
        view.connect(core, cx);
        view
    }

    /// Attaches to the same terminal on a (new) core connection.
    pub fn reattach(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if matches!(self.ended.as_deref(), Some("exited" | "killed" | "finished")) {
            return;
        }
        self.ended = None;
        self.lost = false;
        self.connect(core, cx);
        cx.notify();
    }

    /// A kept terminal can restart without its core connection changing.
    pub fn reattach_if_needed(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        if self.lost || self.ended.is_some() { self.reattach(core, cx); }
    }

    /// Attaches off the UI thread: a terminal on a machine still dialing can
    /// take seconds, and the window must keep drawing meanwhile.
    fn connect(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        let (cols, rows, facts) = {
            let mut g = self.grid.borrow_mut();
            let (cols, rows) = (g.vt.cols, g.vt.rows);
            // The core repaints the whole screen (and history) on attach.
            let cell = device_cell(g.cell, g.scale);
            g.vt = Vt::new(cols, rows);
            g.vt.resize(cols, rows, cell.0, cell.1);
            if self.search_open {
                g.vt.start_search(&self.search_query);
            }
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
            let Ok(Some(rx)) = this.update(cx, |this, cx| this.attached(attached, (cols, rows), cx)) else {
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
                // A failed lookup or reconnect is not evidence the program exited.
                self.ended = Some(format!("{e:#}"));
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

    #[cfg(test)]
    pub(crate) fn dimensions(&self) -> (u16, u16) {
        let grid = self.grid.borrow();
        (grid.vt.cols, grid.vt.rows)
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The focused terminal remains the action target while its local find
    /// input owns keyboard focus (notably for search, copy and paste commands).
    pub fn owns_keyboard_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.is_focused(window) || self.find_field_focused(window, cx)
    }

    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        if self.search_open && self.search_keys
            && let Some(input) = &self.search_input
        {
            return input.read(cx).focus_handle(cx);
        }
        self.focus.clone()
    }

    fn find_field_focused(&self, window: &Window, cx: &App) -> bool {
        self.search_open
            && self.search_input.as_ref().is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
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
                            g.core.automatic_input(ch, &replies);
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

    /// Sizes a terminal that isn't on screen as its pane would be (`AppWindow::fit_hidden_tabs`).
    pub fn fit(&self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) {
        self.grid.borrow_mut().fit(cols, rows, cell, scale);
    }

    pub fn on_screen_this_frame(&self) -> bool {
        self.shown_in.get() == FRAME.load(Ordering::Relaxed)
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
    /// Returns whether anything was copied. Does not clear the selection: mouse
    /// copy-on-select uses this and keeps the highlight.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.grid.borrow().vt.selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        #[cfg(target_os = "linux")]
        cx.write_to_primary(ClipboardItem::new_string(text));
        true
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.grid.borrow_mut().vt.clear_selection();
        cx.notify();
    }

    /// Opens the find bar inside this terminal view. Keeping the input entity
    /// here means its query and selection survive tab switches independently.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reopening = !self.search_open;
        let input = if let Some(input) = self.search_input.clone() {
            input
        } else {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in terminal"));
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.search_query = input.read(cx).value().to_string();
                    this.pending_search_nav = None;
                    this.grid.borrow_mut().vt.set_search_needle(&this.search_query);
                    this.ensure_search_task(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => this.queue_search_navigation(*shift, cx),
                InputEvent::Focus => this.search_keys = true,
                _ => {}
            })
            .detach();
            self.search_input = Some(input.clone());
            input
        };
        // Re-applied on every open, so a theme change while the bar was closed
        // doesn't leave the field with the old colors.
        input.update(cx, |state, _| state.set_editor_style(find_input_style()));

        self.search_open = true;
        self.search_keys = true;
        self.search_query = input.read(cx).value().to_string();
        self.grid.borrow_mut().vt.start_search(&self.search_query);
        self.focus_search_input(window, cx);
        if reopening && !self.search_query.is_empty() {
            if let Some(input) = &self.search_input {
                input.update(cx, |state, cx| state.select_all(window, cx));
            }
        }
        self.ensure_search_task(cx);
        cx.notify();
    }

    fn focus_search_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.search_input {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self.pending_search_nav = None;
        self.search_task.take();
        self.grid.borrow_mut().vt.clear_search();
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn queue_search_navigation(&mut self, previous: bool, cx: &mut Context<Self>) {
        if !self.search_open || self.search_query.is_empty() {
            return;
        }
        if self.grid.borrow().vt.search_progress().complete {
            self.pending_search_nav = None;
            if self.grid.borrow_mut().vt.search_navigate(previous) {
                cx.notify();
            }
        } else {
            self.pending_search_nav = Some(previous);
            self.ensure_search_task(cx);
        }
    }

    fn ensure_search_task(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_query.is_empty() || self.search_task.is_some() {
            return;
        }
        self.search_task = Some(cx.spawn(async move |this, cx| loop {
            let delay = this.update(cx, |this, cx| {
                if !this.search_open {
                    return None;
                }
                if this.shown_in.get() != FRAME.load(Ordering::Relaxed) {
                    return Some(Duration::from_millis(250));
                }
                if this.search_query.is_empty() {
                    return Some(Duration::from_millis(200));
                }

                let mut grid = this.grid.borrow_mut();
                let mut changed = grid.vt.progress_search();
                if grid.vt.search_progress().complete
                    && let Some(previous) = this.pending_search_nav.take()
                {
                    changed |= grid.vt.search_navigate(previous);
                }
                let busy = !grid.vt.search_progress().complete || this.pending_search_nav.is_some();
                drop(grid);
                if changed {
                    cx.notify();
                }
                Some(if busy { Duration::from_millis(33) } else { Duration::from_millis(200) })
            }).unwrap_or(None);
            let Some(delay) = delay else { break };
            cx.background_executor().timer(delay).await;
        }));
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_field_focused(window, cx) && ev.keystroke.key == "escape" {
            self.close_search(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.find_field_focused(window, cx) {
            // The input entity owns text editing and Enter navigation; never
            // leak its keystrokes into the shell running in this terminal.
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

    pub fn scroll_history(&mut self, target: ScrollTarget, cx: &mut Context<Self>) {
        let mut g = self.grid.borrow_mut();
        let rows = g.vt.rows;

        match target {
            ScrollTarget::Lines(lines) => g.vt.scroll_viewport(lines),
            ScrollTarget::Pages(pages) => g.vt.scroll_viewport(pages_to_lines(pages, rows)),
            ScrollTarget::Top => g.vt.scroll_to_top(),
            ScrollTarget::Bottom => g.vt.scroll_to_bottom(),
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

#[derive(Debug, PartialEq)]
pub enum ScrollTarget {
    Lines(isize),
    Pages(f64),
    Top,
    Bottom,
}

pub fn scroll_target(args: &Value) -> Result<ScrollTarget, String> {
    let given: Vec<&str> = ["lines", "pages", "to"].into_iter().filter(|key| is_given(args, key)).collect();
    match given.as_slice() {
        [] => Err("terminal.scroll needs one of lines, pages, to".into()),
        ["lines"] => whole_lines_arg(args).map(ScrollTarget::Lines),
        ["pages"] => pages_arg(args).map(ScrollTarget::Pages),
        ["to"] => edge_arg(args),
        _ => Err(format!("terminal.scroll takes one of lines, pages, to, not {}", given.join(" and "))),
    }
}

fn is_given(args: &Value, key: &str) -> bool {
    args.get(key).is_some_and(|value| !value.is_null())
}

fn whole_lines_arg(args: &Value) -> Result<isize, String> {
    let value = &args["lines"];
    value.as_i64().and_then(|lines| isize::try_from(lines).ok()).ok_or_else(|| format!("terminal.scroll lines is not a whole number: {value}"))
}

fn pages_arg(args: &Value) -> Result<f64, String> {
    let value = &args["pages"];
    value.as_f64().filter(|pages| pages.is_finite()).ok_or_else(|| format!("terminal.scroll pages is not a finite number: {value}"))
}

fn edge_arg(args: &Value) -> Result<ScrollTarget, String> {
    let value = &args["to"];
    match value.as_str() {
        Some("top") => Ok(ScrollTarget::Top),
        Some("bottom") => Ok(ScrollTarget::Bottom),
        _ => Err(format!("terminal.scroll to is \"top\" or \"bottom\", not {value}")),
    }
}

fn pages_to_lines(pages: f64, rows: u16) -> isize {
    (pages * f64::from(rows)).round() as isize
}

fn selection_scroll_lines(y: f32, cell_height: f32, rows: u16) -> isize {
    let height = cell_height * rows as f32;
    let outside = if y < 0.0 { y } else if y >= height { y - height + 1.0 } else { return 0 };
    (outside / cell_height).abs().ceil().clamp(1.0, 5.0) as isize * if outside < 0.0 { -1 } else { 1 }
}

/// A scroll thumb's top and length in px inside a `track` px lane: the
/// scrollable area is `total` rows, the `len`-row viewport sits `offset` rows
/// from its top. None when nothing scrolls; `min` keeps a huge scrollback
/// grabbable. The proportions are Herdr's scrollbar's.
fn scrollbar_thumb(total: u64, offset: u64, len: u64, track: f32, min: f32) -> Option<(f32, f32)> {
    if total <= len || len == 0 || track <= 0.0 {
        return None;
    }
    let thumb = (track * len as f32 / total as f32).clamp(min.min(track), track);
    let travel = track - thumb;
    let top = if travel <= 0.0 { 0.0 } else { offset.min(total - len) as f32 / (total - len) as f32 * travel };
    Some((top, thumb))
}

// Use existing padding when possible, otherwise overlay the last few pixels.
// Scrollback appearing must not resize the terminal or reflow its output.
fn scrollbar_lane(bounds: Bounds<Pixels>, origin: Point<Pixels>, text_width: Pixels, height: Pixels) -> Bounds<Pixels> {
    let edge = bounds.origin.x + bounds.size.width;
    let left = (origin.x + text_width).min(edge - px(8.0)).max(bounds.origin.x);
    Bounds::new(point(left, origin.y), size(edge - left, height))
}

fn scrollbar_grab_at(top: f32, len: f32, y: f32) -> f32 {
    if y >= top && y < top + len { y - top } else { len / 2.0 }
}

/// The scroll position a thumb dragged to `top` px lands on, as a row offset
/// from the scrollback's top (`Vt::scroll_to_row`'s).
fn scrollbar_row_at(total: u64, len: u64, track: f32, min: f32, top: f32) -> usize {
    let Some((_, thumb)) = scrollbar_thumb(total, 0, len, track, min) else { return 0 };
    let travel = track - thumb;
    if travel <= 0.0 {
        return 0;
    }
    (top.clamp(0.0, travel) / travel * (total - len) as f32).round() as usize
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

/// A Nerd Font icon (Unicode private use areas).
fn is_icon(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFD))
}

/// Keep the configured font’s icon proportions; only fit bundled fallback glyphs.
fn fallback_icon(text: &str, font: &Font, ts: &gpui::TextSystem) -> Option<IconGlyph> {
    let c = text.chars().next().filter(|_| is_icon(text))?;
    let id = ts.resolve_font(font);
    if ts.advance(id, px(crate::settings::font_size()), c).is_ok() {
        return None;
    }
    icon_glyph(c)
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
    // libghostty leaves these aliases to fixterms, but GPUI drops their
    // control text. Legacy needs explicit aliases; negotiated modes keep the chord.
    if !vt.win32.on && vt.kitty_keyboard_flags() == 0 && m.control && !m.alt && !m.platform && !m.function {
        let alias = match k.key.as_str() {
            "[" if !m.shift => Some((GHOSTTY_KEY_BRACKET_LEFT, GHOSTTY_KEY_ESCAPE)),
            "i" if !m.shift => Some((GHOSTTY_KEY_I, GHOSTTY_KEY_TAB)),
            "m" if !m.shift => Some((GHOSTTY_KEY_M, GHOSTTY_KEY_ENTER)),
            _ => None,
        };
        if let Some((key, alias)) = alias {
            let legacy = vt.encode_key(alias, 0, 0, None, None);
            return Some(control_alias_bytes(vt, key, &k.key, false, legacy));
        }
        if matches!(k.key.as_str(), "-" | "_") {
            let shifted = m.shift || k.key == "_";
            let text = if shifted { "_" } else { "-" };
            return Some(control_alias_bytes(vt, GHOSTTY_KEY_MINUS, text, shifted, vec![0x1f]));
        }
    }
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

fn control_alias_bytes(vt: &mut vt::Vt, key: GhosttyKey, text: &str, shifted: bool, legacy: Vec<u8>) -> Vec<u8> {
    // libghostty has no mode-2 getter. Probe with printable base text so its
    // encoder can preserve the chord when modifyOtherKeys requests CSI 27.
    let mods = GHOSTTY_MODS_CTRL as GhosttyMods | if shifted { GHOSTTY_MODS_SHIFT as GhosttyMods } else { 0 };
    let encoded = vt.encode_key(key, mods, 0, Some(text), text.chars().next());
    if encoded.starts_with(b"\x1b[27;") { encoded } else { legacy }
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

fn ligature_features(enabled: bool) -> FontFeatures {
    static FEATURES: std::sync::LazyLock<[FontFeatures; 2]> = std::sync::LazyLock::new(|| {
        [0, 1].map(|value| FontFeatures(Arc::new(vec![("calt".into(), value), ("liga".into(), value), ("dlig".into(), 0)])))
    });
    FEATURES[usize::from(enabled)].clone()
}

fn cell_font(bold: bool, italic: bool) -> Font {
    let mut f = font(crate::settings::terminal_font());
    f.features = ligature_features(false);
    f.weight = if bold { FontWeight::BOLD } else { FontWeight::MEDIUM };
    f.style = if italic { FontStyle::Italic } else { FontStyle::Normal };
    f
}

// JetBrains Mono joins punctuation, so ordinary ASCII output can keep using
// the ligatures-off font even when the setting is enabled.
fn needs_ligatures(text: &str, font: &Font) -> bool {
    font.family.as_ref() != crate::settings::DEFAULT_FONT
        || !text.is_ascii()
        || text.as_bytes().windows(2).any(|pair| pair[0].is_ascii_punctuation() && pair[1].is_ascii_punctuation())
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

// Force-width shaping moves glyph origins, but does not resize fallback glyphs.
fn fallback_size(c: &vt::Cell, font: &Font, width: Pixels, ts: &gpui::WindowTextSystem) -> Option<Pixels> {
    if c.wide || c.text.is_ascii() {
        return None;
    }
    let first = c.text.chars().next()?;
    let size = px(crate::settings::font_size());
    let primary = ts.resolve_font(font);
    if ts.advance(primary, size, first).is_ok() {
        return None;
    }
    let run = TextRun { len: c.text.len(), font: font.clone(), ..Default::default() };
    let line = ts.shape_line(c.text.clone().into(), size, &[run], None);
    (line.width() > width).then(|| size * f32::from(width) / f32::from(line.width()))
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
fn row_ops(row: &[vt::Cell], cell: Size<Pixels>, cursor: Option<usize>, window: &mut Window) -> Vec<RowOp> {
    crate::bench::count(&crate::bench::ROWS);
    let ligatures = crate::settings::get().font_ligatures;
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
        let mut text_font = cell_font(c.bold, c.italic);
        if let Some(icon) = fallback_icon(&c.text, &text_font, window.text_system()) {
            // Fit missing icons from the bundled font by their outlines:
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
        let fitted_size = fallback_size(c, &text_font, cell.width, window.text_system());
        let mut text = if c.text.is_empty() { " ".to_owned() } else { c.text.clone() };
        // Shape the cursor cell separately, so its plain glyph doesn't cover a ligature.
        if !c.wide && fitted_size.is_none() && cursor != Some(start) {
            while x < row.len() {
                let d = &row[x];
                let same = !d.wide
                    && (!ligatures || d.bg == c.bg)
                    && d.fg == c.fg
                    && d.bold == c.bold
                    && d.italic == c.italic
                    && d.faint == c.faint
                    && d.underline == c.underline
                    && d.strike == c.strike;
                // Bundled fallback icons are fitted separately from the text run.
                if !same || cursor == Some(x) || (d.text.is_empty() && !d.underline && !d.strike) || crate::sprites::is_sprite(&d.text) || fallback_icon(&d.text, &text_font, window.text_system()).is_some() || fallback_size(d, &text_font, cell.width, window.text_system()).is_some() {
                    break;
                }
                text.push_str(if d.text.is_empty() { " " } else { &d.text });
                x += 1;
            }
        }
        if text.trim().is_empty() && !c.underline && !c.strike {
            continue;
        }
        if ligatures && needs_ligatures(&text, &text_font) {
            text_font.features = ligature_features(true);
        }
        let run = TextRun {
            len: text.len(),
            font: text_font,
            color,
            background_color: None,
            underline: c.underline.then(|| UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
            strikethrough: c.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
        };
        let force = (!c.wide).then_some(cell.width);
        let line = window.text_system().shape_line(text.into(), fitted_size.unwrap_or_else(|| px(crate::settings::font_size())), &[run], force);
        ops.push(RowOp::Text { x: start, line });
    }
    ops
}

/// The find field's editor colors, following the theme at open time.
fn find_input_style() -> InputEditorStyle {
    InputEditorStyle {
        foreground: crate::settings::color("foreground"),
        muted_foreground: crate::settings::color("muted"),
        background: crate::settings::color("input"),
        border: crate::settings::color("border"),
        selection: crate::settings::color("selection"),
        caret: crate::settings::color("foreground"),
        ..InputEditorStyle::default()
    }
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
    let ligatures = crate::settings::get().font_ligatures;
    let cursor = frame.cursor_cell.filter(|_| ligatures);
    let with = (crate::settings::font_size().to_bits(), cell, crate::settings::terminal_font(), crate::settings::fonts_added(), ligatures);
    // When every row changed (output scrolling), nothing is kept: the next
    // frame would find none of them the same.
    let mut painted = std::mem::take(&mut grid.borrow_mut().painted);
    if grid.borrow().painted_with != with || frame.all {
        painted.clear();
    }
    painted.truncate(frame.rows.len());
    for (y, row) in frame.rows.iter().enumerate() {
        let column = cursor.filter(|(_, cy)| *cy as usize == y).map(|(x, _)| x as usize);
        if frame.all || painted.get(y).is_none_or(|(r, _, c)| !Rc::ptr_eq(r, row) || *c != column) {
            let ops = row_ops(row, cell, column, window);
            if y < painted.len() {
                painted[y] = (row.clone(), ops, column);
            } else {
                painted.push((row.clone(), ops, column));
            }
        }
    }

    // Paint all program backgrounds first, then search overlays, then text.
    // This lets a match highlight sit below glyphs without contaminating
    // cached RowOps or the terminal's own selection/copy state.
    for (y, (_, ops, _)) in painted.iter().enumerate() {
        for op in ops {
            match op {
                RowOp::Bg { from, to, color } => {
                    let b = Bounds::new(at(*from, y), size(cell.width * (to - from) as f32, cell.height));
                    window.paint_quad(fill(snap_box(b), *color));
                }
                _ => {}
            }
        }
    }
    for selected in [false, true] {
        for span in frame.search_spans.iter().filter(|span| span.selected == selected) {
            let y = span.row as usize;
            let Some(row) = frame.rows.get(y) else { continue };
            let start = span.start as usize;
            let end = (span.end as usize + 1).min(row.len());
            if start >= end {
                continue;
            }
            let opacity = if selected { 0.55 } else { 0.28 };
            let highlight = crate::settings::color("accent").opacity(opacity);
            let b = Bounds::new(at(start, y), size(cell.width * (end - start) as f32, cell.height));
            window.paint_quad(fill(snap_box(b), highlight));
        }
    }
    for (y, (_, ops, _)) in painted.iter().enumerate() {
        for op in ops {
            match op {
                RowOp::Bg { .. } => {}
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
    {
        let mut g = grid.borrow_mut();
        g.cursor_at = frame.cursor_cell.map(|(x, y)| Bounds::new(at(x as usize, y as usize), cell));
        // As in Herdr, hide on the alternate screen and below five columns.
        g.thumb = None;
        g.track = None;
        let sb = g.vt.scrollbar();
        let track = f32::from(cell.height) * f32::from(g.vt.rows);
        if crate::settings::get().pane_scrollbars
            && !g.vt.alt_screen()
            && g.vt.cols > 4
            && let Some((top, len)) = scrollbar_thumb(sb.total, sb.offset, sb.len, track, f32::from(cell.height))
        {
            let lane = scrollbar_lane(bounds, origin, cell.width * f32::from(g.vt.cols), px(track));
            let w = px(4.0);
            let x = lane.origin.x + lane.size.width - w - px(2.0);
            let color = crate::settings::color("muted");
            let rail = Bounds::new(point(x + w / 2.0 - px(0.5), origin.y), size(px(1.0), px(track)));
            window.paint_quad(fill(snap_box(rail), color.opacity(0.35)));
            let thumb = Bounds::new(point(x, origin.y + px(top)), size(w, px(len)));
            window.paint_quad(fill(snap_box(thumb), color).corner_radii(w / 2.0));
            g.thumb = Some(Bounds::new(point(lane.origin.x, thumb.origin.y), size(lane.size.width, thumb.size.height)));
            g.track = Some(lane);
        }
    }
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

    fn replace_text_in_range(&mut self, _range: Option<std::ops::Range<usize>>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        // While the find field holds the keys, the input-method path must not
        // be a way around that (see `key_down`); with the terminal focused,
        // input methods type into the program as usual.
        if !self.find_field_focused(window, cx) {
            self.send(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _selected: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Composition previews stay out of the terminal too while the find
        // field holds the keys; with the terminal focused they show as usual.
        if self.find_field_focused(window, cx) {
            return;
        }
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
        let terminal_focused = self.focus.is_focused(window);
        if terminal_focused {
            self.search_keys = false;
        }
        let search_focused = self.find_field_focused(window, cx);
        // The find input is part of this terminal's interaction, not a loss of
        // attention to the terminal itself. Keep core focus/cursor state, while
        // only installing the terminal's IME handler when its own focus handle
        // owns keyboard input.
        let focused = terminal_focused || search_focused;
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
        let search_input = self.search_open.then(|| self.search_input.clone()).flatten();
        let search_progress = self.grid.borrow().vt.search_progress();
        let search_label = if self.search_query.is_empty() {
            String::new()
        } else if !search_progress.complete {
            if search_progress.total == 0 { "Searching…".to_owned() } else { format!("{}+", search_progress.total) }
        } else if search_progress.total == 0 {
            "No matches".to_owned()
        } else if let Some(selected) = search_progress.selected {
            format!("{} / {}", selected + 1, search_progress.total)
        } else {
            format!("{} matches", search_progress.total)
        };
        let address = self.address.clone();

        div()
            .id(SharedString::from(format!("term-{}", self.address)))
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .relative()
            // Below 1, the layout's background shows through (cells a program colors stay solid).
            .bg(hsla(vt::theme().bg).opacity(crate::settings::get().terminal_opacity))
            .on_action(cx.listener(|this, a: &gpui_shell::action::ShellAction, window, cx| {
                if this.find_field_focused(window, cx) {
                    let action: Option<Box<dyn gpui::Action>> = match a.id() {
                        "terminal.copy" => Some(Box::new(gpui_base::input::Copy)),
                        "terminal.paste" => Some(Box::new(gpui_base::input::Paste)),
                        _ => None,
                    };
                    if let Some(action) = action {
                        window.dispatch_action(action, cx);
                        cx.stop_propagation();
                        return;
                    }
                }
                // `send:<text>` bindings from keys.json type into this terminal.
                match a.id().strip_prefix("send:") {
                    Some(_) if this.find_field_focused(window, cx) => cx.stop_propagation(),
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
                let mut g = this.grid.borrow_mut();
                if let (Some(track), Some(thumb)) = (g.track, g.thumb)
                    && track.contains(&ev.position)
                {
                    let grab = scrollbar_grab_at(f32::from(thumb.origin.y), f32::from(thumb.size.height), f32::from(ev.position.y));
                    this.scrollbar_grab = Some(grab);
                    if !thumb.contains(&ev.position) {
                        let sb = g.vt.scrollbar();
                        let top = f32::from(ev.position.y - track.origin.y) - grab;
                        let min = f32::from(g.cell.height);
                        g.vt.scroll_to_row(scrollbar_row_at(sb.total, sb.len, f32::from(track.size.height), min, top));
                    }
                    drop(g);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                drop(g);
                if this.mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(ev.button), ev.position, ev.modifiers) {
                    // The program took the click: Herdr's old selection ends, so a copy can't repeat it.
                    this.clear_selection(cx);
                } else {
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
                        let (cols, rows) = grid_size(bounds.size, cell);
                        grid.borrow_mut().fit(cols, rows, cell, window.scale_factor());
                    },
                    move |bounds, _, window, cx| {
                        paint_grid(bounds, &grid_paint, focused, window, cx);
                        let view = selection_view.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = view.update(cx, |this, cx| {
                                    if this.scrollbar_grab.is_some() {
                                        if ev.pressed_button != Some(MouseButton::Left) {
                                            this.scrollbar_grab = None;
                                            return;
                                        }
                                        let mut g = this.grid.borrow_mut();
                                        let sb = g.vt.scrollbar();
                                        let (track, min) = (f32::from(g.cell.height) * f32::from(g.vt.rows), f32::from(g.cell.height));
                                        let top = f32::from(ev.position.y - g.origin.y) - this.scrollbar_grab.unwrap_or(0.0);
                                        g.vt.scroll_to_row(scrollbar_row_at(sb.total, sb.len, track, min, top));
                                        drop(g);
                                        cx.notify();
                                        return;
                                    }
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
                                    if this.scrollbar_grab.is_some() {
                                        if ev.button == MouseButton::Left {
                                            this.scrollbar_grab = None;
                                            cx.stop_propagation();
                                        }
                                        return;
                                    }
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
                        if terminal_focused {
                            window.handle_input(&input_focus, ElementInputHandler::new(bounds, input_view), cx);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(search_input, |d, input| {
                d.child(
                    div()
                        .flex()
                        .id(SharedString::from(format!("find-bar-{address}")))
                        .debug_selector(|| "find-bar".to_owned())
                        .absolute()
                        .top_2()
                        .right_2()
                        .w_full()
                        .max_w(px(460.0))
                        .h(px(38.0))
                        .gap_1()
                        .items_center()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(crate::settings::color("border"))
                        .bg(crate::settings::color("popover"))
                        .shadow_md()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex_1().min_w(px(48.0)).h_full().child(Input::new(&input)))
                        .child(div().w(px(58.0)).text_xs().text_color(crate::settings::color("muted")).child(search_label))
                        .child(
                            div()
                                .id(SharedString::from(format!("find-next-{address}")))
                                .debug_selector(|| "find-up".to_owned())
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.queue_search_navigation(false, cx);
                                    this.focus_search_input(window, cx);
                                }))
                                .child("↑"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("find-prev-{address}")))
                                .debug_selector(|| "find-down".to_owned())
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.queue_search_navigation(true, cx);
                                    this.focus_search_input(window, cx);
                                }))
                                .child("↓"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("find-close-{address}")))
                                .w(px(26.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .hover(|d| d.bg(crate::settings::color("hover")))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx)))
                                .child("×"),
                        ),
                )
            })
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
    use super::{ScrollTarget, pages_to_lines, scroll_target};
    use serde_json::{Value, json};

    #[gpui::test]
    fn attach_errors_do_not_announce_a_terminal_exit(cx: &mut gpui::TestAppContext) {
        use gpui::*;
        let view = cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "remote/t1".into(), (40, 5), cx));
        let ends = cx.new(|cx| {
            cx.subscribe(&view, |count: &mut usize, _, event: &super::TerminalEvent, _| {
                if matches!(event, super::TerminalEvent::Ended) {
                    *count += 1;
                }
            }).detach();
            0usize
        });
        cx.run_until_parked();
        for kind in ["not_found", "unavailable", "permission_denied"] {
            view.update(cx, |view, cx| {
                let error = crate::hc::RpcError(json!({ "message": "attach failed", "data": { "kind": kind } }));
                view.attached(Err(error.into()), (40, 5), cx);
            });
            cx.run_until_parked();
            assert_eq!(ends.read_with(cx, |count, _| *count), 0, "{kind}");
        }
        view.update(cx, |view, cx| {
            let error = crate::hc::RpcError(json!({ "message": "program finished", "data": { "kind": "finished" } }));
            view.attached(Err(error.into()), (40, 5), cx);
        });
        cx.run_until_parked();
        assert_eq!(ends.read_with(cx, |count, _| *count), 1);
    }

    #[gpui::test]
    fn terminal_release_ends_window_text_selection(cx: &mut gpui::TestAppContext) {
        use gpui::*;
        use gpui_base::{TextSelectionLayer, TextView, TextViewState};

        struct Root {
            terminal: Entity<super::TerminalView>,
            text: Entity<TextViewState>,
        }
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().flex().flex_col().w(px(400.)).child(TextSelectionLayer)
                    .child(div().w_full().h(px(100.)).child(self.terminal.clone()))
                    // Separate mod roots install layers on both sides of the terminal.
                    .child(TextSelectionLayer)
                    .child(TextView::new(&self.text).scrollable(false).selectable(true))
                    .child(TextSelectionLayer)
            }
        }
        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root {
            terminal: cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 5), cx)),
            text: cx.new(|cx| TextViewState::markdown("Move the pointer over this selectable text.", cx)),
        });
        cx.run_until_parked();
        let terminal = point(px(50.), px(50.));
        let (start, end) = root.read_with(cx, |root, cx| {
            let bounds = root.text.read(cx).bounds();
            assert!(bounds.size.height > px(0.));
            (point(bounds.left() + px(2.), bounds.top() + px(10.)),
             point(bounds.left() + px(150.), bounds.top() + px(10.)))
        });
        for _ in 0..2 {
            cx.simulate_mouse_down(terminal, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(terminal, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_move(start, None, Modifiers::default());
            cx.simulate_mouse_move(end, None, Modifiers::default());
            assert!(root.read_with(cx, |root, cx| root.text.read(cx).selected_text()).is_empty());

            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            assert!(!root.read_with(cx, |root, cx| root.text.read(cx).selected_text()).is_empty());
        }
    }

    #[gpui::test]
    fn each_terminal_keeps_its_find_bar_inside_its_own_pane(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root {
            first: Entity<super::TerminalView>,
            second: Entity<super::TerminalView>,
        }
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().flex().size_full()
                    .child(div().w(px(320.0)).h(px(150.0)).child(self.first.clone()))
                    .child(div().w(px(320.0)).h(px(150.0)).child(self.second.clone()))
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root {
            first: cx.new(|cx| {
                let terminal = super::TerminalView::attach(crate::hc::offline(), "t1".into(), (32, 6), cx);
                terminal.grid.borrow_mut().vt.write(b"alpha result");
                terminal
            }),
            second: cx.new(|cx| {
                let terminal = super::TerminalView::attach(crate::hc::offline(), "t2".into(), (32, 6), cx);
                terminal.grid.borrow_mut().vt.write(b"beta result");
                terminal
            }),
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.first.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("a l p h a");
        cx.run_until_parked();
        assert!(cx.debug_bounds("find-bar").is_some());
        root.read_with(cx, |root, cx| {
            assert!(root.first.read(cx).search_open);
            assert!(!root.second.read(cx).search_open);
            assert_eq!(root.first.read(cx).search_query, "alpha");
        });

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.second.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("b e t a");
        cx.run_until_parked();
        assert!(cx.debug_bounds("find-bar").is_some());
        root.read_with(cx, |root, cx| {
            assert!(root.first.read(cx).search_open);
            assert!(root.second.read(cx).search_open);
            assert_eq!(root.first.read(cx).search_query, "alpha");
            assert_eq!(root.second.read(cx).search_query, "beta");
        });
    }

    #[gpui::test]
    fn find_input_does_not_type_into_terminal_and_escape_closes_it(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root(cx.new(|cx| {
            let terminal = super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx);
            terminal.grid.borrow_mut().vt.write(b"needle appears here");
            terminal
        })));
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("n e e d l e enter");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert_eq!(terminal.search_query, "needle");
            let frame = terminal.grid.borrow_mut().vt.frame();
            let text: String = frame.rows[0].iter().map(|cell| cell.text.as_str()).collect();
            assert!(text.starts_with("needle appears here"), "search input leaked into the shell: {text:?}");
        });

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| assert!(!root.0.read(cx).search_open));
    }

    #[gpui::test]
    fn reconnect_keeps_find_searching_the_new_terminal(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        cx.update(gpui_base::init);
        let terminal = cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx));
        let (_, cx) = cx.add_window_view(|_, _| gpui::Empty);
        cx.update(|window, app| terminal.update(app, |terminal, cx| {
            terminal.open_search(window, cx);
            terminal.search_query = "needle".into();
            terminal.reattach(crate::hc::offline(), cx);
            let mut grid = terminal.grid.borrow_mut();
            grid.vt.write(b"replayed needle\r\n");
            for _ in 0..1000 {
                grid.vt.progress_search();
                if grid.vt.search_progress().complete { break; }
            }
            assert!(grid.vt.search_progress().complete);
            assert_eq!(grid.vt.search_progress().total, 1);
            assert_eq!(grid.vt.frame().search_spans.len(), 1);
            grid.vt.set_search_needle("replayed");
            for _ in 0..1000 {
                grid.vt.progress_search();
                if grid.vt.search_progress().complete { break; }
            }
            assert_eq!(grid.vt.search_progress().total, 1, "query edits must keep working after reconnect");
        }));
    }

    #[gpui::test]
    fn shipped_clipboard_aliases_edit_find_without_terminal_input(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().key_context("Herdr").size_full()
                    .on_action(cx.listener(|this, action: &gpui_shell::action::ShellAction, _, cx| {
                        match action.id() {
                            "terminal.paste" => this.0.update(cx, |terminal, cx| terminal.paste(cx)),
                            "terminal.copy" => { this.0.update(cx, |terminal, cx| terminal.copy(cx)); }
                            _ => cx.propagate(),
                        }
                    }))
                    .child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let bindings = crate::keys::parse(crate::keys::DEFAULTS).expect("shipped keys parse");
        cx.update(|app| crate::keys::install(bindings, app));
        let (root, cx) = cx.add_window_view(|_, cx| {
            Root(cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx)))
        });
        cx.run_until_parked();
        let aliases: &[(&str, &str)] = if cfg!(target_os = "macos") {
            &[("cmd-v", "cmd-c")]
        } else {
            &[("ctrl-shift-v", "ctrl-shift-c"), ("shift-insert", "ctrl-insert")]
        };
        for &(paste, copy) in aliases {
            cx.update(|window, app| {
                root.update(app, |root, app| {
                    root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
                });
                app.write_to_clipboard(ClipboardItem::new_string("needle".into()));
            });
            cx.run_until_parked();
            cx.simulate_keystrokes(paste);
            cx.run_until_parked();
            root.read_with(cx, |root, cx| {
                let terminal = root.0.read(cx);
                assert_eq!(terminal.search_query, "needle", "{paste}");
                assert!(terminal.typed_at.get().is_none(), "{paste} sent terminal input");
            });
            cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" });
            cx.update(|_, app| app.write_to_clipboard(ClipboardItem::new_string("unchanged".into())));
            cx.simulate_keystrokes(copy);
            cx.run_until_parked();
            cx.update(|_, app| assert_eq!(app.read_from_clipboard().unwrap().text().as_deref(), Some("needle"), "{copy}"));
            root.read_with(cx, |root, cx| assert!(root.0.read(cx).typed_at.get().is_none(), "{copy} sent terminal input"));
        }
    }

    #[gpui::test]
    fn reopening_the_find_bar_selects_the_kept_query(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root(cx.new(|cx| {
            let terminal = super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx);
            terminal.grid.borrow_mut().vt.write(b"needle appears here");
            terminal
        })));
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("n e e d l e");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| assert_eq!(root.0.read(cx).search_query, "needle"));

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| assert!(!root.0.read(cx).search_open));

        // The kept query comes back selected, so typing replaces it instead
        // of appending to the stale needle.
        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(terminal.search_open);
            assert_eq!(terminal.search_query, "needle");
            let input = terminal.search_input.as_ref().unwrap().read(cx);
            assert_eq!(input.selected_range(), 0.."needle".len());
        });
        cx.simulate_keystrokes("x y z");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| assert_eq!(root.0.read(cx).search_query, "xyz"));
    }

    #[gpui::test]
    fn find_bar_chevrons_step_older_up_and_newer_down(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root(cx.new(|cx| {
            let terminal = super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 5), cx);
            terminal.grid.borrow_mut().vt.write(b"old alpha\r\nbetween\r\nnew alpha");
            terminal
        })));
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("a l p h a");
        cx.run_until_parked();

        let click = |cx: &mut gpui::VisualTestContext, selector: &'static str| {
            let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is laid out")).center();
            cx.simulate_mouse_move(at, None, Modifiers::default());
            cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
        };
        let selected_row = |cx: &mut gpui::VisualTestContext| {
            root.read_with(cx, |root, cx| {
                root.0.read(cx).grid.borrow_mut().vt.frame()
                    .search_spans.iter()
                    .filter(|span| span.selected)
                    .map(|span| span.row)
                    .collect::<Vec<_>>()
            })
        };

        click(cx, "find-up");
        assert_eq!(selected_row(cx), vec![2], "the newest match, at the bottom, is selected first");
        click(cx, "find-up");
        assert_eq!(selected_row(cx), vec![0], "the second press up is the older match");
        click(cx, "find-down");
        assert_eq!(selected_row(cx), vec![2]);
    }

    #[gpui::test]
    fn input_methods_follow_keyboard_focus_while_the_find_bar_is_open(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root(cx.new(|cx| {
            super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx)
        })));
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| {
                    terminal.open_search(window, cx);
                    gpui::EntityInputHandler::replace_text_in_range(terminal, None, "leak", window, cx);
                    gpui::EntityInputHandler::replace_and_mark_text_in_range(terminal, None, "leak", None, window, cx);
                });
            });
        });
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(terminal.typed_at.get().is_none(), "text reached the program while the find field held the keys");
            assert!(terminal.grid.borrow().marked.is_none(), "composition previewed while the find field held the keys");
        });

        // The terminal canvas was clicked: the terminal root (not the find
        // field) owns keyboard focus, so text input methods reach the program
        // while the bar stays open.
        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| {
                    terminal.focus.focus(window, cx);
                    gpui::EntityInputHandler::replace_text_in_range(terminal, None, "typed", window, cx);
                });
            });
        });
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(terminal.search_open);
            assert!(terminal.typed_at.get().is_some(), "text did not reach the program once the terminal held the keys");
        });
    }

    #[gpui::test]
    fn typing_reaches_the_program_while_the_find_bar_is_open(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| Root(cx.new(|cx| {
            super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx)
        })));
        cx.run_until_parked();

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.open_search(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("l s");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert_eq!(terminal.search_query, "ls");
            assert!(terminal.typed_at.get().is_none(), "keystrokes reached the program while the find field held the keys");
        });

        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.focus.focus(window, cx));
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("l s");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(terminal.search_open);
            assert!(terminal.typed_at.get().is_some(), "keystrokes did not reach the program once the terminal held the keys");
        });

        cx.update(|_, app| root.update(app, |root, app| {
            root.0.update(app, |terminal, _| terminal.typed_at.set(None));
        }));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(terminal.search_open, "terminal Escape must not close Find");
            assert!(terminal.typed_at.get().is_some(), "Escape must reach the program");
        });
    }

    #[gpui::test]
    fn shipped_find_binding_opens_search_but_ctrl_f_reaches_the_program(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .key_context("Herdr")
                    .size_full()
                    .on_action(cx.listener(|this, action: &gpui_shell::action::ShellAction, window, cx| {
                        if action.id() == "terminal.search" {
                            this.0.update(cx, |terminal, cx| terminal.open_search(window, cx));
                        } else {
                            cx.propagate();
                        }
                    }))
                    .child(self.0.clone())
            }
        }

        cx.update(gpui_base::init);
        let bindings = crate::keys::parse(crate::keys::DEFAULTS).expect("shipped keys parse");
        cx.update(|app| crate::keys::install(bindings, app));
        let (root, cx) = cx.add_window_view(|_, cx| {
            Root(cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 6), cx)))
        });
        cx.run_until_parked();
        cx.update(|window, app| {
            root.update(app, |root, app| {
                root.0.update(app, |terminal, cx| terminal.focus.focus(window, cx));
            });
        });

        cx.simulate_keystrokes("ctrl-f");
        cx.run_until_parked();
        root.read_with(cx, |root, cx| {
            let terminal = root.0.read(cx);
            assert!(!terminal.search_open);
            assert!(terminal.typed_at.get().is_some(), "Ctrl+F must reach the program");
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-shift-f" });
        cx.run_until_parked();
        root.read_with(cx, |root, cx| assert!(root.0.read(cx).search_open));
        assert!(cx.debug_bounds("find-bar").is_some());
    }

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

    // The native macOS text system is private to GPUI; creating MacPlatform here
    // requires the main thread. Exercise real shaping directly on Linux instead.
    #[cfg(target_os = "linux")]
    #[test]
    fn ligatures_change_glyphs_without_moving_terminal_cells() {
        use super::*;
        use std::borrow::Cow;

        let system = Arc::new(TextSystem::new(Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(crate::settings::DEFAULT_FONT))));
        system.add_fonts(vec![Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf"))]).unwrap();
        let text_system = WindowTextSystem::new(system);
        let original = crate::settings::get();
        let mut settings = (*original).clone();
        settings.font_family = crate::settings::DEFAULT_FONT.into();
        for font_size in [px(10.0), px(14.0), px(24.0), px(36.0)] {
            for text in ["a=>b", "a!=b", "a===b", "a->b", "a<=b", "a>=b", "a|>b"] {
                let mut variants = Vec::new();
                for enabled in [false, true] {
                    settings.font_ligatures = enabled;
                    crate::settings::set(settings.clone());
                    let mut font = cell_font(false, false);
                    font.features = ligature_features(enabled);
                    let font_id = text_system.resolve_font(&font);
                    let width = text_system.advance(font_id, font_size, 'M').unwrap().width;
                    let run = TextRun { len: text.len(), font, ..Default::default() };
                    let line = text_system.shape_line(text.into(), font_size, &[run], Some(width));

                    assert!(f32::from(line.width() - width * text.len() as f32).abs() < 0.1, "{text}: width drift");
                    let glyphs: Vec<_> = line.runs.iter().flat_map(|run| &run.glyphs).collect();
                    for glyph in &glyphs {
                        assert!(f32::from(glyph.position.x - width * glyph.index as f32).abs() < 0.1, "{text}: glyph drift");
                    }
                    variants.push(glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>());
                }
                assert_ne!(variants[0], variants[1], "{text}: ligatures must change the glyphs");
            }
        }
        crate::settings::set((*original).clone());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_symbols_fit_their_terminal_cells_pixels() {
        use super::*;
        use std::borrow::Cow;

        struct Symbols;
        impl Render for Symbols {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().bg(rgb(0x000000)).child(canvas(
                    |_, _, _| {},
                    |_, _, window, cx| {
                        let cell = cell_size(window);
                        for (y, text) in ["🕹", "🖥", "🌡", "🕹\u{fe0f}", "🖥\u{fe0f}", "🌡\u{fe0f}", "M"].iter().enumerate() {
                            let wide = text.contains('\u{fe0f}');
                            let mut row = vec![vt::Cell { text: text.to_string(), wide, fg: vt::Rgb(255, 0, 0), ..Default::default() }];
                            if wide { row.push(vt::Cell::default()); }
                            row.push(vt::Cell { text: "a".into(), fg: vt::Rgb(255, 255, 255), ..Default::default() });
                            for op in row_ops(&row, cell, None, window) {
                                if let RowOp::Text { x, line } = op {
                                    line.paint(point(px(4.) + cell.width * x as f32, cell.height * y as f32), cell.height, TextAlign::Left, None, window, cx).unwrap();
                                }
                            }
                        }
                    },
                ).size_full())
            }
        }

        let platform = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(crate::settings::DEFAULT_FONT));
        let system = TextSystem::new(platform.clone());
        system.add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf")),
            Cow::Borrowed(include_bytes!("../vendor/ghostty/src/font/res/NotoEmoji-Regular.ttf")),
        ]).unwrap();
        let mut cx = HeadlessAppContext::with_platform(platform, Arc::new(()), gpui_platform::current_headless_renderer);
        let original = crate::settings::get();
        let mut settings = (*original).clone();
        settings.font_family = crate::settings::DEFAULT_FONT.into();
        settings.font_size = 24.;
        crate::settings::set(settings);
        let window = cx.open_window(size(px(120.), px(280.)), |_, cx| cx.new(|_| Symbols)).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
            let image = window.render_to_image().unwrap();
            let row: Vec<_> = ["a", "🕹", "a", "🖥", "a", "🌡", "a"].into_iter()
                .map(|text| vt::Cell { text: text.into(), ..Default::default() }).collect();
            let ops = row_ops(&row, cell_size(window), None, window);
            let text: Vec<_> = ops.iter().filter_map(|op| match op {
                RowOp::Text { x, line } => Some((*x, line.font_size)),
                _ => None,
            }).collect();
            assert_eq!(text.len(), 7, "oversized fallback must break a same-style run");
            for (x, size) in text {
                if x % 2 == 0 { assert_eq!(size, px(24.)); }
                else { assert!(size < px(24.)); }
            }
            for text in ["🕹\u{fe0f}", "🖥\u{fe0f}", "🌡\u{fe0f}"] {
                let row = [vt::Cell { text: text.into(), wide: true, ..Default::default() }];
                for op in row_ops(&row, cell_size(window), None, window) {
                    if let RowOp::Text { line, .. } = op { assert_eq!(line.font_size, px(24.)); }
                }
            }
            if let Ok(path) = std::env::var("HERDR_SYMBOL_PIXELS") { image.save(path).unwrap(); }
            let scale = window.scale_factor();
            let cell = cell_size(window);
            for (y, slots) in [1, 1, 1, 2, 2, 2, 1].into_iter().enumerate() {
                let mut inside = 0;
                let mut outside = 0;
                for (x, py, pixel) in image.enumerate_pixels() {
                    let logical_y = py as f32 / scale;
                    if logical_y < f32::from(cell.height) * y as f32 || logical_y >= f32::from(cell.height) * (y + 1) as f32 { continue; }
                    if pixel[0] > 80 && pixel[1] < 20 && pixel[2] < 20 {
                        if x as f32 / scale < 4. + f32::from(cell.width) * slots as f32 { inside += 1; } else { outside += 1; }
                    }
                }
                println!("symbol row {y}: inside={inside}, overflow={outside}");
                assert!(inside > 10, "fixture must draw row {y}");
                assert_eq!(outside, 0, "row {y} glyph must fit its {slots} cell(s)");
            }
        }).unwrap();
        crate::settings::set((*original).clone());
    }

    #[test]
    fn ordinary_output_does_not_enable_ligature_shaping() {
        use super::*;
        let font = font(crate::settings::DEFAULT_FONT);
        assert!(!needs_ligatures("hello world 123 www", &font));
        assert!(!needs_ligatures("a=b - c", &font));
        assert!(needs_ligatures("a=>b -> c", &font));
        assert!(needs_ligatures("界=>", &font));
        assert!(needs_ligatures("fi ->", &gpui::font("Fira Code")));
        assert!(needs_ligatures("fi", &gpui::font("Fira Code")));
    }

    #[gpui::test]
    fn ligatures_off_keeps_one_style_run_across_backgrounds(cx: &mut gpui::TestAppContext) {
        use super::*;
        let original = crate::settings::get();
        let mut settings = (*original).clone();
        settings.font_ligatures = false;
        crate::settings::set(settings);
        let mut row: Vec<_> = "a=>b 42".chars().map(|ch| vt::Cell { text: ch.to_string(), ..Default::default() }).collect();
        row[2].bg = Some(vt::Rgb(10, 20, 30));
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, _| {
            let ops = row_ops(&row, size(px(8.), px(16.)), None, window);
            let runs: Vec<_> = ops.iter().filter_map(|op| match op {
                RowOp::Text { x, line } => Some((*x, line.text.as_ref())),
                _ => None,
            }).collect();
            assert_eq!(runs, vec![(0, "a=>b 42")]);
        }).unwrap();
        crate::settings::set((*original).clone());
    }

    #[gpui::test]
    fn ligatures_break_at_the_cursor_cell(cx: &mut gpui::TestAppContext) {
        use super::*;
        let original = crate::settings::get();
        let mut settings = (*original).clone();
        settings.font_ligatures = true;
        crate::settings::set(settings);
        let row: Vec<_> = "a=>b".chars().map(|ch| vt::Cell { text: ch.to_string(), ..Default::default() }).collect();
        let window = cx.add_window(|_, _| Empty);

        window.update(cx, |_, window, _| {
            for (cursor, expected) in [
                (None, vec![(0, "a=>b")]),
                (Some(1), vec![(0, "a"), (1, "="), (2, ">b")]),
                (Some(2), vec![(0, "a="), (2, ">"), (3, "b")]),
            ] {
                let ops = row_ops(&row, size(px(8.), px(16.)), cursor, window);
                let runs: Vec<_> = ops.iter().filter_map(|op| match op {
                    RowOp::Text { x, line } => Some((*x, line.text.as_ref())),
                    _ => None,
                }).collect();
                assert_eq!(runs, expected, "cursor: {cursor:?}");
            }
        }).unwrap();
        crate::settings::set((*original).clone());
    }

    #[gpui::test]
    fn a_click_the_program_takes_ends_the_selection(cx: &mut gpui::TestAppContext) {
        use gpui::*;

        struct Root(Entity<super::TerminalView>);
        impl Render for Root {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(px(400.)).h(px(100.)).child(self.0.clone())
            }
        }
        cx.update(gpui_base::init);
        let (root, cx) = cx.add_window_view(|_, cx| {
            Root(cx.new(|cx| super::TerminalView::attach(crate::hc::offline(), "t1".into(), (40, 5), cx)))
        });
        cx.run_until_parked();
        let terminal = root.read_with(cx, |root, _| root.0.clone());
        terminal.update(cx, |t, _| {
            let mut g = t.grid.borrow_mut();
            // The program tracks the mouse, and a Shift+drag selected some text.
            g.vt.write(b"\x1b[?1000hold text");
            g.vt.select(super::Select::Press, 0, 0, 0., 5., 10., 60.);
            g.vt.select(super::Select::Drag, 2, 0, 25., 5., 10., 60.);
            g.vt.select(super::Select::Release, 2, 0, 25., 5., 10., 60.);
            assert!(g.vt.mouse_tracking());
            assert!(g.vt.selection_text().is_some());
        });
        cx.simulate_mouse_down(point(px(50.), px(50.)), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(point(px(50.), px(50.)), MouseButton::Left, Modifiers::default());
        assert_eq!(terminal.read_with(cx, |t, _| t.grid.borrow().vt.selection_text()), None);
    }

    #[gpui::test]
    fn ligatures_break_at_selection_boundaries(cx: &mut gpui::TestAppContext) {
        use super::*;
        let original = crate::settings::get();
        let mut settings = (*original).clone();
        settings.font_ligatures = true;
        crate::settings::set(settings);
        let mut vt = Vt::new(30, 2);
        vt.write(b"a=>b");
        vt.select(Select::Press, 2, 0, 25., 5., 10., 60.);
        vt.select(Select::Drag, 3, 0, 30., 5., 10., 60.);
        vt.select(Select::Release, 3, 0, 30., 5., 10., 60.);
        assert_eq!(vt.selection_text().as_deref(), Some(">"));
        let window = cx.add_window(|_, _| Empty);

        window.update(cx, |_, window, _| {
            let ops = row_ops(&vt.frame().rows[0], size(px(8.), px(16.)), None, window);
            let runs: Vec<_> = ops.iter().filter_map(|op| match op {
                RowOp::Text { x, line } => Some((*x, line.text.as_ref())),
                _ => None,
            }).collect();
            assert_eq!(runs, vec![(0, "a="), (2, ">"), (3, "b")]);
        }).unwrap();
        crate::settings::set((*original).clone());
    }

    #[gpui::test]
    fn ligatures_keep_wide_neighbors_in_their_columns(cx: &mut gpui::TestAppContext) {
        use super::*;
        let original = crate::settings::get();
        let window = cx.add_window(|_, _| Empty);
        let mut vt = Vt::new(30, 2);
        vt.write("界->😀=>中!=x".as_bytes());
        window.update(cx, |_, window, _| {
            for (font_size, zoom) in [(10.0, 1.0), (18.0, 1.5), (24.0, 2.0)] {
                let mut settings = (*original).clone();
                settings.font_ligatures = true;
                settings.font_size = font_size;
                settings.zoom = zoom;
                crate::settings::set(settings);
                let ops = row_ops(&vt.frame().rows[0], size(px(8.), px(16.)), None, window);
                let runs: Vec<_> = ops.iter().filter_map(|op| match op {
                    RowOp::Text { x, line } => Some((*x, line.text.as_ref())),
                    _ => None,
                }).collect();
                assert_eq!(runs, vec![(0, "界"), (2, "->"), (4, "😀"), (6, "=>"), (8, "中"), (10, "!=x")]);
            }
        }).unwrap();
        crate::settings::set((*original).clone());
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
                        let ops = super::row_ops(&row, gpui::size(gpui::px(8.0), gpui::px(16.0)), None, window);
                        let rendered: String = ops.iter().filter_map(|op| match op {
                            super::RowOp::Text { line, .. } => Some(line.text.as_ref()),
                            _ => None,
                        }).collect();
                        assert!(rendered.contains(missing), "{text:?}, underline={underline}, strike={strike}");
                        assert!(rendered.contains('\u{f10d}'));
                        assert!(!ops.iter().any(|op| matches!(op, super::RowOp::Icon { .. })));
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
    fn icons_available_in_the_terminal_font_stay_in_one_text_run(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, _| {
            // The test text system supplies every glyph in the requested font.
            for (bold, italic) in [(false, false), (true, false), (false, true)] {
                for text in ["\u{f1c0}\u{f0e7} 99.6%", "\u{f0e4} 54 t/s", "\u{f233} \u{f0e7}"] {
                    let row: Vec<_> = text.chars().map(|c| crate::vt::Cell {
                        text: c.to_string(), bold, italic, ..Default::default()
                    }).collect();
                    let ops = super::row_ops(&row, gpui::size(gpui::px(8.0), gpui::px(16.0)), None, window);
                    assert_eq!(ops.len(), 1, "{text:?}");
                    let super::RowOp::Text { x, line } = &ops[0] else { panic!("configured font was bypassed") };
                    assert_eq!(*x, 0);
                    assert_eq!(line.text.as_ref(), text);
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
                    let ops = super::row_ops(&row, gpui::size(gpui::px(8.0), gpui::px(16.0)), None, window);
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
    fn the_scroll_thumb_maps_the_viewport_into_its_track() {
        use super::scrollbar_thumb as thumb;
        // No scrollback: no thumb.
        assert_eq!(thumb(40, 0, 40, 100.0, 4.0), None);
        // Half the scrollable area visible: a half-height thumb at the top.
        assert_eq!(thumb(80, 0, 40, 100.0, 4.0), Some((0.0, 50.0)));
        // At the bottom it reaches the track's end.
        assert_eq!(thumb(80, 40, 40, 100.0, 4.0), Some((50.0, 50.0)));
        // A huge scrollback still leaves a grabbable thumb.
        assert_eq!(thumb(10_000, 0, 40, 100.0, 4.0), Some((0.0, 4.0)));
    }

    #[test]
    fn scrollbar_lane_survives_zero_and_small_padding_without_resizing() {
        use gpui::{point, px, size, Bounds};
        let bounds = Bounds::new(point(px(20.0), px(30.0)), size(px(400.0), px(200.0)));
        for padding in [0.0, 1.0, 2.0, 14.0] {
            for remainder in [0.0, 0.5, 7.0] {
                let origin = bounds.origin + point(px(padding), px(padding));
                let width = px(400.0 - padding * 2.0 - remainder);
                let lane = super::scrollbar_lane(bounds, origin, width, px(160.0));
                assert!(lane.size.width >= px(8.0));
                assert_eq!(lane.origin.x + lane.size.width, px(420.0));
                assert_eq!(lane.origin.y, origin.y);
                assert_eq!(lane.size.height, px(160.0));
            }
        }
    }

    #[test]
    fn track_click_centers_the_thumb_and_preserves_a_thumb_grab() {
        use super::{scrollbar_grab_at as grab, scrollbar_row_at as row_at, scrollbar_thumb as thumb};
        let (top, len) = thumb(1000, 800, 40, 100.0, 4.0).unwrap();
        assert_eq!(grab(top, len, top + 1.0), 1.0);
        let offset = grab(top, len, 50.0);
        assert_eq!(offset, len / 2.0);
        assert_eq!(row_at(1000, 40, 100.0, 4.0, 50.0 - offset), 480);
        assert_eq!(row_at(1000, 40, 100.0, 4.0, -offset), 0);
        assert_eq!(row_at(1000, 40, 100.0, 4.0, 100.0 - offset), 960);
    }

    #[test]
    fn a_scroll_thumb_drag_round_trips_to_a_row() {
        use super::{scrollbar_row_at as row_at, scrollbar_thumb as thumb};
        let (track, min) = (100.0, 4.0);
        // The track's ends are the scrollback's ends.
        assert_eq!(row_at(80, 40, track, min, 0.0), 0);
        assert_eq!(row_at(80, 40, track, min, track), 40);
        // A thumb's painted top reads back as its own offset.
        let (top, _) = thumb(80, 25, 40, track, min).unwrap();
        assert_eq!(row_at(80, 40, track, min, top), 25);
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
            ("ctrl-[", None, b"\x1b", b"\x1b[91;5u", b"\x1b[91;5u"),
            // GPUI strips Shift from punctuation; control text is absent.
            ("ctrl-_", None, b"\x1f", b"\x1b[45;6u", b"\x1b[45;6u"),
            ("ctrl--", None, b"\x1f", b"\x1b[45;5u", b"\x1b[45;5u"),
            // Also accept backends that retain the physical minus and Shift.
            ("ctrl-shift--", None, b"\x1f", b"\x1b[45;6u", b"\x1b[45;6u"),
            ("ctrl-/", None, b"\x1f", b"\x1b[47;5u", b"\x1b[47;5u"),
            ("ctrl-i", None, b"\t", b"\x1b[105;5u", b"\x1b[105;5u"),
            ("ctrl-m", None, b"\r", b"\x1b[109;5u", b"\x1b[109;5u"),
            ("ctrl-]", None, b"\x1d", b"\x1b[93;5u", b"\x1b[93;5u"),
            ("ctrl-\\", None, b"\x1c", b"\x1b[92;5u", b"\x1b[92;5u"),
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

    #[test]
    fn minus_control_aliases_respect_modify_other_keys() {
        for (spec, typed, modified) in [
            ("ctrl--", None, b"\x1b[27;5;45~".as_slice()),
            ("ctrl--", Some("-"), b"\x1b[27;5;45~"),
            ("ctrl-_", None, b"\x1b[27;6;95~"),
            ("ctrl-_", Some("_"), b"\x1b[27;6;95~"),
            ("ctrl-shift--", None, b"\x1b[27;6;95~"),
            ("ctrl-shift--", Some("_"), b"\x1b[27;6;95~"),
        ] {
            for (setup, expected) in [
                (b"".as_slice(), b"\x1f".as_slice()),
                (b"\x1b[>4;1m", b"\x1f"),
                (b"\x1b[>4;2m", modified),
                (b"\x1b[>4;2m\x1b[>4;0m", b"\x1f"),
            ] {
                let mut vt = Vt::new(20, 3);
                vt.write(setup);
                assert_eq!(key_bytes(&mut vt, &press(spec, typed)).as_deref(), Some(expected), "{spec}, typed={typed:?}, setup={setup:?}");
            }
        }
    }

    #[test]
    fn control_aliases_respect_modify_other_keys() {
        for (setup, expected) in [
            (b"".as_slice(), [b"\x1b".as_slice(), b"\t", b"\r"]),
            (b"\x1b[>4;2m".as_slice(), [b"\x1b[27;5;91~".as_slice(), b"\x1b[27;5;105~", b"\x1b[27;5;109~"]),
            (b"\x1b[>4;2m\x1b[>4;0m".as_slice(), [b"\x1b".as_slice(), b"\t", b"\r"]),
            (b"\x1b[>4;1m".as_slice(), [b"\x1b".as_slice(), b"\t", b"\r"]),
            (b"\x1b[>1u".as_slice(), [b"\x1b[91;5u".as_slice(), b"\x1b[105;5u", b"\x1b[109;5u"]),
            (b"\x1b[>4;2m\x1b[>1u".as_slice(), [b"\x1b[91;5u".as_slice(), b"\x1b[105;5u", b"\x1b[109;5u"]),
        ] {
            let mut vt = Vt::new(20, 3);
            vt.write(setup);
            for (spec, want) in ["ctrl-[", "ctrl-i", "ctrl-m"].into_iter().zip(expected) {
                assert_eq!(key_bytes(&mut vt, &press(spec, None)).as_deref(), Some(want), "{spec}, setup={setup:?}");
            }
        }
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

    #[test]
    fn scroll_takes_exactly_one_of_lines_pages_or_to() {
        assert_eq!(scroll_target(&json!({ "lines": -5 })), Ok(ScrollTarget::Lines(-5)));
        assert_eq!(scroll_target(&json!({ "pages": -0.5 })), Ok(ScrollTarget::Pages(-0.5)));
        assert_eq!(scroll_target(&json!({ "to": "top" })), Ok(ScrollTarget::Top));
        assert_eq!(scroll_target(&json!({ "to": "bottom", "terminal": "t1" })), Ok(ScrollTarget::Bottom));
        assert_eq!(scroll_target(&json!({ "lines": null, "to": "top" })), Ok(ScrollTarget::Top), "null is omitted");
    }

    #[test]
    fn scroll_names_missing_and_repeated_targets() {
        let needs_one = "terminal.scroll needs one of lines, pages, to";
        assert_eq!(scroll_target(&json!({})), Err(needs_one.into()));
        assert_eq!(scroll_target(&json!({ "terminal": "t1", "lines": null })), Err(needs_one.into()));
        assert_eq!(scroll_target(&json!({ "lines": 1, "to": "top" })), Err("terminal.scroll takes one of lines, pages, to, not lines and to".into()));
        assert_eq!(scroll_target(&json!({ "lines": 1, "pages": 1, "to": "top" })), Err("terminal.scroll takes one of lines, pages, to, not lines and pages and to".into()));
    }

    #[test]
    fn scroll_rejects_values_the_target_does_not_take() {
        let error = |args: Value| scroll_target(&args).unwrap_err();
        assert_eq!(error(json!({ "lines": 1.5 })), "terminal.scroll lines is not a whole number: 1.5");
        assert_eq!(error(json!({ "lines": "5" })), "terminal.scroll lines is not a whole number: \"5\"");
        assert_eq!(error(json!({ "pages": "half" })), "terminal.scroll pages is not a finite number: \"half\"");
        assert_eq!(error(json!({ "to": "middle" })), "terminal.scroll to is \"top\" or \"bottom\", not \"middle\"");
        assert_eq!(error(json!({ "to": 5 })), "terminal.scroll to is \"top\" or \"bottom\", not 5");
    }

    #[test]
    fn pages_become_whole_lines_of_the_terminals_rows() {
        assert_eq!(pages_to_lines(1.0, 24), 24);
        assert_eq!(pages_to_lines(-1.0, 24), -24);
        assert_eq!(pages_to_lines(-0.5, 24), -12);
        assert_eq!(pages_to_lines(-0.5, 25), -13, "half of an odd row count rounds away from zero");
        assert_eq!(pages_to_lines(0.02, 24), 0);
    }
}

//! What a script application's tasks do across a reload and between copies.
//!
//! A mod is shown as several copies of one application and reloaded in place
//! on every save. Work started by a copy belongs to that copy, and work started
//! by a replaced build ends with it.

use crate::{HostModule, HostValue, ScriptView, ShellRuntime};
use gpui::{Entity, IntoElement as _, TestAppContext, VisualTestContext};
use std::{cell::Cell, ops::Deref as _, path::PathBuf, rc::Rc, time::Duration};

struct Empty;

impl gpui::Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

struct App {
    runtime: Rc<ShellRuntime>,
    context: VisualTestContext,
    directory: PathBuf,
    extra_dirs: Vec<PathBuf>,
    revision: Rc<Cell<u64>>,
}

impl App {
    fn new(cx: &mut TestAppContext, name: &str, source: &str) -> Self {
        cx.update(crate::init);
        let runtime = ShellRuntime::new_isolated().expect("runtime");
        let revision = Rc::new(Cell::new(0));
        let provider = revision.clone();
        runtime.set_source_revision_provider(move |_| Some(provider.get()));
        cx.update(|cx| runtime.set_global(cx));
        let directory = std::env::temp_dir().join(format!("gpui-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("application directory");
        std::fs::write(directory.join("main.js"), source).expect("source");
        let window = cx.add_window(|_, _| Empty);
        let context = VisualTestContext::from_window(*window.deref(), cx);
        Self {
            runtime,
            context,
            directory,
            extra_dirs: Vec::new(),
            revision,
        }
    }

    fn load_separate_app(&mut self, name: &str, source: &str) -> (PathBuf, Entity<ScriptView>) {
        let dir = std::env::temp_dir().join(format!("gpui-shell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("extra application directory");
        std::fs::write(dir.join("main.js"), source).expect("source");
        self.extra_dirs.push(dir.clone());
        let runtime = self.runtime.clone();
        let view = self.context.update(|window, cx| {
            let view_type = runtime.load_app(&dir, "main.js").expect("load");
            runtime.instantiate_view(&view_type, window, cx).expect("instantiate")
        });
        (dir, view)
    }

    fn save_in(&mut self, dir: &PathBuf, view: &Entity<ScriptView>, source: &str) -> anyhow::Result<()> {
        std::fs::write(dir.join("main.js"), source).expect("source");
        self.revision.set(self.revision.get() + 1);
        let runtime = self.runtime.clone();
        self.context
            .update(|window, cx| crate::watch::reload(&runtime, view, dir, "main.js", window, cx))
    }

    /// Copies of one loaded application, like one mod shown in several blocks.
    fn copies(&mut self, count: usize) -> Vec<Entity<ScriptView>> {
        let (runtime, directory) = (self.runtime.clone(), self.directory.clone());
        self.context.update(|window, cx| {
            (0..count)
                .map(|_| {
                    let view_type = runtime.load_app(&directory, "main.js").expect("load");
                    runtime.instantiate_view(&view_type, window, cx).expect("instantiate")
                })
                .collect()
        })
    }

    fn save(&mut self, view: &Entity<ScriptView>, source: &str) -> anyhow::Result<()> {
        let dir = self.directory.clone();
        self.save_in(&dir, view, source)
    }

    fn settle(&mut self) {
        self.context.executor().advance_clock(Duration::from_millis(50));
        self.context.run_until_parked();
    }

    fn draw(&mut self, view: &Entity<ScriptView>) {
        let drawn = view.clone();
        self.context.draw(
            gpui::Point::default(),
            gpui::size(gpui::px(400.), gpui::px(300.)),
            move |_, _| drawn.into_any_element(),
        );
    }

    fn text(&mut self, view: &Entity<ScriptView>) -> String {
        self.draw(view);
        self.context.update(|_, cx| {
            view.read(cx).snapshot().map(crate::RenderSnapshot::debug_tree).unwrap_or_default()
        })
    }

    fn child_texts(&mut self, view: &Entity<ScriptView>) -> Vec<String> {
        self.draw(view);
        let children: Vec<_> = self.context.update(|_, cx| {
            let Some(snapshot) = view.read(cx).snapshot() else {
                return Vec::new();
            };
            (0..snapshot.len() as u32)
                .filter_map(|id| snapshot.arena().node(id))
                .filter_map(|node| match node.component() {
                    Some(crate::spec::Component::ChildView(child)) => Some(child.view().clone()),
                    _ => None,
                })
                .collect()
        });
        children.iter().map(|child| self.text(child)).collect()
    }

    fn build_error(&mut self, view: &Entity<ScriptView>) -> Option<String> {
        self.draw(view);
        view.read_with(&self.context, |view, _| view.build_error().map(str::to_owned))
    }

    fn dirty(&mut self, view: &Entity<ScriptView>) -> bool {
        view.read_with(&self.context, |view, _| view.is_dirty())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
        for dir in &self.extra_dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A promise every copy shares, settled by `fire()`, and a trace the view shows.
const SHARED: &str = r#"
globalThis.trace ??= [];
globalThis.gate ??= (() => { const g = {}; g.p = new Promise((r) => (g.r = r)); return g; })();
globalThis.fire = () => {
  const settle = globalThis.gate.r;
  globalThis.gate.p = new Promise((r) => (globalThis.gate.r = r));
  settle();
};
"#;

fn waiting_loop(build: u32) -> String {
    format!(
        "import {{ View }} from \"gpui-kit\";\n{SHARED}\n\
         export default class Probe extends View {{\n\
           init(_props, cx) {{\n\
             cx.spawn(async (cx) => {{\n\
               for (;;) {{ await globalThis.gate.p; cx.notify(); globalThis.trace.push(\"build{build}\"); }}\n\
             }});\n\
             if ({build} > 1) cx.timer.after(10, () => globalThis.fire());\n\
           }}\n\
           render() {{ return `trace:${{globalThis.trace.join(\",\")}}`; }}\n\
         }}\n"
    )
}

#[gpui::test]
fn a_replaced_builds_loop_stops_at_its_next_cx_call(cx: &mut TestAppContext) {
    let mut app = App::new(cx, "replaced-loop", &waiting_loop(1));
    let view = app.copies(1).remove(0);
    app.save(&view, &waiting_loop(2)).expect("reload");
    app.settle();

    let text = app.text(&view);
    assert!(text.contains("trace:build2"), "the new build's loop must run: {text}");
    assert!(!text.contains("build1"), "the replaced build's loop must not run on: {text}");
}

#[gpui::test]
fn a_task_redraws_the_copy_that_started_it(cx: &mut TestAppContext) {
    let source = format!(
        "import {{ View }} from \"gpui-kit\";\n{SHARED}\n\
         export default class Copy extends View {{\n\
           init(_props, cx) {{\n\
             this.number = globalThis.copies = (globalThis.copies ?? 0) + 1;\n\
             if (this.number === 1) cx.spawn(async (cx) => {{ for (;;) {{ await globalThis.gate.p; cx.notify(); }} }});\n\
             else cx.timer.after(10, () => globalThis.fire());\n\
           }}\n\
           render() {{ return `copy ${{this.number}}`; }}\n\
         }}\n"
    );
    let mut app = App::new(cx, "owner-notify", &source);
    let copies = app.copies(2);
    let (first, second) = (&copies[0], &copies[1]);
    app.text(first);
    app.text(second);
    assert!(!app.dirty(first) && !app.dirty(second));

    app.settle();

    assert!(app.dirty(first), "the copy whose task called cx.notify() must redraw");
    assert!(!app.dirty(second), "the copy that happened to wake the task must not");
}

#[gpui::test]
fn a_timer_that_stops_itself_still_redraws(cx: &mut TestAppContext) {
    let source = "import { View } from \"gpui-kit\";\n\
         export default class Clock extends View {\n\
           init(_props, cx) {\n\
             this.ticks = 0;\n\
             this.timer = cx.timer.every(10, (cx) => { this.ticks++; this.timer.cancel(); cx.notify(); });\n\
           }\n\
           render() { return `ticks:${this.ticks}`; }\n\
         }\n";
    let mut app = App::new(cx, "self-cancel", source);
    let view = app.copies(1).remove(0);
    app.text(&view);
    app.settle();
    assert!(app.dirty(&view), "cancelling its own timer must not take the handler's cx.notify() with it");
    assert!(app.text(&view).contains("ticks:1"));
}

#[gpui::test]
fn owner_this_is_the_view_during_a_reloads_init(cx: &mut TestAppContext) {
    let source = |caption: &str| {
        format!(
            "import {{ View }} from \"gpui-kit\";\n\
             export default class Owned extends View {{\n\
               init(_props, cx) {{\n\
                 cx.spawn(async () => {{}}, {{ owner: this }});\n\
                 cx.timer.every(60_000, () => {{}}, {{ owner: this }});\n\
               }}\n\
               render() {{ return \"{caption}\"; }}\n\
             }}\n"
        )
    };
    let mut app = App::new(cx, "owner-this", &source("first"));
    let view = app.copies(1).remove(0);
    app.save(&view, &source("second")).expect("{ owner: this } must name the view being reloaded");
    assert!(app.text(&view).contains("second"));
}

#[gpui::test]
fn the_first_command_after_a_reload_reaches_the_new_build(cx: &mut TestAppContext) {
    let (send, inbox) = async_channel::unbounded::<String>();
    crate::export_module(HostModule::new("inbox").async_function("next", move |_| {
        let inbox = inbox.clone();
        Ok(async move { inbox.recv().await.map(HostValue::Str).map_err(|e| crate::HostError::new(e.to_string())) })
    }))
    .expect("`inbox` is free");

    let source = |build: u32| {
        format!(
            "import {{ View }} from \"gpui-kit\";\n\
             import {{ next }} from \"inbox\";\n\
             globalThis.got ??= [];\n\
             export default class Listener extends View {{\n\
               init(_props, cx) {{\n\
                 cx.spawn(async (cx) => {{ for (;;) {{ const c = await next(); globalThis.got.push(`build{build}:${{c}}`); cx.notify(); }} }});\n\
               }}\n\
               render() {{ return `got:${{globalThis.got.join(\",\")}}`; }}\n\
             }}\n"
        )
    };
    let mut app = App::new(cx, "first-command", &source(1));
    let view = app.copies(1).remove(0);
    app.settle();
    app.save(&view, &source(2)).expect("reload");
    app.settle();

    send.send_blocking("ping".into()).expect("send");
    app.settle();

    let text = app.text(&view);
    crate::clear_exported_modules();
    assert!(text.contains("got:build2:ping"), "the first command was lost: {text}");
}

#[gpui::test]
fn render_promise_across_sibling_reload_mounts_nested_entity_cleanly(cx: &mut TestAppContext) {
    let panel_source = |version: &str| {
        format!(
            "import {{ View, div }} from \"gpui-kit\";\n\
             class Child extends View {{\n\
               init(props) {{ this.label = props.label; }}\n\
               update(props) {{ this.label = props.label; }}\n\
               render() {{ return `child:${{this.label}}`; }}\n\
             }}\n\
             export default class Panel extends View {{\n\
               init(_props, cx) {{\n\
                 this.panelCx = cx;\n\
                 this.child = null;\n\
                 this.sent = false;\n\
                 let resolveMail = null;\n\
                 this.wake = (value) => {{ if (resolveMail) {{ const r = resolveMail; resolveMail = null; r(value); }} }};\n\
                 const inbox = new Promise((r) => {{ resolveMail = r; }});\n\
                 cx.spawn(async (taskCx) => {{\n\
                   const label = await inbox;\n\
                   const child = taskCx.new(Child, {{ label: `${{label}}-init` }});\n\
                   child.set_props({{ label }});\n\
                   this.child = child;\n\
                   taskCx.notify();\n\
                 }});\n\
               }}\n\
               render() {{\n\
                 if (!this.sent) {{\n\
                   this.sent = true;\n\
                   this.wake(\"render-{version}\");\n\
                 }}\n\
                 return this.child ? div().child(this.child) : \"pending-{version}\";\n\
               }}\n\
             }}\n"
        )
    };
    let tabs_source = |version: &str| {
        format!(
            "import {{ View }} from \"gpui-kit\";\n\
             export default class Tabs extends View {{\n\
               init(_props, cx) {{\n\
                 cx.spawn(async () => {{\n\
                   await cx.sleep(0);\n\
                 }});\n\
               }}\n\
               render() {{ return \"tabs-{version}\"; }}\n\
             }}\n"
        )
    };

    let mut app = App::new(cx, "render-promise-panel", &panel_source("v1"));
    let panel = app.copies(1).remove(0);
    // Load sibling tabs first so its cx.sleep(0) wake runs before panel's deferred render drain.
    let (tabs_dir, tabs) = app.load_separate_app("render-promise-tabs", &tabs_source("v1"));

    let _ = app.text(&panel);
    app.settle();

    let err_v1 = app.build_error(&panel);
    assert!(err_v1.is_none(), "panel v1 failed to mount nested child: {err_v1:?}");
    let children_v1 = app.child_texts(&panel);
    assert!(
        children_v1.iter().any(|t| t.contains("child:render-v1")),
        "expected render-v1 child, got: {children_v1:?}"
    );

    // Now reload panel itself while tabs also reloads and runs its sleep task.
    app.save(&panel, &panel_source("v2")).expect("reload panel");
    app.save_in(&tabs_dir, &tabs, &tabs_source("v2")).expect("reload tabs v2");
    let _ = app.text(&panel);
    let _ = app.text(&tabs);
    app.settle();

    let err_v2 = app.build_error(&panel);
    assert!(err_v2.is_none(), "panel v2 failed to mount nested child across reload: {err_v2:?}");
    let children_v2 = app.child_texts(&panel);
    assert!(
        children_v2.iter().any(|t| t.contains("child:render-v2")),
        "expected render-v2 child, got: {children_v2:?}"
    );
}

#[gpui::test]
fn stale_render_promise_across_own_reload_stops_without_corrupting_replacement(
    cx: &mut TestAppContext,
) {
    let source = |version: &str| {
        format!(
            "import {{ View, div }} from \"gpui-kit\";\n\
             class Child extends View {{\n\
               init(props) {{ this.label = props.label; }}\n\
               update(props) {{ this.label = props.label; }}\n\
               render() {{ return `child:{version}:${{this.label}}`; }}\n\
             }}\n\
             export default class Panel extends View {{\n\
               init(_props, cx) {{\n\
                 this.child = cx.new(Child, {{ label: \"boot\" }});\n\
                 this.triggered = false;\n\
                 Promise.resolve().then(() => {{\n\
                   this.child = cx.new(Child, {{ label: \"init-micro\" }});\n\
                   cx.notify();\n\
                 }});\n\
                 cx.spawn(async (taskCx) => {{\n\
                   await new Promise((r) => {{ this.resolveRender = r; }});\n\
                   this.child = taskCx.new(Child, {{ label: \"from-render\" }});\n\
                   taskCx.notify();\n\
                 }});\n\
               }}\n\
               render() {{\n\
                 if (!this.triggered && this.resolveRender) {{\n\
                   this.triggered = true;\n\
                   this.resolveRender();\n\
                 }}\n\
                 return div().child(this.child);\n\
               }}\n\
             }}\n"
        )
    };

    let mut app = App::new(cx, "stale-render-promise", &source("v1"));
    let panel = app.copies(1).remove(0);

    // Draw v1 so v1's render() resolves its JS Promise, then reload to v2 before settling.
    let _ = app.text(&panel);
    app.save(&panel, &source("v2")).expect("reload panel to v2");
    app.settle();

    let err = app.build_error(&panel);
    assert!(err.is_none(), "panel v2 failed after reload: {err:?}");
    let children = app.child_texts(&panel);
    assert!(
        children.iter().any(|t| t.contains("child:v2:"))
            && !children.iter().any(|t| t.contains("child:v1:")),
        "expected v2 child cleanly isolated from stale v1 render promise, got: {children:?}"
    );
}

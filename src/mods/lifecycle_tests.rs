use super::*;
use gpui::{IntoElement, Render, TestAppContext, VisualTestContext};
use std::ops::Deref;
use gpui_shell::{ShellRoot, ShellRuntime, policy::Policy};
use std::{cell::Cell, rc::Rc, sync::atomic::{AtomicU64, Ordering}, time::Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = PathBuf::from(format!("/var/tmp/herdr-lifecycle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.js"), source).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

struct Mounted(gpui::Entity<ShellRoot>);
impl Render for Mounted {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

#[gpui::test]
fn size_change_reports_initial_and_resized_layout_without_pointer_input(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().id("root").size_full().border(3).p(7)
      .on_size_change((size, cx) => { note(size.width, size.height); cx.notify(); });
  }
}"#);
    let sizes = Rc::new(RefCell::new(Vec::new()));
    let calls = sizes.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push((a.number(0)?, a.number(1)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1, "unchanged layout must not repeat the callback");
    visual.simulate_resize(gpui::size(gpui::px(720.), gpui::px(480.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().last().copied(), Some((720., 480.)));
    assert_eq!(sizes.borrow().len(), 2);
}

#[gpui::test]
fn external_file_drops_reach_mod_elements(cx: &mut TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent, InputEvent};
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().size_full().on_drop((event, cx) => { note(event.paths); cx.stop_propagation(); });
  }
}"#);
    let paths = Rc::new(RefCell::new(Vec::new()));
    let calls = paths.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(from_host(a.value(0)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    visual.update(|window, cx| {
        let position = gpui::point(gpui::px(20.), gpui::px(20.));
        window.dispatch_event(FileDropEvent::Entered {
            position,
            paths: ExternalPaths([PathBuf::from("/var/tmp/screenshot.png"), PathBuf::from("/var/tmp/a file.jpg")].into_iter().collect()),
        }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    });
    visual.run_until_parked();
    assert_eq!(*paths.borrow(), vec![json!(["/var/tmp/screenshot.png", "/var/tmp/a file.jpg"])]);
}

struct PathDropTarget {
    root: gpui::Entity<ShellRoot>,
    drops: Rc<RefCell<Vec<gpui_shell::DraggedPaths>>>,
}
impl Render for PathDropTarget {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{InteractiveElement, ParentElement, Styled};
        let drops = self.drops.clone();
        gpui::div().w(gpui::px(400.)).h(gpui::px(300.))
            .on_drop(move |paths: &gpui_shell::DraggedPaths, _, _| drops.borrow_mut().push(paths.clone()))
            .child(self.root.clone())
    }
}

fn assert_path_drag_preview_at(visual: &mut VisualTestContext, cursor: gpui::Point<gpui::Pixels>) {
    visual.update(|window, cx| {
        assert!(cx.has_active_drag());
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        let mut badges: Vec<_> = window.painted_quads().into_iter().filter(|quad|
            quad.corner_radii.top_left == gpui::px(6.).scale(scale)
                && quad.border_widths.top == gpui::px(1.).scale(scale)
        ).map(|quad| quad.bounds).collect();
        // Border sides may be separate quads with the same badge bounds.
        badges.dedup();
        assert_eq!(badges.len(), 1, "one compact file drag badge is painted");
        assert_eq!(badges[0].origin, (cursor + gpui::point(gpui::px(12.), gpui::px(12.))).scale(scale),
            "the compact badge follows the pointer, not the source element's grab offset");
    });
}

#[gpui::test]
fn file_drag_preview_follows_the_pointer_from_either_end_of_wide_sources(cx: &mut TestAppContext) {
    use gpui::{MouseButton, point, px};
    cx.update(gpui_shell::init);
    for width in [180., 800.] {
        let fixture = Fixture::new(&format!(r#"
import {{ View, div }} from "gpui-kit";
export default class extends View {{
  render() {{
    return div().id("source").w({width}).h(42).text_right()
      .drag_paths(["/work/notes.txt"]).child("notes.txt");
  }}
}}"#));
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(gpui::size(px(1000.), px(600.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        for scale in [1., 2.] {
            visual.simulate_scale_factor_change(scale);
            for x in [20., width - 20.] {
                visual.simulate_mouse_down(point(px(x), px(20.)), MouseButton::Left, Default::default());
                let start = point(px(x + 10.), px(25.));
                visual.simulate_mouse_move(start, MouseButton::Left, Default::default());
                assert_path_drag_preview_at(&mut visual, start);
                for cursor in [point(px(350.), px(150.)), point(px(80.), px(200.))] {
                    visual.simulate_mouse_move(cursor, MouseButton::Left, Default::default());
                    assert_path_drag_preview_at(&mut visual, cursor);
                }
                visual.update(|window, cx| assert!(cx.stop_active_drag(window)));
                visual.simulate_mouse_up(point(px(80.), px(200.)), MouseButton::Left, Default::default());
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
        }
    }
}

#[gpui::test]
fn file_tree_native_drags_keep_origin_and_do_not_click_on_drop_or_cancel(cx: &mut TestAppContext) {
    use gpui::{MouseButton, point, px};
    cx.update(gpui_shell::init);
    let clicks = Rc::new(Cell::new(0));
    let calls = clicks.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("clicked", move |_| {
            calls.set(calls.get() + 1);
            Ok(HostValue::Null)
        })
    ).unwrap());
    let script = r#"
import { View, div } from "gpui-kit";
import { clicked } from "fixture";
export default class extends View {
  render() {
    const paths = ["/work/中文 O'Brien/file name.txt", "/work/a folder"];
    const row = div().id("source").w(180).h(80)
      .drag_paths(paths, {on: "server"}).on_click(() => clicked());
    paths[0] = "/changed/after/render";
    return row;
  }
}"#;
    for source in [None, Some("server")] {
        let fixture = Fixture::new(&script.replace("on: \"server\"", if source.is_some() { "on: \"server\"" } else { "on: null" }));
        let runtime = ShellRuntime::new_isolated().unwrap();
        let drops = Rc::new(RefCell::new(Vec::new()));
        let observed = drops.clone();
        let window = cx.add_window(|window, cx| PathDropTarget {
            root: runtime.try_load(&fixture.0, window, cx).unwrap(), drops: observed,
        });
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        let before = clicks.get();
        visual.simulate_click(point(px(20.), px(20.)), Default::default());
        assert_eq!(clicks.get(), before + 1, "a normal click still opens the row");
        for attempt in 0..3 {
            visual.simulate_mouse_down(point(px(20.), px(20.)), MouseButton::Left, Default::default());
            visual.simulate_mouse_move(point(px(35.), px(35.)), MouseButton::Left, Default::default());
            assert_path_drag_preview_at(&mut visual, point(px(35.), px(35.)));
            if attempt == 2 { visual.update(|window, cx| assert!(cx.stop_active_drag(window))); }
            let end = if attempt == 1 { point(px(500.), px(400.)) } else { point(px(250.), px(150.)) };
            visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
            visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
            visual.run_until_parked();
            visual.update(|_, cx| assert!(!cx.has_active_drag()));
        }
        assert_eq!(clicks.get(), before + 1, "dragging never previews or expands the row");
        assert_eq!(*drops.borrow(), vec![gpui_shell::DraggedPaths {
            paths: vec!["/work/中文 O'Brien/file name.txt".into(), "/work/a folder".into()],
            on: source.map(str::to_owned),
        }], "only the completed drop delivers the original snapshot");
    }
}

#[gpui::test]
fn files_preview_header_drags_the_open_file_without_dragging_the_editor_or_back_button(cx: &mut TestAppContext) {
    use gpui::{MouseButton, point, px};
    cx.update(gpui_shell::init);
    for source in [None, Some("server")] {
        let (root, path) = ("/work/中文 O'Brien", "scripts/a file.txt");
        let mut module = HostModule::new("herdr");
        for (name, value) in [
            ("spaces", json!([{"id":"s1", "active":true, "folder":root, "machine":source}])),
            ("panes", json!([])), ("focused", Value::Null),
            ("theme", serde_json::to_value(&*crate::settings::theme()).unwrap()),
            ("settings", json!({})), ("window", json!({"platform":std::env::consts::OS})),
        ] {
            module = module.function(name, move |_| Ok(to_host(&value)));
        }
        for name in ["exec", "readTextFile", "watchFolder"] {
            module = module.function(name, move |_| panic!("unexpected I/O while dragging a preview: {name}"));
        }
        gpui_shell::policy::set_default(Policy::new().with_host_module(module).unwrap());
        let fixture = Fixture::new(&format!(r#"
import Files from "./files.js";
import {{ EditorState }} from "gpui-base";
export default class extends Files {{
  init() {{
    const {{ root, on, path }} = {};
    this.root = root; this.on = on; this.file = path;
    this.context = {{key: JSON.stringify({{cwd: root, on, space: "s1"}})}};
    this.entries = new Map(); this.folders = new Set(); this.marks = {{}};
    this.code = EditorState.new({{value: "select this text", readonly: true, language: "txt"}});
  }}
}}"#, json!({"root":root, "on":source, "path":path})));
        std::fs::write(fixture.0.join("files.js"), include_str!("../../defaults/mods/files/main.js")).unwrap();
        let runtime = ShellRuntime::new_isolated().unwrap();
        let drops = Rc::new(RefCell::new(Vec::new()));
        let observed = drops.clone();
        let window = cx.add_window(|window, cx| PathDropTarget {
            root: runtime.try_load(&fixture.0, window, cx).unwrap(), drops: observed,
        });
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        // The editor is selectable content, not a file drag source.
        visual.simulate_mouse_down(point(px(30.), px(65.)), MouseButton::Left, Default::default());
        visual.simulate_mouse_move(point(px(100.), px(65.)), MouseButton::Left, Default::default());
        visual.update(|_, cx| assert!(!cx.has_active_drag()));
        visual.simulate_mouse_up(point(px(100.), px(65.)), MouseButton::Left, Default::default());
        visual.simulate_mouse_down(point(px(360.), px(20.)), MouseButton::Left, Default::default());
        visual.simulate_mouse_move(point(px(375.), px(25.)), MouseButton::Left, Default::default());
        assert_path_drag_preview_at(&mut visual, point(px(375.), px(25.)));
        visual.simulate_mouse_move(point(px(250.), px(150.)), MouseButton::Left, Default::default());
        assert_path_drag_preview_at(&mut visual, point(px(250.), px(150.)));
        visual.simulate_mouse_up(point(px(250.), px(150.)), MouseButton::Left, Default::default());
        visual.run_until_parked();
        assert_eq!(*drops.borrow(), vec![gpui_shell::DraggedPaths {
            paths: vec![format!("{root}/{path}")], on: source.map(str::to_owned),
        }]);
        // Going back removes the file-path drag source without starting another drop.
        visual.simulate_click(point(px(24.), px(20.)), Default::default());
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        visual.simulate_mouse_down(point(px(250.), px(20.)), MouseButton::Left, Default::default());
        visual.simulate_mouse_move(point(px(270.), px(25.)), MouseButton::Left, Default::default());
        visual.update(|_, cx| assert!(!cx.has_active_drag(), "the folder label is not a file drag source"));
        visual.simulate_mouse_up(point(px(270.), px(25.)), MouseButton::Left, Default::default());
        assert_eq!(drops.borrow().len(), 1);
    }
}

#[gpui::test]
fn mod_run_rejects_unknown_arguments_before_queueing_a_command(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { run } from "herdr";
import { note } from "fixture";
export default class extends View {
  init() {
    for (const [name, args] of [["space.close", {space: "X", confirmed: true}], ["terminal.kill", {id: "X"}]]) {
      try { run(name, args); note("accepted invalid command"); }
      catch (error) { note(String(error)); }
    }
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "test".into(), props: Value::Null }, 0)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let _root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("space.close: unknown argument(s): space; valid arguments: id, confirmed"), "{}", seen[0]);
    assert!(seen[1].contains("terminal.kill: unknown argument(s): id; valid arguments: terminal"), "{}", seen[1]);
}

#[gpui::test]
fn focus_command_lets_a_mod_take_arrow_keys(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { nextCommand } from "herdr";
import { note } from "fixture";
export default class extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") { this.handle.focus(); note(command); }
        cx.notify();
      }
    });
  }
  render() {
    return div().size_full().track_focus(this.handle)
      .on_key_down((e, cx) => { note(e.key); cx.stop_propagation(); });
  }
}"#);
    let notes = Rc::new(RefCell::new(Vec::new()));
    let calls = notes.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "focus-test".into(), props: Value::Null }, 42)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.run_until_parked();
    set_shown_copies([42].into());
    assert!(send_command_to(42, "focus"));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.simulate_keystrokes("down up");
    visual.run_until_parked();
    assert_eq!(*notes.borrow(), vec!["focus", "down", "up"]);
    set_shown_copies(Default::default());
}

#[gpui::test]
fn close_is_once_and_awaits_its_promise(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  close(cx) {
    note("start");
    return Promise.resolve().then(() => { note("done"); cx.notify(); });
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        let deadline = Instant::now() + Duration::from_millis(250);
        runtime.close(&root, deadline, window, cx).detach();
        runtime.close(&root, deadline, window, cx).detach();
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), ["start", "done"]);
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn close_starts_native_work_before_releasing_the_copy(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { start } from "fixture";
export default class extends View {
  close() { start(); }
  render() { return div(); }
}"#);
    let started = Rc::new(Cell::new(false));
    let call = started.clone();
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let complete = completed.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("start", move |_| {
            call.set(true);
            let complete = complete.clone();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                complete.store(true, Ordering::SeqCst);
                let _ = tx.send_blocking(());
            });
            Ok(async move { let _ = rx.recv().await; Ok(HostValue::Null) })
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| runtime.close(&root, Instant::now() + Duration::from_millis(250), window, cx).detach()).unwrap();
    drop(root);
    cx.run_until_parked();
    assert!(started.get());
    for _ in 0..100 {
        if completed.load(Ordering::SeqCst) { break; }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(completed.load(Ordering::SeqCst));
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn final_close_pumps_native_awaits_before_the_window_goes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait, note } from "fixture";
export default class extends View {
  async close(cx) { await cx.sleep(1); await wait(); note(); await wait(); note(); }
  render() { return div(); }
}"#);
    let completed = Rc::new(Cell::new(0));
    let calls = completed.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture")
            .async_function("wait", |_| Ok(async {
                std::thread::sleep(Duration::from_millis(10));
                Ok(HostValue::Null)
            }))
            .function("note", move |_| { calls.set(calls.get() + 1); Ok(HostValue::Null) })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        // Each native await runs on its own thread; on a loaded runner that took up to 205ms for the
        // whole close. The deadline only bounds a hang, and the pump returns as soon as both finish.
        runtime.close_now(&root, Instant::now() + Duration::from_secs(5), window, cx);
        assert_eq!(completed.get(), 2, "both native awaits must finish before window destruction");
        window.remove_window();
    }).unwrap();
}

#[gpui::test]
fn final_close_stops_waiting_at_its_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait } from "fixture";
export default class extends View {
  async close() { await wait(); }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("wait", |_| Ok(std::future::pending::<gpui_shell::HostResult>()))
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| {
        runtime.close_now(&root, start + Duration::from_millis(15), window, cx);
        window.remove_window();
    }).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
}

#[gpui::test]
fn close_interrupts_a_synchronous_loop_at_the_shared_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
export default class extends View {
  close() { while (true) {} }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| runtime.close(&root, start + Duration::from_millis(15), window, cx).detach()).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    cx.run_until_parked();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

use super::*;
use gpui::{IntoElement, Render, TestAppContext, VisualTestContext};
use std::ops::Deref;
use herdr_shell::{ShellRoot, ShellRuntime, policy::Policy};
use std::{cell::Cell, rc::Rc, sync::{Arc, atomic::{AtomicU64, Ordering}}, time::Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = crate::test_tmp().join(format!("herdr-lifecycle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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
    cx.update(herdr_shell::init);
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
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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


/// Both kinds of scroll handle say where their area is: how far it has
/// scrolled (counted up from the start), how big its content is and how much
/// of it is on screen.
#[gpui::test]
fn scroll_handles_report_offset_content_and_viewport(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { ScrollHandle, VirtualListScrollHandle, v_virtual_list } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  init(_, cx) {
    this.plain = ScrollHandle.new();
    this.rows = VirtualListScrollHandle.new();
    this.focus = cx.focus_handle();
    this.focus.focus();
  }
  render() {
    return div().size_full().track_focus(this.focus)
      .on_key_down((e, cx) => {
        if (e.key === "e") {
          this.plain.scroll_to_item(4);
          this.rows.scroll_to_bottom();
        }
        if (e.key === "r") for (const handle of [this.plain, this.rows]) {
          const offset = handle.offset(), content = handle.content_size(), viewport = handle.viewport_size();
          note(offset.x, offset.y, content.width, content.height, viewport.width, viewport.height);
        }
        cx.notify();
      })
      .child(div().id("plain").flex().flex_col().w(40).h(120).overflow_y_scroll().track_scroll(this.plain)
        .children([0, 1, 2, 3, 4].map(i => div().id("item-" + i).w(40).h(40).flex_none().child(String(i)))))
      .child(div().w(40).h(100).child(v_virtual_list("rows", 50, 20, (i) => String(i),
        (range) => Array.from({ length: range.end - range.start }, (_, i) => div().h(20).child(String(range.start + i))))
        .track_scroll(this.rows)));
  }
}"#);
    let notes = Rc::new(RefCell::new(Vec::new()));
    let calls = notes.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push([a.number(0)?, a.number(1)?, a.number(2)?, a.number(3)?, a.number(4)?, a.number(5)?]);
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    let mut read = |keys: &str| {
        notes.borrow_mut().clear();
        visual.simulate_keystrokes(keys);
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        notes.borrow().clone()
    };
    read("");
    // [offset.x, offset.y, content width, content height, viewport width, viewport height]
    assert_eq!(read("r"), vec![[0., 0., 40., 200., 40., 120.], [0., 0., 40., 1000., 40., 100.]], "at the start");
    read("e");
    assert_eq!(read("r"), vec![[0., 80., 40., 200., 40., 120.], [0., 900., 40., 1000., 40., 100.]], "at the end");
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}

#[gpui::test]
fn plain_mod_scroll_handles_reveal_children_once_without_moving_visible_items(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    for (overflow, horizontal) in [("overflow_x_scroll", true), ("overflow_x_scrollbar", true), ("overflow_y_scroll", false), ("overflow_y_scrollbar", false)] {
        let source = r#"
import { View, div } from "gpui-kit";
import { ScrollHandle } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  init(_, cx) {
    this.scroll = ScrollHandle.new();
    this.scroll.scroll_to_item(4);
    this.focus = cx.focus_handle();
    this.focus.focus();
  }
  render() {
    return div().size_full().track_focus(this.focus)
      .on_key_down((e, cx) => {
        this.scroll.scroll_to_item(Number(e.key));
        cx.notify();
      })
      .child(div().id("area").flex().DIRECTION().w(WIDTH).h(HEIGHT).OVERFLOW().track_scroll(this.scroll).on_scroll_wheel(() => {})
        .children([0, 1, 2, 3, 4].map(i => div().id("item-" + i).w(40).h(40).flex_none()
          .on_click(() => note(i)).child(String(i)))));
  }
}"#.replace("OVERFLOW", overflow)
            .replace("DIRECTION", if horizontal { "flex_row" } else { "flex_col" })
            .replace("WIDTH", if horizontal { "120" } else { "40" })
            .replace("HEIGHT", if horizontal { "40" } else { "120" });
        let fixture = Fixture::new(&source);
        let notes = Rc::new(RefCell::new(Vec::new()));
        let calls = notes.clone();
        herdr_shell::policy::set_default(Policy::new().with_host_module(
            HostModule::new("fixture").function("note", move |a| {
                calls.borrow_mut().push(a.number(0)? as usize);
                Ok(HostValue::Null)
            })
        ).unwrap());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        let position = |along| if horizontal { gpui::point(gpui::px(along), gpui::px(20.)) } else { gpui::point(gpui::px(20.), gpui::px(along)) };
        visual.simulate_click(position(100.), Default::default());
        assert_eq!(*notes.borrow(), vec![4], "{overflow}: the last child must be visible");
        visual.simulate_keystrokes("3");
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(10.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2], "{overflow}: revealing a visible child must keep the offset");
        visual.simulate_keystrokes("0");
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(10.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2, 0], "{overflow}: the first child must be visible again");
        let delta = if horizontal { gpui::point(gpui::px(-80.), gpui::px(0.)) } else { gpui::point(gpui::px(0.), gpui::px(-80.)) };
        visual.simulate_mouse_move(position(20.), None, Default::default());
        visual.simulate_event(gpui::ScrollEvent {
            position: position(20.),
            delta: gpui::ScrollDelta::Pixels(delta),
            ..Default::default()
        });
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_click(position(100.), Default::default());
        assert_eq!(*notes.borrow(), vec![4, 2, 0, 4], "{overflow}: manual scrolling must work after a consumed request");
        visual.update(|window, _| window.remove_window());
        visual.run_until_parked();
    }
}

/// `scroll_to_bottom` means the end of the list as it is drawn next: asked
/// before the first frame, or in the handler that adds rows, it still lands on
/// the last row.
#[gpui::test]
fn a_virtual_list_scrolls_to_the_bottom_of_the_rows_it_draws_next(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { VirtualListScrollHandle, v_virtual_list } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  init(_, cx) {
    this.count = 50;
    this.rows = VirtualListScrollHandle.new();
    this.rows.scroll_to_bottom();
    this.focus = cx.focus_handle();
    this.focus.focus();
  }
  render() {
    return div().size_full().track_focus(this.focus)
      .on_key_down((e, cx) => {
        this.count += 10;
        this.rows.scroll_to_bottom();
        cx.notify();
      })
      .child(div().w(40).h(100).child(v_virtual_list("rows", this.count, 20, (i) => String(i),
        (range) => {
          note(range.end);
          return Array.from({ length: range.end - range.start }, (_, i) => div().h(20).child(String(range.start + i)));
        })
        .track_scroll(this.rows)));
  }
}"#);
    let drawn = Rc::new(Cell::new(0));
    let last = drawn.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            last.set(a.number(0)? as usize);
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    let settle = |visual: &mut VisualTestContext| {
        for _ in 0..3 {
            visual.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            });
            visual.run_until_parked();
        }
        drawn.get()
    };
    assert_eq!(settle(&mut visual), 50, "asked before the first frame: the last row is drawn");
    visual.simulate_keystrokes("a");
    assert_eq!(settle(&mut visual), 60, "asked in the handler that added rows: the new last row is drawn");
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}

#[gpui::test]
fn external_file_drops_reach_mod_elements(cx: &mut TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent, InputEvent};
    cx.update(herdr_shell::init);
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
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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
        // Typed last, as before dragging a screenshot in: no mouse move reaches the window.
        window.dispatch_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("a").unwrap(),
            is_held: false,
            prefer_character_input: false,
        }.to_platform_input(), cx);
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
    drops: Rc<RefCell<Vec<herdr_shell::DraggedPaths>>>,
}
impl Render for PathDropTarget {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{InteractiveElement, ParentElement, Styled};
        let drops = self.drops.clone();
        gpui::div().w(gpui::px(400.)).h(gpui::px(300.))
            .on_drop(move |paths: &herdr_shell::DraggedPaths, _, _| drops.borrow_mut().push(paths.clone()))
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
    cx.update(herdr_shell::init);
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
    cx.update(herdr_shell::init);
    let clicks = Rc::new(Cell::new(0));
    let calls = clicks.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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
        assert_eq!(*drops.borrow(), vec![herdr_shell::DraggedPaths {
            paths: vec!["/work/中文 O'Brien/file name.txt".into(), "/work/a folder".into()],
            on: source.map(str::to_owned),
        }], "only the completed drop delivers the original snapshot");
    }
}

#[gpui::test]
fn file_drags_from_other_apps_report_the_held_button(cx: &mut TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent, InputEvent};
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().id("row").size_full().on_mouse_move((e) => note(e.pressed_button ?? "none"));
  }
}"#);
    let log = Rc::new(RefCell::new(Vec::new()));
    let calls = log.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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
        window.dispatch_event(FileDropEvent::Entered {
            position: gpui::point(gpui::px(20.), gpui::px(20.)),
            paths: ExternalPaths([PathBuf::from("/var/tmp/notes.md")].into_iter().collect()),
        }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
        window.dispatch_event(FileDropEvent::Pending { position: gpui::point(gpui::px(30.), gpui::px(25.)) }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
        window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
    });
    visual.run_until_parked();
    let seen = log.borrow().clone();
    assert!(!seen.is_empty(), "the drag's moves should reach the element");
    assert!(seen.iter().all(|b| *b == json!("left")), "every move of a file drag should carry the held button: {seen:?}");
}

#[gpui::test]
fn window_mouse_events_follow_a_drag_off_the_element(cx: &mut TestAppContext) {
    use gpui::{DispatchPhase, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement as _, Styled as _, point, px};

    struct MountedWithCapture(gpui::Entity<ShellRoot>);

    impl Render for MountedWithCapture {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            gpui::div().size_full()
                // Like terminal selection, these listeners can stop events in capture.
                // Register before the mod so an ordinary capture listener there would miss them.
                .child(gpui::canvas(|_, _, _| {}, |_, _, window, _| {
                    window.on_mouse_event(|e: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && e.position.y >= px(30.) {
                            cx.stop_propagation();
                        }
                    });
                    window.on_mouse_event(|e: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && e.position.y >= px(30.) {
                            cx.stop_propagation();
                        }
                    });
                }).absolute())
                .child(self.0.clone())
        }
    }

    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
import { note } from "fixture";
export default class extends View {
  render() {
    const bar = div().id("bar").w(100).h(30).border(3).p(5).flex_none()
      .on_mouse_down("left", () => { this.dragging = true; })
      .on_mouse_up("left", () => { this.dragging = false; note("bar-up"); });
    return v_flex().size_full().pl(40)
      .child(this.done ? bar : bar
        .on_window_mouse_move((e) => {
          note(`move ${e.local_position.x},${e.local_position.y} ${e.pressed_button ?? "none"}`);
          if (this.dragging && e.pressed_button !== "left") {
            this.dragging = false;
            note("cancelled");
          }
        })
        .on_window_mouse_up("left", (e, cx) => {
          note(`up ${e.button} ${e.local_position.x},${e.local_position.y} ${e.bounds.width}x${e.bounds.height}`);
          if (e.local_position.y < 30) {
            cx.stop_propagation();
          } else {
            this.done = true;
            cx.propagate();
            cx.notify();
          }
        })
        .on_window_mouse_up("back", (e) => note(`up ${e.button}`))
        .on_window_mouse_up("forward", (e) => note(`up ${e.button}`)))
      .child(div().id("pane").flex_1().on_mouse_up("left", () => note("pane-up")));
  }
}"#);
    let log = Rc::new(RefCell::new(Vec::<String>::new()));
    let calls = log.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| MountedWithCapture(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.simulate_resize(gpui::size(px(400.), px(400.)));
    let draw = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
    };
    draw(&mut visual);

    let none = Modifiers::default();
    let over_bar = point(px(60.), px(20.));
    let over_pane = point(px(90.), px(300.));

    visual.simulate_mouse_down(over_bar, MouseButton::Left, none);
    visual.simulate_mouse_up(over_bar, MouseButton::Left, none);
    visual.simulate_mouse_down(over_bar, MouseButton::Left, none);
    visual.simulate_mouse_move(over_pane, MouseButton::Left, none);
    // A release outside the window can leave the next move as the only signal.
    visual.simulate_mouse_move(over_pane, None, none);
    visual.simulate_mouse_down(over_bar, MouseButton::Left, none);
    visual.simulate_mouse_up(over_pane, MouseButton::Right, none);
    visual.simulate_mouse_up(over_pane, MouseButton::Navigate(NavigationDirection::Back), none);
    visual.simulate_mouse_up(over_pane, MouseButton::Navigate(NavigationDirection::Forward), none);
    visual.simulate_mouse_up(over_pane, MouseButton::Left, none);
    draw(&mut visual);
    visual.simulate_mouse_move(over_bar, None, none);
    visual.simulate_mouse_up(over_pane, MouseButton::Left, none);
    draw(&mut visual);

    assert_eq!(*log.borrow(), [
        "up left 20,20 100x30", "bar-up",
        "move 50,300 left", "move 50,300 none", "cancelled",
        "up back", "up forward",
        "up left 50,300 100x30",
    ]);
}

#[gpui::test]
fn mod_run_rejects_unknown_arguments_before_queueing_a_command(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
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
    herdr_shell::policy::set_default(Policy::new()
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
    cx.update(herdr_shell::init);
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
    herdr_shell::policy::set_default(Policy::new()
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
fn herdr_checking_is_false_in_a_live_load(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { checking } from "herdr";
import { note } from "fixture";
export default class extends View {
  init() { note(String(checking())); }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    herdr_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "test".into(), props: Value::Null }, 0)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let _root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    assert_eq!(*seen.borrow(), ["false"]);
}

#[gpui::test]
fn herdr_checking_is_true_while_a_check_loads_the_mod(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { checking } from "herdr";
import { note } from "fixture";
export default class extends View {
  init() { note(String(checking())); }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    install_check_policy();
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let _root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    assert_eq!(*seen.borrow(), ["true"]);
}

#[gpui::test]
fn close_is_once_and_awaits_its_promise(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
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
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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
fn mod_socket_writes_reads_and_reports_eof(cx: &mut TestAppContext) {
    use std::{io::{Read, Write}, net::{Shutdown, TcpListener}};

    cx.executor().allow_parking();
    cx.update(herdr_shell::init);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let source = r#"
import { View, div } from "gpui-kit";
import { connect } from "net";
import { note } from "fixture";
export default class extends View {
  init(_, cx) {
    cx.spawn(async () => {
      const socket = await connect("127.0.0.1", __PORT__);
      await socket.write("pingé");
      const bytes = await socket.read(4);
      const eof = await socket.read(4);
      note(bytes instanceof Uint8Array, bytes?.length, bytes?.[0], eof === null);
      socket.close();
    });
  }
  render() { return div(); }
}"#.replace("__PORT__", &listener.local_addr().unwrap().port().to_string());
    let fixture = Fixture::new(&source);
    let seen = Rc::new(RefCell::new(None));
    let calls = seen.clone();
    herdr_shell::policy::set_default(Policy::new().with_capabilities(capabilities())
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.replace(Some((a.boolean(0)?, a.number(1)?, a.number(2)?, a.boolean(3)?)));
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    cx.run_until_parked();

    let (mut peer, _) = listener.accept().unwrap();
    peer.set_nonblocking(false).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    let server = std::thread::spawn(move || {
        let mut request = [0; 6];
        peer.read_exact(&mut request).unwrap();
        assert_eq!(&request, "pingé".as_bytes());
        peer.write_all(&[0xfe]).unwrap();
        peer.shutdown(Shutdown::Write).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while seen.borrow().is_none() && Instant::now() < deadline {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(1));
    }
    server.join().unwrap();
    assert_eq!(*seen.borrow(), Some((true, 1.0, 254.0, true)));
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn reload_releases_a_pending_tcp_read_without_a_close_hook(cx: &mut TestAppContext) {
    use std::io::Read as _;
    use std::net::TcpListener;

    cx.update(herdr_shell::init);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let source = r#"
import { View, div } from "gpui-kit";
import { nextCommand } from "herdr";
import { connect } from "net";
import { noteRead } from "fixture";
export default class extends View {
  init(_, cx) {
    cx.spawn(async () => {
      const socket = await connect("127.0.0.1", __PORT__);
      await nextCommand();
      noteRead();
      await socket.read(1);
    });
  }
  render() { return div(); }
}"#.replace("__PORT__", &listener.local_addr().unwrap().port().to_string());
    let fixture = Fixture::new(&source);
    let reading = Rc::new(Cell::new(false));
    let started = reading.clone();
    herdr_shell::policy::set_default(Policy::new().with_capabilities(capabilities())
        .with_host_module(module(Instance { name: "read-test".into(), props: Value::Null }, 44)).unwrap()
        .with_host_module(HostModule::new("fixture").function("noteRead", move |_| {
            started.set(true);
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    cx.run_until_parked();

    let (mut peer, _) = listener.accept().unwrap();
    peer.set_nonblocking(false).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let peer = std::thread::spawn(move || peer.read(&mut [0]));
    set_shown_copies([44].into());
    assert!(send_command_to(44, "read"));
    cx.run_until_parked();
    assert!(reading.get());

    std::fs::write(fixture.0.join("main.js"), r#"
import { View, div } from "gpui-kit";
export default class extends View { render() { return div(); } }
"#).unwrap();
    window.update(cx, |_, window, cx| runtime.reload_now(&root, window, cx).unwrap()).unwrap();
    cx.run_until_parked();
    assert_eq!(peer.join().unwrap().expect("the old socket is still open"), 0);
    set_shown_copies(Default::default());
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn explicit_host_cancellation_drops_work_and_settles_once(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait, ready, note } from "fixture";
export default class extends View {
  init(_props, cx) {
    cx.spawn(async () => {
      const pending = wait();
      pending.cancel();
      pending.cancel();
      note(await pending);
      const done = ready();
      note(await done);
      done.cancel();
      done.cancel();
    });
  }
  render() { return div(); }
}"#);
    struct Dropped(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); }
    }
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let work = dropped.clone();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture")
            .cancellable_async_function("wait", HostValue::Array(vec![]), move |_| {
                let guard = Dropped(work.clone());
                Ok(async move {
                    let _guard = guard;
                    std::future::pending::<herdr_shell::HostResult>().await
                })
            })
            .cancellable_async_function("ready", HostValue::Null, |_| Ok(async { Ok(HostValue::Number(7.0)) }))
            .function("note", move |a| { calls.borrow_mut().push(a.value(0)?.clone()); Ok(HostValue::Null) })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), [HostValue::Array(vec![]), HostValue::Number(7.0)]);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn close_starts_native_work_before_releasing_the_copy(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { start } from "fixture";
export default class extends View {
  close() { start(); }
  render() { return div(); }
}"#);
    let started = Rc::new(Cell::new(false));
    let call = started.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("start", move |_| {
            call.set(true);
            // Starting native work must not depend on its promise settling.
            Ok(std::future::pending::<herdr_shell::HostResult>())
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| runtime.close(&root, Instant::now() + Duration::from_millis(250), window, cx).detach()).unwrap();
    drop(root);
    cx.run_until_parked();
    assert!(started.get());
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn final_close_pumps_native_awaits_before_the_window_goes(cx: &mut TestAppContext) {
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait, note } from "fixture";
export default class extends View {
  async close(cx) { await cx.sleep(1); await wait(); note(); await wait(); note(); }
  render() { return div(); }
}"#);
    let completed = Rc::new(Cell::new(0));
    let calls = completed.clone();
    herdr_shell::policy::set_default(Policy::new().with_host_module(
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
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait } from "fixture";
export default class extends View {
  async close() { await wait(); }
  render() { return div(); }
}"#);
    herdr_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("wait", |_| Ok(std::future::pending::<herdr_shell::HostResult>()))
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
    cx.update(herdr_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
export default class extends View {
  close() { while (true) {} }
  render() { return div(); }
}"#);
    herdr_shell::policy::set_default(Policy::new());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| runtime.close(&root, start + Duration::from_millis(15), window, cx).detach()).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    cx.run_until_parked();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

// HERDR: source editor surface; read-only state still accepts focus, selection and copy.
use std::rc::Rc;

use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement as _, MouseButton, ParentElement as _,
    Refineable as _, StyleRefinement, Styled as _, div,
};
use gpui_base::input::{Editor, InputBase};

use crate::{
    engine::ShellRuntime,
    entities::EntityHandle,
    materialize::{
        Behavior, Children, StateStyles, warn_unhonoured_a11y, with_active_and_focus, with_hover,
    },
};

pub(in crate::materialize) fn editor(
    runtime: &Rc<ShellRuntime>,
    handle: EntityHandle,
    refinement: StyleRefinement,
    behavior: Behavior,
    states: StateStyles,
    children: Children,
    cx: &mut App,
) -> AnyElement {
    warn_unhonoured_a11y(&behavior, "Editor", &[]);
    let Some(state) = runtime.entities().editor(handle) else {
        tracing::error!("editor handle {handle} is no longer live");
        return div().into_any_element();
    };
    // HERDR: GPUI needs a concrete font family, not CSS's generic "monospace".
    let typography = &gpui_base::Theme::global(cx).tokens.typography;
    let (font_family, font_size) = runtime.entities().editor_font(handle);
    let font_family = font_family
        .map(Into::into)
        .unwrap_or_else(|| typography.mono.clone());
    let font_size = font_size
        .map(crate::value::scaled_px)
        .unwrap_or(typography.mono_md.size);
    let focus_target = state.clone();
    let mut frame = InputBase::new(("gpui-shell-editor", handle))
        .flex()
        .items_start()
        .w_full()
        .h_full()
        .min_h_0()
        .font_family(font_family)
        .text_size(font_size)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            focus_target.update(cx, |state, cx| state.focus(window, cx));
        });
    frame.style().refine(&refinement);
    frame.extend(children);
    let frame = with_hover(frame, &states);
    let frame = with_active_and_focus(frame, &states);
    frame.child(Editor::new(&state)).into_any_element()
}

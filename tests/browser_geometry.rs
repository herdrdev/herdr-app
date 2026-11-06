use gpui::prelude::*;
use gpui::{
    canvas, div, Context, Entity, HitboxBehavior, HitboxId, IntoElement, ParentElement, Render,
    StyleRefinement, Styled, TestAppContext, Window,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Surface {
    hitbox: Rc<Cell<Option<HitboxId>>>,
    renders: Rc<Cell<usize>>,
}
impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let hitbox = self.hitbox.clone();
        canvas(
            move |bounds, window, _| {
                hitbox.set(Some(
                    window.insert_hitbox(bounds, HitboxBehavior::Normal).id,
                ))
            },
            |_, _, _, _| {},
        )
        .size_full()
    }
}
struct Host {
    surface: Entity<Surface>,
    hitbox: Rc<Cell<Option<HitboxId>>>,
    seen: Rc<RefCell<Vec<bool>>>,
    show: bool,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hitbox = self.hitbox.clone();
        let seen = self.seen.clone();
        div()
            .size_full()
            .when(self.show, |root| {
                root.child(
                    self.surface
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
            })
            .child(canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    seen.borrow_mut().push(
                        hitbox
                            .get()
                            .is_some_and(|id| window.prepainted_hitbox(id).is_some()),
                    );
                },
            ))
    }
}

#[gpui::test]
fn native_surface_geometry_survives_cached_frames_but_not_a_hidden_tab(cx: &mut TestAppContext) {
    use gpui::AppContext as _;
    let hitbox = Rc::new(Cell::new(None));
    let renders = Rc::new(Cell::new(0));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_window(|_, cx| Host {
        surface: cx.new(|_| Surface {
            hitbox: hitbox.clone(),
            renders: renders.clone(),
        }),
        hitbox: hitbox.clone(),
        seen: seen.clone(),
        show: true,
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
    };
    draw(cx);
    assert_eq!(seen.borrow().last(), Some(&true));
    let baseline = renders.get();
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    draw(cx);
    assert_eq!(renders.get(), baseline);
    assert_eq!(seen.borrow().last(), Some(&true));
    window
        .update(cx, |host, _, cx| {
            host.show = false;
            cx.notify();
        })
        .unwrap();
    draw(cx);
    assert_eq!(seen.borrow().last(), Some(&false));
    window
        .update(cx, |host, _, cx| {
            host.show = true;
            cx.notify();
        })
        .unwrap();
    draw(cx);
    assert_eq!(seen.borrow().last(), Some(&true));
}

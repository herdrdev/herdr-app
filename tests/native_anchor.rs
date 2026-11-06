//! Native anchors (HERDR patch in vendor/gpui-pre): where something drawn
//! outside GPUI sits in the scene, and whether GPUI painted over it. Native
//! spinners (gpui-kit's `native_spin`) rely on both.

use gpui::{
    Bounds, ContentMask, Edges, Hsla, Quad, ScaledPixels, Scene, Size, black, point,
    transparent_black,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: Size {
            width: ScaledPixels(w),
            height: ScaledPixels(h),
        },
    }
}

fn border(bounds: Bounds<ScaledPixels>, width: f32) -> Quad {
    Quad {
        bounds,
        content_mask: ContentMask {
            bounds: rect(0., 0., 1000., 1000.),
        },
        background: transparent_black().into(),
        border_color: black(),
        border_widths: Edges::all(ScaledPixels(width)),
        ..Default::default()
    }
}

fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        bounds,
        content_mask: ContentMask {
            bounds: rect(0., 0., 1000., 1000.),
        },
        background: color.into(),
        ..Default::default()
    }
}

fn covered(scene: &mut Scene) -> bool {
    let anchor = *scene.anchors().next().unwrap();
    scene.anchor_covered(&anchor)
}

#[test]
fn what_was_painted_before_does_not_cover_it() {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(rect(0., 0., 100., 100.), black()));
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    assert!(!covered(&mut scene));
}

#[test]
fn something_painted_over_it_covers_it() {
    let mut scene = Scene::default();
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    scene.insert_primitive(quad(rect(20., 20., 100., 100.), black()));
    assert!(covered(&mut scene));
}

#[test]
fn something_painted_after_it_elsewhere_does_not() {
    let mut scene = Scene::default();
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    // Touching its edge, and overlapping only the outer device pixel.
    scene.insert_primitive(quad(rect(34., 10., 50., 24.), black()));
    scene.insert_primitive(quad(rect(0., 33., 50., 20.), black()));
    assert!(!covered(&mut scene));
}

#[test]
fn a_fully_transparent_primitive_does_not_cover_it() {
    let mut scene = Scene::default();
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    scene.insert_primitive(quad(rect(0., 0., 100., 100.), transparent_black()));
    assert!(!covered(&mut scene));
}

#[test]
fn a_cached_view_replays_its_anchor() {
    let mut previous = Scene::default();
    previous.insert_anchor(7, rect(10., 10., 24., 24.));
    let mut scene = Scene::default();
    scene.replay(0..previous.len(), &previous);
    assert_eq!(scene.anchors().count(), 1);
    assert_eq!(scene.anchors().next().unwrap().key, 7);
    scene.insert_primitive(quad(rect(0., 0., 100., 100.), black()));
    assert!(covered(&mut scene));
}

#[test]
fn a_box_border_painted_after_its_contents_does_not_cover_them() {
    let mut scene = Scene::default();
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    // GPUI paints a box's border after its children, over the whole box.
    scene.insert_primitive(border(rect(0., 0., 100., 100.), 2.));
    assert!(!covered(&mut scene));
}

#[test]
fn a_border_strip_over_it_covers_it() {
    let mut scene = Scene::default();
    scene.insert_anchor(1, rect(10., 10., 24., 24.));
    scene.insert_primitive(border(rect(20., 0., 100., 100.), 4.));
    assert!(covered(&mut scene));
}

//! Browser policy shared by initial loads and every navigation.

pub fn allows_navigation(input: &str) -> bool {
    url::Url::parse(input).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

pub fn should_show(drawn: bool, unclipped: bool, overlay: bool) -> bool {
    drawn && unclipped && !overlay
}

/// Where overlays cross a native view: rectangles `[x, y, width, height]` in
/// device pixels relative to the view's top left. A native child is drawn
/// above everything GPUI paints, so these parts are cut out of it.
pub fn holes(
    view: &gpui::Bounds<gpui::Pixels>,
    covers: &[gpui::Bounds<gpui::Pixels>],
    scale: f32,
) -> Vec<[i32; 4]> {
    let px = |v: gpui::Pixels| f32::from(v) * scale;
    let (left, top) = (px(view.left()).round(), px(view.top()).round());
    covers
        .iter()
        .filter(|cover| cover.intersects(view))
        .map(|cover| {
            let cut = cover.intersect(view);
            // Grow outward to whole pixels so no overlay pixel stays covered.
            let x0 = (px(cut.left()).floor() - left) as i32;
            let y0 = (px(cut.top()).floor() - top) as i32;
            let x1 = (px(cut.right()).ceil() - left) as i32;
            let y1 = (px(cut.bottom()).ceil() - top) as i32;
            [x0, y0, x1 - x0, y1 - y0]
        })
        .filter(|[_, _, w, h]| *w > 0 && *h > 0)
        .collect()
}

/// The parts of the view outside its clip mask (a scrolled or partly hidden
/// pane), as covers: a native child cannot be clipped by GPUI, so these parts
/// are cut out of it like overlays. Slivers under half a device pixel, from
/// rounding at a mask's edge, are not.
pub fn outside_mask(
    view: &gpui::Bounds<gpui::Pixels>,
    mask: &gpui::Bounds<gpui::Pixels>,
    scale: f32,
) -> Vec<gpui::Bounds<gpui::Pixels>> {
    use gpui::{point, Bounds};
    let (l, t, r, b) = (view.left(), view.top(), view.right(), view.bottom());
    let inner = view.intersect(mask);
    if inner.size.width <= gpui::px(0.) || inner.size.height <= gpui::px(0.) {
        return vec![*view];
    }
    let (il, it, ir, ib) = (inner.left(), inner.top(), inner.right(), inner.bottom());
    [
        Bounds::from_corners(point(l, t), point(r, it)),
        Bounds::from_corners(point(l, ib), point(r, b)),
        Bounds::from_corners(point(l, it), point(il, ib)),
        Bounds::from_corners(point(ir, it), point(r, ib)),
    ]
    .into_iter()
    .filter(|band| {
        f32::from(band.size.width) * scale >= 0.5 && f32::from(band.size.height) * scale >= 0.5
    })
    .collect()
}

/// The overlays leave nothing of the view to show.
pub fn fully_covered(
    view: &gpui::Bounds<gpui::Pixels>,
    covers: &[gpui::Bounds<gpui::Pixels>],
) -> bool {
    covers.iter().any(|cover| {
        cover.left() <= view.left()
            && cover.top() <= view.top()
            && cover.right() >= view.right()
            && cover.bottom() >= view.bottom()
    })
}

#[cfg(test)]
mod tests {
    use super::{fully_covered, holes, outside_mask};
    use gpui::{point, px, size, Bounds};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn a_toast_in_the_corner_cuts_only_its_own_area() {
        let view = rect(100., 50., 800., 600.);
        let toast = rect(700., 550., 300., 80.);
        assert_eq!(holes(&view, &[toast], 1.5), vec![[900, 750, 300, 120]]);
        assert!(!fully_covered(&view, &[toast]));
    }

    #[test]
    fn overlays_elsewhere_leave_the_view_whole() {
        let view = rect(100., 50., 800., 600.);
        assert!(holes(&view, &[rect(0., 0., 90., 40.)], 1.).is_empty());
    }

    #[test]
    fn a_clipped_view_cuts_only_what_its_mask_hides() {
        let view = rect(100., 50., 800., 600.);
        // Its right 200 px and bottom 100 px are outside the pane's mask.
        let bands = outside_mask(&view, &rect(0., 0., 800., 550.), 1.);
        assert_eq!(
            holes(&view, &bands, 1.),
            vec![[0, 500, 800, 100], [700, 0, 100, 500]]
        );
        assert!(!fully_covered(&view, &bands));
        assert!(outside_mask(&view, &rect(0., 0., 2000., 2000.), 1.).is_empty());
        // A rounding sliver at the mask's edge leaves the view whole.
        assert!(outside_mask(&view, &rect(100.2, 50., 800., 600.), 1.5).is_empty());
        // Nothing of it inside the mask: all of it goes.
        assert!(fully_covered(
            &view,
            &outside_mask(&view, &rect(1000., 0., 100., 100.), 1.)
        ));
    }

    #[test]
    fn a_dialog_backdrop_covers_everything() {
        let view = rect(100., 50., 800., 600.);
        assert!(fully_covered(&view, &[rect(0., 0., 1200., 900.)]));
    }
}

/// Compare integer native-window pixels with GPUI's rendered clip mask.
pub fn is_unclipped_at_scale(
    bounds: &gpui::Bounds<gpui::Pixels>,
    mask: &gpui::Bounds<gpui::Pixels>,
    scale: f32,
) -> bool {
    use gpui::{point, size, Bounds};
    if !scale.is_finite()
        || scale <= 0.
        || mask.size.width <= gpui::px(0.)
        || mask.size.height <= gpui::px(0.)
    {
        return false;
    }
    // Wry rounds the native origin and size separately, using f64 logical
    // coordinates. GPUI's Window::cover_bounds instead floors the mask's near
    // edges and ceils its far edges in f32 device space. Compare those actual
    // pixel regions, not the unsnapped mask; no tolerance or extra pixels.
    let native_pixel =
        |value: gpui::Pixels| (f64::from(f32::from(value)) * f64::from(scale)).round();
    let native = Bounds::new(
        point(native_pixel(bounds.left()), native_pixel(bounds.top())),
        size(
            native_pixel(bounds.size.width),
            native_pixel(bounds.size.height),
        ),
    );
    let mask = Bounds::from_corners(
        point(
            f64::from((f32::from(mask.left()) * scale).floor()),
            f64::from((f32::from(mask.top()) * scale).floor()),
        ),
        point(
            f64::from((f32::from(mask.right()) * scale).ceil()),
            f64::from((f32::from(mask.bottom()) * scale).ceil()),
        ),
    );
    is_unclipped(&native, &mask)
}

/// A native child cannot be clipped by GPUI, so its whole surface must fit.
pub fn is_unclipped<T>(bounds: &gpui::Bounds<T>, mask: &gpui::Bounds<T>) -> bool
where
    T: Copy + std::fmt::Debug + Default + PartialOrd + std::ops::Add<Output = T>,
{
    // Intersecting reconstructs the size by subtraction, which can change a
    // fractional f32 dimension even when the mask contains the whole surface.
    bounds.size.width > T::default()
        && bounds.size.height > T::default()
        && bounds.left() >= mask.left()
        && bounds.top() >= mask.top()
        && bounds.right() <= mask.right()
        && bounds.bottom() <= mask.bottom()
}

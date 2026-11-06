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

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

/// A native child cannot be clipped by GPUI, so its whole surface must fit.
pub fn is_unclipped(
    bounds: &gpui::Bounds<gpui::Pixels>,
    mask: &gpui::Bounds<gpui::Pixels>,
) -> bool {
    // Intersecting reconstructs the size by subtraction, which can change a
    // fractional f32 dimension even when the mask contains the whole surface.
    bounds.size.width > gpui::px(0.)
        && bounds.size.height > gpui::px(0.)
        && bounds.left() >= mask.left()
        && bounds.top() >= mask.top()
        && bounds.right() <= mask.right()
        && bounds.bottom() <= mask.bottom()
}

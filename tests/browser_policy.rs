#[path = "../src/browser/policy.rs"]
mod policy;

#[test]
fn pages_can_use_http_and_https_without_native_app_access() {
    for url in [
        "http://localhost:19432/",
        "http://127.0.0.1:45693/?token=abc",
        "https://example.com/review#comment",
    ] {
        assert!(policy::allows_navigation(url), "{url}");
    }
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,hello",
        "herdr:terminal.kill",
        "about:blank",
        "https://",
        "not a url",
        "https://user:password@example.com/",
    ] {
        assert!(!policy::allows_navigation(url), "{url}");
    }
}

#[test]
fn native_pages_show_only_when_drawn_unclipped_without_overlays() {
    assert!(policy::should_show(true, true, false));
    assert!(!policy::should_show(false, true, false));
    assert!(!policy::should_show(true, false, false));
    assert!(!policy::should_show(true, true, true));
}

#[test]
fn fractional_native_surfaces_show_when_the_mask_contains_them() {
    use gpui::{point, px, size, Bounds};
    let bounds = Bounds::new(
        point(px(2094.6667), px(91.3333)),
        size(px(464.6667), px(1351.3333)),
    );
    let mask = Bounds::new(point(px(2000.), px(60.)), size(px(600.), px(1600.)));
    assert!(policy::is_unclipped(&bounds, &mask));
    assert!(policy::is_unclipped(&bounds, &bounds));
}

#[test]
fn fractional_scale_uses_the_rendered_mask_not_its_unsnapped_edges() {
    use gpui::{point, px, size, Bounds};
    // Captured from the native Windows pane at 150% display scaling.
    let bounds = Bounds::new(
        point(px(277.33334), px(118.666664)),
        size(px(994.), px(664.6667)),
    );
    let mask = Bounds::new(point(px(277.66666), px(51.)), size(px(993.3334), px(732.)));
    assert!(policy::is_unclipped_at_scale(&bounds, &mask, 1.5));
    for edge in 0..4 {
        let mut clipped = mask;
        match edge {
            0 => clipped.origin.x += px(2.),
            1 => clipped.origin.y = bounds.top() + px(2.),
            2 => clipped.size.width -= px(2.),
            _ => clipped.size.height = bounds.bottom() - mask.top() - px(2.),
        }
        assert!(
            !policy::is_unclipped_at_scale(&bounds, &clipped, 1.5),
            "edge {edge}"
        );
    }
}

#[test]
fn native_origin_and_size_round_independently_before_checking_the_mask() {
    use gpui::{point, px, size, Bounds};
    let bounds = Bounds::new(point(px(0.5), px(0.5)), size(px(10.5), px(10.5)));
    let mask = Bounds::new(point(px(0.), px(0.)), size(px(11.), px(11.)));
    // The native child is at (1, 1), sized (11, 11): its far edge is 12,
    // even though the unrounded logical far edge is only 11.
    assert!(!policy::is_unclipped_at_scale(&bounds, &mask, 1.));
    let mask = Bounds::new(mask.origin, size(px(12.), px(12.)));
    assert!(policy::is_unclipped_at_scale(&bounds, &mask, 1.));
}

#[test]
fn native_surfaces_hide_for_clipping_at_every_edge_or_empty_size() {
    use gpui::{point, px, size, Bounds};
    let bounds = Bounds::new(
        point(px(1000.6667), px(43.3333)),
        size(px(464.6667), px(680.6667)),
    );
    for edge in 0..4 {
        let mut mask = bounds;
        match edge {
            0 => mask.origin.x += px(0.25),
            1 => mask.origin.y += px(0.25),
            2 => mask.size.width -= px(0.25),
            _ => mask.size.height -= px(0.25),
        }
        assert!(!policy::is_unclipped(&bounds, &mask), "edge {edge}");
    }
    for dimensions in [
        size(px(0.), px(10.)),
        size(px(10.), px(0.)),
        size(px(-1.), px(10.)),
        size(px(10.), px(-1.)),
    ] {
        let empty = Bounds::new(bounds.origin, dimensions);
        assert!(!policy::is_unclipped(&empty, &bounds));
    }
}

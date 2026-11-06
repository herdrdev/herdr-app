#[path = "../src/browser/policy.rs"]
mod policy;

#[test]
fn pages_can_use_http_and_https_without_native_app_access() {
    for url in ["http://localhost:19432/", "http://127.0.0.1:45693/?token=abc", "https://example.com/review#comment"] {
        assert!(policy::allows_navigation(url), "{url}");
    }
    for url in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,hello", "herdr:terminal.kill", "about:blank", "https://", "not a url", "https://user:password@example.com/"] {
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

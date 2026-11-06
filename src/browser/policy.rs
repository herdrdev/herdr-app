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

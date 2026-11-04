//! The app icon's badge: `setBadge` in mods. macOS draws it on the dock icon; other
//! systems have no badge here yet, so it does nothing there. Only the app's own
//! process can badge its icon: the Dock ignores a label set from outside.

use gpui_shell::HostValue;

/// What a mod's `setBadge(value)` shows: a string as given, a number as written
/// (0 shows nothing), null or nothing clears. An empty label clears.
pub fn label(value: Option<&HostValue>) -> Result<String, &'static str> {
    match value {
        None | Some(HostValue::Null) => Ok(String::new()),
        Some(HostValue::Str(s)) => Ok(s.clone()),
        Some(HostValue::Number(n)) if !n.is_finite() => Err("setBadge: the number must be finite"),
        Some(HostValue::Number(n)) if *n == 0.0 => Ok(String::new()),
        Some(HostValue::Number(n)) => Ok(n.to_string()),
        Some(_) => Err("setBadge takes a string, a number or null"),
    }
}

/// Shows `label` on the dock icon until the app quits or another call replaces it;
/// an empty label clears it. AppKit only lets the main thread touch the dock tile.
#[cfg(target_os = "macos")]
pub fn set(label: &str) -> Result<(), &'static str> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    let mtm = MainThreadMarker::new().ok_or("setBadge: not on the main thread")?;
    let text = (!label.is_empty()).then(|| NSString::from_str(label));
    NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(text.as_deref());
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set(_label: &str) -> Result<(), &'static str> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_show_as_given() {
        assert_eq!(label(Some(&HostValue::Str("3".into()))), Ok("3".into()));
        assert_eq!(label(Some(&HostValue::Str("!".into()))), Ok("!".into()));
    }

    #[test]
    fn numbers_show_as_written_and_zero_clears() {
        assert_eq!(label(Some(&HostValue::Number(7.0))), Ok("7".into()));
        assert_eq!(label(Some(&HostValue::Number(2.5))), Ok("2.5".into()));
        assert_eq!(label(Some(&HostValue::Number(0.0))), Ok(String::new()));
    }

    #[test]
    fn null_nothing_and_empty_clear() {
        assert_eq!(label(None), Ok(String::new()));
        assert_eq!(label(Some(&HostValue::Null)), Ok(String::new()));
        assert_eq!(label(Some(&HostValue::Str(String::new()))), Ok(String::new()));
    }

    #[test]
    fn other_values_are_refused() {
        assert!(label(Some(&HostValue::Bool(true))).is_err());
        assert!(label(Some(&HostValue::Number(f64::NAN))).is_err());
        assert!(label(Some(&HostValue::Array(vec![]))).is_err());
    }
}

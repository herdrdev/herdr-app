use gpui::*;

actions!(herdr, [Quit, Hide, HideOthers, ShowAll, Settings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Settings, _| crate::mods::request(crate::mods::Request::Run { name: "app.settings".into(), args: serde_json::Value::Null }));
    cx.set_menus([Menu::new("Herdr").items([
        MenuItem::action("Settings", Settings),
        MenuItem::separator(),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("Hide Herdr", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit Herdr", Quit),
    ])]);
    if let Some(path) = icon_path(std::env::var_os("HERDR_APP_ICON"))
        && !gpui_macos::set_app_icon(&path)
    {
        tracing::warn!("HERDR_APP_ICON {}: not an image macOS can open", path.display());
    }
}

/// `HERDR_APP_ICON`: an image (PNG, ICNS, ...) shown as the Dock icon for this
/// run, so side-by-side windows with their own `HERDR_APP_HOME` can be told
/// apart. Empty or unset keeps the app's icon.
fn icon_path(value: Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    value.filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}

pub fn action(name: &str) -> Option<Box<dyn Action>> {
    match name {
        "app.quit" => Some(Box::new(Quit)),
        "app.hide" => Some(Box::new(Hide)),
        "app.hide_others" => Some(Box::new(HideOthers)),
        "app.show_all" => Some(Box::new(ShowAll)),
        "app.settings" => Some(Box::new(Settings)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's glob import brings its own `test` attribute.
    use super::icon_path;

    #[test]
    fn icon_path_skips_unset_and_empty() {
        assert_eq!(icon_path(None), None);
        assert_eq!(icon_path(Some("".into())), None);
        assert_eq!(icon_path(Some("/tmp/icon.png".into())), Some("/tmp/icon.png".into()));
    }
}

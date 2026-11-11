use gpui::*;

actions!(herdr, [Quit, Hide, HideOthers, ShowAll, Settings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Settings, _| crate::mods::request(crate::mods::Request::Run { name: "app.settings".into(), args: serde_json::Value::Null }));
    cx.set_menus([Menu::new("Herdr").items([
        MenuItem::action(crate::i18n::text("macos-settings"), Settings),
        MenuItem::separator(),
        MenuItem::os_submenu(crate::i18n::text("macos-services"), SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(crate::i18n::text("macos-hide"), Hide),
        MenuItem::action(crate::i18n::text("macos-hide-others"), HideOthers),
        MenuItem::action(crate::i18n::text("macos-show-all"), ShowAll),
        MenuItem::separator(),
        MenuItem::action(crate::i18n::text("macos-quit"), Quit),
    ])]);
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

use gpui::*;
use gpui_shell::action::ShellAction;

use crate::i18n::text;

actions!(herdr, [Quit, Hide, HideOthers, ShowAll, Settings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Settings, _| crate::mods::request(crate::mods::Request::Run { name: "app.settings".into(), args: serde_json::Value::Null }));
    refresh_menus(cx);
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

/// Builds the menu bar again. macOS takes each item's key from the keymap
/// when the bar is built, so this runs after keys.json loads, and when the
/// layouts or the shown layout change.
pub fn refresh_menus(cx: &mut App) {
    let shown = crate::mods::shared("layout").and_then(|v| v.as_str().map(str::to_owned));
    cx.set_menus(menus(&crate::window::layout_names(), shown.as_deref()));
}

/// An item that runs a command as keys.json names it (`pane.split:right`).
/// It is the same action as the key's, so the item shows the person's key.
fn command(title: &str, command: &str) -> MenuItem {
    MenuItem::Action {
        name: title.to_owned().into(),
        action: action(command).unwrap_or_else(|| Box::new(ShellAction::new(command.to_owned()))),
        os_action: None,
        checked: false,
        disabled: false,
    }
}

/// GPUI registers the menu named "Window" as the app's windows menu, so
/// macOS adds its own window items there (tiling, the window list).
fn menus(layouts: &[String], shown: Option<&str>) -> Vec<Menu> {
    let layout_items = layouts
        .iter()
        .map(|name| command(name, &format!("layout.switch:{name}")).checked(Some(name.as_str()) == shown))
        .chain([MenuItem::separator(), command(&text("macos-next-layout"), "layout.switch")]);
    vec![
        Menu::new("Herdr").items([
            command(&text("macos-what-s-new"), "app.whats_new"),
            command(&text("macos-check-for-updates"), "app.settings:updates"),
            MenuItem::separator(),
            command(&text("macos-settings"), "app.settings"),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            command(&text("macos-hide"), "app.hide"),
            command(&text("macos-hide-others"), "app.hide_others"),
            command(&text("macos-show-all"), "app.show_all"),
            MenuItem::separator(),
            command(&text("macos-quit"), "app.quit"),
        ]),
        Menu::new("File").items([
            command(&text("macos-new-tab"), "tab.new"),
            command(&text("macos-new-space"), "space.new"),
            MenuItem::separator(),
            command(&text("macos-split-right"), "pane.split:right"),
            command(&text("macos-split-down"), "pane.split:down"),
            MenuItem::separator(),
            command(&text("macos-rename-tab"), "tab.rename"),
            command(&text("macos-rename-space"), "space.rename"),
            MenuItem::separator(),
            command(&text("macos-close-pane"), "pane.close"),
            command(&text("macos-close-tab"), "tab.close"),
            command(&text("macos-close-space"), "space.close"),
        ]),
        Menu::new("Edit").items([
            command(&text("macos-copy"), "terminal.copy"),
            command(&text("macos-paste"), "terminal.paste"),
            MenuItem::separator(),
            command(&text("macos-find"), "terminal.search"),
        ]),
        Menu::new("View").items([
            MenuItem::submenu(Menu::new("Layout").items(layout_items)),
            command(&text("macos-toggle-sidebar"), "layout.toggle:sidebar"),
            command(&text("macos-zoom-pane"), "pane.zoom"),
            MenuItem::separator(),
            command(&text("macos-actual-size"), "app.zoom_reset"),
            command(&text("macos-zoom-in"), "app.zoom_in"),
            command(&text("macos-zoom-out"), "app.zoom_out"),
            MenuItem::separator(),
            command(&text("macos-command-palette"), "app.palette"),
        ]),
        Menu::new("Window").items([
            command(&text("macos-minimize"), "window.minimize"),
            command(&text("macos-zoom"), "window.maximize"),
            MenuItem::separator(),
            command(&text("macos-go-to-tab"), "mod.open:picker"),
            command(&text("macos-next-tab"), "tab.next"),
            command(&text("macos-previous-tab"), "tab.prev"),
            command(&text("macos-next-space"), "space.next"),
            command(&text("macos-previous-space"), "space.prev"),
            command(&text("macos-next-agent"), "agent.next"),
            command(&text("macos-previous-agent"), "agent.prev"),
        ]),
        Menu::new("Help").items([
            command(&text("macos-keyboard-shortcuts"), "app.help"),
            command(&text("macos-welcome"), "app.welcome"),
            MenuItem::separator(),
            command(&text("macos-send-feedback"), "app.feedback"),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would replace the standard one.
    use super::{ShellAction, menus};
    use gpui::{Action, Keymap, Menu, MenuItem};

    fn actions(items: &[MenuItem], out: &mut Vec<(String, Box<dyn Action>)>) {
        for item in items {
            match item {
                MenuItem::Action { name, action, .. } => out.push((name.to_string(), action.boxed_clone())),
                MenuItem::Submenu(menu) => actions(&menu.items, out),
                _ => {}
            }
        }
    }

    fn all(menus: &[Menu]) -> Vec<(String, Box<dyn Action>)> {
        let mut out = Vec::new();
        for menu in menus {
            actions(&menu.items, &mut out);
        }
        out
    }

    /// A renamed command would leave an item that does nothing.
    #[test]
    fn every_menu_item_runs_a_command() {
        let menus = menus(&["main".into(), "terminals".into()], Some("main"));
        for (title, action) in all(&menus) {
            let Some(shell) = action.as_any().downcast_ref::<ShellAction>() else { continue };
            let (name, value) = shell.id().split_once(':').map_or((shell.id(), None), |(n, v)| (n, Some(v)));
            let c = crate::commands::find(name).unwrap_or_else(|| panic!("{title}: no command {name:?}"));
            if let Some(v) = value {
                assert!(!crate::commands::shorthand(c, v).is_null(), "{title}: {name} takes no {v:?}");
            }
        }
        assert!(menus.iter().any(|m| m.name == "Window"));
    }

    /// Items show the key the keymap binds to the same command.
    #[test]
    fn menu_items_show_the_shipped_keys() {
        let keymap = Keymap::new(crate::keys::parse(crate::keys::DEFAULTS).unwrap());
        let menus = menus(&[], None);
        let items = all(&menus);
        let key = |title: &str| {
            let (_, action) = items.iter().find(|(t, _)| t == title).unwrap();
            keymap.bindings_for_action(action.as_ref()).next().map(|b| b.keystrokes()[0].unparse())
        };
        assert_eq!(key("New Tab").as_deref(), Some("cmd-t"));
        assert_eq!(key("Split Right").as_deref(), Some("cmd-d"));
        assert_eq!(key("Settings…").as_deref(), Some("cmd-,"));
        assert_eq!(key("Quit Herdr").as_deref(), Some("cmd-q"));
        assert_eq!(key("Copy").as_deref(), Some("cmd-c"));
        assert_eq!(key("Toggle Sidebar").as_deref(), Some("cmd-b"));
        assert_eq!(key("Minimize").as_deref(), Some("cmd-m"));
    }

    #[test]
    fn the_layout_menu_marks_the_shown_layout() {
        let menus = menus(&["main".into(), "terminals".into()], Some("terminals"));
        let view = menus.iter().find(|m| m.name == "View").unwrap();
        let Some(MenuItem::Submenu(layouts)) = view.items.first() else { panic!("View starts with Layout") };
        let checked: Vec<String> = layouts
            .items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { name, checked: true, .. } => Some(name.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, ["terminals"]);
    }
}

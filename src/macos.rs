use gpui::*;
use gpui_shell::action::ShellAction;

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

/// Menu items keep the terminal command's shortcut, but editing belongs to
/// a focused control that handles editing. Other focus keeps the terminal fallback.
pub fn dispatch_edit(action: &ShellAction, terminal_focused: bool, window: &mut Window, cx: &mut App) -> bool {
    if terminal_focused {
        return false;
    }
    let action: Box<dyn Action> = match action.id() {
        "terminal.copy" => Box::new(gpui_base::input::Copy),
        "terminal.paste" => Box::new(gpui_base::input::Paste),
        _ => return false,
    };
    if !window.is_action_available(action.as_ref(), cx) {
        return false;
    }
    window.dispatch_action(action, cx);
    true
}

/// GPUI registers the menu named "Window" as the app's windows menu, so
/// macOS adds its own window items there (tiling, the window list).
pub(crate) fn menus(layouts: &[String], shown: Option<&str>) -> Vec<Menu> {
    let layout_items = layouts
        .iter()
        .map(|name| command(name, &format!("layout.switch:{name}")).checked(Some(name.as_str()) == shown))
        .chain([MenuItem::separator(), command("Next Layout", "layout.switch")]);
    vec![
        Menu::new("Herdr").items([
            command("What's New", "app.whats_new"),
            command("Check for Updates…", "app.settings:updates"),
            MenuItem::separator(),
            command("Settings…", "app.settings"),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            command("Hide Herdr", "app.hide"),
            command("Hide Others", "app.hide_others"),
            command("Show All", "app.show_all"),
            MenuItem::separator(),
            command("Quit Herdr", "app.quit"),
        ]),
        Menu::new("File").items([
            command("New Tab", "tab.new"),
            command("New Space", "space.new"),
            MenuItem::separator(),
            command("Split Right", "pane.split:right"),
            command("Split Down", "pane.split:down"),
            MenuItem::separator(),
            command("Rename Tab…", "tab.rename"),
            command("Rename Space…", "space.rename"),
            MenuItem::separator(),
            command("Close Pane", "pane.close"),
            command("Close Tab", "tab.close"),
            command("Close Space", "space.close"),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Copy", ShellAction::new("terminal.copy"), OsAction::Copy),
            MenuItem::os_action("Paste", ShellAction::new("terminal.paste"), OsAction::Paste),
            MenuItem::separator(),
            command("Find…", "terminal.search"),
        ]),
        Menu::new("View").items([
            MenuItem::submenu(Menu::new("Layout").items(layout_items)),
            command("Toggle Sidebar", "layout.toggle:sidebar"),
            command("Zoom Pane", "pane.zoom"),
            MenuItem::separator(),
            command("Actual Size", "app.zoom_reset"),
            command("Zoom In", "app.zoom_in"),
            command("Zoom Out", "app.zoom_out"),
            MenuItem::separator(),
            command("Command Palette…", "app.palette"),
        ]),
        Menu::new("Window").items([
            command("Minimize", "window.minimize"),
            command("Zoom", "window.maximize"),
            MenuItem::separator(),
            command("Go to Tab…", "mod.open:picker"),
            command("Next Tab", "tab.next"),
            command("Previous Tab", "tab.prev"),
            command("Next Space", "space.next"),
            command("Previous Space", "space.prev"),
            command("Next Agent", "agent.next"),
            command("Previous Agent", "agent.prev"),
        ]),
        Menu::new("Help").items([
            command("Keyboard Shortcuts", "app.help"),
            command("Welcome to Herdr", "app.welcome"),
            MenuItem::separator(),
            command("Send Feedback…", "app.feedback"),
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
    #[cfg(target_os = "macos")]
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
    fn clipboard_items_show_personal_keys() {
        let keymap = Keymap::new(crate::keys::parse(r#"{"global":{"cmd-alt-c":"terminal.copy","cmd-alt-v":"terminal.paste"}}"#).unwrap());
        let items = all(&menus(&[], None));
        for (title, shortcut) in [("Copy", "cmd-alt-c"), ("Paste", "cmd-alt-v")] {
            let (_, action) = items.iter().find(|(name, _)| name == title).unwrap();
            assert_eq!(keymap.bindings_for_action(action.as_ref()).next().unwrap().keystrokes()[0], gpui::KeyBinding::new(shortcut, gpui::NoAction, None).keystrokes()[0]);
        }
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

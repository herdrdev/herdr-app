#[gpui::test]
fn sidebar_toggle_stays_visible_in_the_collapsed_macos_rail(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();

    for (width, tooltip) in [(76, "Expand sidebar"), (268, "Collapse sidebar")] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("window".into(), json!({
            "platform": "macos",
            "titlebar": "none",
            "fullscreen": false,
            "buttons": false,
        }));
        values.get_mut("layout").unwrap()["children"][0]["size"]["px"] = json!(width);

        let tree = describe_with_values(cx, &staged, "mods/workspace-chrome", values, None).unwrap();
        assert!(tree.contains("sidebar-toggle"), "width={width}: {tree}");
        assert!(tree.contains(&format!(":tooltip[Str(\"{tooltip}\")]")), "width={width}: {tree}");
    }
}


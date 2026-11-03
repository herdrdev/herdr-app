use std::process::Command;

#[test]
fn commands_need_no_window_or_app_home() {
    let home = std::env::temp_dir().join(format!(
        "herdr-app-commands-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
        .arg("commands")
        .env("HERDR_APP_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let commands: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let commands = commands.as_array().unwrap();
    assert!(!commands.is_empty());
    for name in ["pane.split", "pane.move"] {
        assert!(commands.iter().any(|c| c["name"] == name), "{name}");
    }
    for command in commands {
        assert!(command["name"].is_string());
        assert!(command["about"].is_string());
        assert!(command["args"].is_array());
    }
    assert!(!home.exists(), "listing commands must not initialize an app home");
}

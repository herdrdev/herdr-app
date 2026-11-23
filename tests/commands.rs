use std::process::Command;

#[test]
#[ignore = "requires a native desktop; run cargo test --test commands check_keeps_shared_state_transient -- --ignored"]
fn check_keeps_shared_state_transient() {
    let root = std::env::temp_dir().join(format!("herdr-check-shared-{}", std::process::id()));
    let home = root.join("home");
    let module = root.join("mod");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("main.js"), r#"
import { View, div } from "gpui-kit";
import { shared, setShared } from "herdr";
export default class Probe extends View {
  init() {
    if (shared("sentinel") !== null) throw new Error("read desktop state");
    setShared("probe", "x".repeat(1000000));
    if (shared("probe").length !== 1000000) throw new Error("shared round trip failed");
  }
  render() { return div(); }
}
"#).unwrap();
    let path = home.join("shared.json");
    for original in [Some("{\"sentinel\":true}"), Some("broken json"), None] {
        if let Some(text) = original { std::fs::write(&path, text).unwrap(); }
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
            .arg("check").arg(&module)
            .env("HERDR_APP_HOME", &home)
            .env("HC_SESSION", format!("check-shared-{}", std::process::id()))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("APPDATA", root.join("appdata"))
            .env_remove("HC_TERMINAL_ID").env_remove("HC_TERMINAL_SOCKET").env_remove("HC_SCOPE")
            .output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), original);
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), usize::from(original.is_some()));
        if original.is_some() { std::fs::remove_file(&path).unwrap(); }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "requires a Windows desktop and HERDR_TEST_WSL_DISTRO naming an installed distribution"]
fn check_uses_the_selected_guest_home() {
    let distro = std::env::var("HERDR_TEST_WSL_DISTRO").unwrap();
    let root = std::env::temp_dir().join(format!("herdr-check-wsl-{}", std::process::id()));
    let home = root.join("home");
    let module = root.join("mod");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(home.join("settings.json"), serde_json::json!({ "wsl": distro }).to_string()).unwrap();
    std::fs::write(module.join("main.js"), r#"
import { View, div } from "gpui-kit";
import { home, settings } from "herdr";
export default class Probe extends View {
  init() {
    if (!settings().wsl || !home().startsWith("/")) throw new Error("check did not enter the selected guest");
  }
  render() { return div(); }
}
"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
        .arg("check").arg(&module).env("HERDR_APP_HOME", &home)
        .env("HC_SESSION", format!("check-wsl-{}", std::process::id())).output().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[cfg(windows)]
fn check_rejects_wsl(distro: &str) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = std::env::temp_dir().join(format!("herdr-check-rejected-{}-{stamp}", std::process::id()));
    let module = home.join("mod");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(home.join("settings.json"), serde_json::json!({ "wsl": distro }).to_string()).unwrap();
    // This link-only mod would pass without a desktop if the checker ignored discovery failure.
    std::fs::write(module.join("link.js"), "export function handleLink() { return false; }").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
        .arg("check").arg(module).env("HERDR_APP_HOME", &home)
        .env("HC_SESSION", format!("check-rejected-{}", std::process::id())).output().unwrap();
    std::fs::remove_dir_all(home).unwrap();
    assert!(!output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    String::from_utf8(output.stderr).unwrap()
}

#[cfg(windows)]
#[test]
fn check_stops_before_loading_a_mod_if_its_distro_is_unreachable() {
    let error = check_rejects_wsl(&format!("HerdrMissingTest-{}", std::process::id()));
    assert!(error.contains("herdr-app check:") && error.contains("WSL"), "{error}");
}

#[cfg(windows)]
#[test]
#[ignore = "requires HERDR_TEST_WSL_CORELESS_DISTRO naming an installed distribution without a guest core"]
fn check_stops_before_loading_a_mod_if_no_guest_core_was_validated() {
    let error = check_rejects_wsl(&std::env::var("HERDR_TEST_WSL_CORELESS_DISTRO").unwrap());
    assert!(error.contains("No guest herdr-core"), "{error}");
}

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
    assert!(commands.iter().any(|c| c["name"] == "pane.split"));
    for command in commands {
        assert!(command["name"].is_string());
        assert!(command["about"].is_string());
        assert!(command["args"].is_array());
    }
    assert!(!home.exists(), "listing commands must not initialize an app home");
}

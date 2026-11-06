use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

struct App {
    root: PathBuf,
    session: String,
    child: Option<Child>,
}

impl App {
    fn command(&self, binary: &str) -> Command {
        let mut command = Command::new(binary);
        command
            .env("HERDR_APP_HOME", self.root.join("home"))
            .env("HC_SESSION", &self.session)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("HERDR_APP_NO_SOUND", "1")
            .env("WAYLAND_DISPLAY", "")
            .env_remove("HC_SOCKET")
            .env_remove("HC_TERMINAL_ID")
            .env_remove("HC_TERMINAL_SOCKET")
            .env_remove("HC_SCOPE");
        command
    }

    fn start() -> Self {
        let mut app = Self {
            root: std::env::temp_dir().join(format!("herdr-bg-{}", std::process::id())),
            session: format!("background-{}", std::process::id()),
            child: None,
        };
        assert!(!app.root.exists());
        std::fs::create_dir_all(app.root.join("home")).unwrap();
        std::fs::write(
            app.root.join("home/shared.json"),
            r#"{"frame.welcomed":"test"}"#,
        )
        .unwrap();
        let log = std::fs::File::create(app.root.join("app.log")).unwrap();
        app.child = Some(
            app.command(env!("CARGO_BIN_EXE_herdr-app"))
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let output = app
                .command(env!("CARGO_BIN_EXE_herdr-app"))
                .arg("status")
                .output()
                .unwrap();
            if output.status.success() {
                break;
            }
            assert!(
                app.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "app exited: {}",
                std::fs::read_to_string(app.root.join("app.log")).unwrap()
            );
            assert!(
                Instant::now() < deadline,
                "app did not start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        app
    }

    fn query(&self, args: &[&str]) -> Value {
        let output = self
            .command(env!("CARGO_BIN_EXE_herdr-app"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.get("error").is_none(), "{args:?}: {value}");
        value
    }

    fn run(&self, name: &str, args: Value) -> Value {
        self.query(&["do", name, &args.to_string()])
    }

    fn shot(&self, name: &str) {
        let Some(directory) = std::env::var_os("HERDR_BG_ARTIFACTS") else {
            return;
        };
        let directory = PathBuf::from(directory);
        // Let asynchronous mod rendering settle before capturing evidence.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::create_dir_all(&directory).unwrap();
        let output = self
            .command(env!("CARGO_BIN_EXE_herdr-app"))
            .arg("shot")
            .arg(directory.join(format!("{name}.png")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn selection(&self) -> Value {
        let listing = self.query(&["ls"]);
        json!(listing["spaces"].as_array().unwrap().iter().map(|space| json!({
            "space": space["id"], "active": space["active"],
            "tab": space["tabs"].as_array().unwrap().iter().find(|tab| tab["active"] == true).map(|tab| tab["id"].clone()),
        })).collect::<Vec<_>>())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.command("hc").args(["server", "stop"]).output();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn creation_commands_advertise_optional_focus() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
        .arg("commands")
        .output()
        .unwrap();
    assert!(output.status.success());
    let commands: Value = serde_json::from_slice(&output.stdout).unwrap();
    for name in ["space.new", "tab.new"] {
        let command = commands
            .as_array()
            .unwrap()
            .iter()
            .find(|command| command["name"] == name)
            .unwrap();
        assert!(
            command["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg["name"] == "focus" && arg["required"] == false),
            "{name}"
        );
    }
}

#[test]
#[ignore = "requires an isolated desktop; run with a private DISPLAY and cargo test --test background_creation -- --ignored"]
fn background_creation_keeps_the_selected_space_and_each_selected_tab() {
    let app = App::start();
    let folder = app.root.to_string_lossy();
    // Even focus:false must show something when there is no existing selection.
    let first = app.run(
        "space.new",
        json!({"name":"source", "tab":"original", "folder":folder, "focus":false}),
    );
    let first_selection = app.selection();
    let focused_pane = app.query(&["status"])["focused_pane"].clone();
    assert!(focused_pane.is_string());
    assert_eq!(first_selection[0]["space"], first["space"]);
    assert_eq!(first_selection[0]["active"], true);
    assert_eq!(first_selection[0]["tab"], first["tab"]);
    let second = app.run(
        "space.new",
        json!({"name":"background", "tab":"background-original", "folder":folder, "focus":false}),
    );
    let after_space = app.selection();
    assert_eq!(after_space[0], first_selection[0]);
    assert_eq!(after_space[1]["active"], false);
    assert_eq!(after_space[1]["tab"], second["tab"]);

    // Neither a background tab here nor in another space changes that space's selection.
    for space in [&first["space"], &second["space"]] {
        let created = app.run(
            "tab.new",
            json!({"space":space, "name":"background-tab", "folder":folder, "focus":false}),
        );
        assert!(created["terminal"].is_string());
        assert_eq!(app.selection(), after_space);
    }

    let module = app.run(
        "tab.new",
        json!({"space":second["space"], "name":"background-mod", "mod":"files", "focus":false}),
    );
    assert!(module["pane"].is_string());
    assert_eq!(app.selection(), after_space);
    assert_eq!(app.query(&["status"])["focused_pane"], focused_pane);
    app.shot("background-selection");
    // Existing callers and an explicit true still select the new space/tab.
    let shown = app.run(
        "tab.new",
        json!({"space":second["space"], "name":"shown", "folder":folder}),
    );
    let selected = app.selection();
    assert_eq!(selected[1]["active"], true);
    assert_eq!(selected[1]["tab"], shown["tab"]);
    let third = app.run("space.new", json!({"name":"shown-space", "folder":folder}));
    let selected = app.selection();
    assert_eq!(selected[2]["active"], true);
    assert_eq!(selected[2]["tab"], third["tab"]);
    let tab = app.run(
        "tab.new",
        json!({"space":first["space"], "name":"shown-tab", "folder":folder, "focus":true}),
    );
    let selected = app.selection();
    assert_eq!(selected[0]["active"], true);
    assert_eq!(selected[0]["tab"], tab["tab"]);
    let fourth = app.run(
        "space.new",
        json!({"name":"explicit-focus", "folder":folder, "focus":true}),
    );
    let selected = app.selection();
    assert_eq!(selected[3]["active"], true);
    assert_eq!(selected[3]["tab"], fourth["tab"]);
    app.shot("explicit-selection");
}

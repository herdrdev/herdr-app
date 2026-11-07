use std::process::Command;

#[test]
fn invalid_folder_arguments_fail_before_starting_or_contacting_the_app() {
    let root = std::env::temp_dir().join(format!("herdr-open-args-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("a file & 'quoted'.txt");
    std::fs::write(&file, "fixture").unwrap();
    let home = root.join("home");
    let missing = root.join("missing");
    for (args, expected) in [
        (vec!["open"], "usage:"),
        (vec!["open", ".", "."], "usage:"),
        (vec!["open", ""], "folder path is required"),
        (vec!["open", file.to_str().unwrap()], "not a folder"),
        (vec!["open", missing.to_str().unwrap()], "not a folder"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-app"))
            .args(args).env("HERDR_APP_HOME", &home).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(!home.exists(), "invalid arguments must not initialize an app home");
    }
    std::fs::remove_dir_all(root).unwrap();
}

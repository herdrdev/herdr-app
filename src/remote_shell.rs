//! The shell used to check and launch agents on another machine.

pub fn check(commands: &[String]) -> Vec<String> {
    let posix = "for c; do unalias -- \"$c\" 2>/dev/null; command -v \"$c\" >/dev/null 2>&1 && printf 'herdr-has:%s\\n' \"$c\"; done";
    in_shell(posix, "for c in $argv[2..]; command -v $c >/dev/null 2>&1; and printf 'herdr-has:%s\\n' $c; end", posix, commands.to_vec())
}

pub fn launch(command: Vec<String>) -> Vec<String> {
    in_shell("\"$@\"; exec \"$0\" -li", "command $argv[2..]; exec $argv[1] -li", "\"$@\"; exec \"${SHELL:-sh}\" -i", command)
}

/// Select the shell there, not the viewer's shell. The core may have started
/// with only the SSH session's minimal PATH; load login and interactive profiles
/// before checking or launching. Only our fixed scripts become shell source.
fn in_shell(posix: &str, fish: &str, fallback: &str, args: Vec<String>) -> Vec<String> {
    let quote = |s: &str| s.replace('\'', "'\\''");
    let script = format!(
        r#"shell=${{SHELL:-/bin/sh}}
case "${{shell##*/}}" in
  fish) exec "$shell" -lic '{}' "$shell" "$@" ;;
  sh|bash|zsh|dash|ksh) exec "$shell" -lic '{}' "$shell" "$@" ;;
  *) exec sh -c '{}' sh "$@" ;;
esac"#,
        quote(fish),
        quote(posix),
        quote(fallback),
    );
    // Keep the command at argv[4..]: the Windows fallback reads it there when
    // sh is unavailable. Passing it separately also preserves literal arguments.
    let mut argv = vec!["sh".into(), "-c".into(), script, "sh".into()];
    argv.extend(args);
    argv
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Home(PathBuf);

    impl Home {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!("herdr-shell-{}-{} 'home", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(dir.join("agent bin")).unwrap();
            let login = "export HERDR_TEST_LOGIN=loaded\nexport HERDR_TEST_BIN=\"$HOME/agent bin\"\n";
            let interactive = "export PATH=\"$HERDR_TEST_BIN:$PATH\"\nprintf 'rc greeting without newline'\nsleep 0.1\nfixture_function() { :; }\nalias missing_agent='printf alias'\n";
            for (file, text) in [
                (".zprofile", login.to_owned()),
                (".zshrc", interactive.to_owned()),
                (".bash_profile", format!("{login}. \"$HOME/.bashrc\"\n")),
                (".bashrc", format!("case $- in *i*) ;; *) return;; esac\n{interactive}")),
            ] {
                std::fs::write(dir.join(file), text).unwrap();
            }
            let agent = dir.join("agent bin/fixture_agent");
            std::fs::write(&agent, "#!/bin/sh\nprintf '<%s>\\n' \"$HERDR_TEST_LOGIN\" \"$@\"\n").unwrap();
            std::fs::set_permissions(agent, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self(dir)
        }

        fn run(&self, argv: Vec<String>, shell: Option<&str>) -> Output {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .env_clear()
                .env("HOME", &self.0)
                .env("ZDOTDIR", &self.0)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("TERM", "dumb")
                .stdin(Stdio::null());
            if let Some(shell) = shell {
                cmd.env("SHELL", shell);
            }
            // Interactive zsh otherwise opens the test runner's controlling tty.
            use std::os::unix::process::CommandExt;
            unsafe extern "C" {
                fn setsid() -> std::ffi::c_int;
            }
            unsafe {
                cmd.pre_exec(|| {
                    if setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            cmd.output().unwrap()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn finds_agents(shell: &str) {
        let home = Home::new();
        let commands = ["fixture_agent", "fixture_function", "missing_agent", "missing'command;false"].map(String::from);
        let out = home.run(check(&commands), Some(shell));
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("herdr-has:fixture_agent\n"), "{stdout:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.contains("herdr-has:fixture_function\n"), "{stdout:?}");
        assert!(!stdout.contains("herdr-has:missing"), "{stdout:?}");
    }

    fn launches_agent(shell: &str) {
        let home = Home::new();
        let cmd = ["fixture_agent", "a 'quoted' task; $(not-a-command)\nnext line", ""].map(String::from).to_vec();
        let out = home.run(launch(cmd), Some(shell));
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("<loaded>\n<a 'quoted' task; $(not-a-command)\nnext line>\n<>\n"), "{stdout:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn bash_detects_agents_from_login_and_interactive_profiles() {
        finds_agents("/bin/bash");
    }

    #[test]
    fn bash_launches_agents_with_literal_arguments() {
        launches_agent("/bin/bash");
    }

    #[test]
    fn zsh_detects_agents_from_login_and_interactive_profiles() {
        if std::path::Path::new("/bin/zsh").exists() {
            finds_agents("/bin/zsh");
        }
    }

    #[test]
    fn zsh_launches_agents_with_literal_arguments() {
        if std::path::Path::new("/bin/zsh").exists() {
            launches_agent("/bin/zsh");
        }
    }

    #[test]
    fn fish_detects_and_launches_agents_with_literal_arguments() {
        let shell = "/usr/bin/fish";
        if !std::path::Path::new(shell).exists() {
            return;
        }
        let home = Home::new();
        let config = home.0.join(".config/fish");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.fish"), "if status is-interactive\nset -gx PATH \"$HOME/agent bin\" $PATH\necho rc greeting\nsleep 0.1\nend\n").unwrap();
        let out = home.run(check(&["fixture_agent".into(), "missing_agent".into()]), Some(shell));
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("herdr-has:fixture_agent\n"), "{stdout:?}");
        assert!(!stdout.contains("herdr-has:missing"), "{stdout:?}");
        let out = home.run(launch(vec!["fixture_agent".into(), "a 'quoted' task; $(not-a-command)".into(), "".into()]), Some(shell));
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(stdout.contains("<a 'quoted' task; $(not-a-command)>\n<>\n"), "{stdout:?}");
    }

    #[test]
    fn missing_shell_keeps_working_with_sh() {
        let home = Home::new();
        let out = home.run(check(&["printf".into()]), None);
        assert!(String::from_utf8(out.stdout).unwrap().contains("herdr-has:printf\n"));
        let out = home.run(launch(vec!["/usr/bin/printf".into(), "%s".into(), "literal 'argument'".into()]), None);
        // The account's own shell may still greet (zsh on macOS runners).
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(stdout.replace("rc greeting without newline", ""), "literal 'argument'", "{stdout:?}");
    }

    #[test]
    fn absolute_command_overrides_keep_paths_and_arguments_literal() {
        let home = Home::new();
        let agent = home.0.join("agent bin/fixture_agent").to_string_lossy().into_owned();
        let out = home.run(launch(vec![agent, "$(not-a-command); 'literal'".into()]), Some("/bin/bash"));
        assert!(String::from_utf8(out.stdout).unwrap().contains("<loaded>\n<$(not-a-command); 'literal'>\n"));
    }

    #[test]
    fn remote_shell_paths_with_spaces_are_not_split() {
        use std::os::unix::fs::symlink;
        let home = Home::new();
        let shell = home.0.join("bash");
        symlink("/bin/bash", &shell).unwrap();
        let shell = shell.to_str().unwrap();
        let out = home.run(check(&["fixture_agent".into()]), Some(shell));
        assert!(String::from_utf8(out.stdout).unwrap().contains("herdr-has:fixture_agent\n"));
        let out = home.run(launch(vec!["fixture_agent".into(), "literal".into()]), Some(shell));
        assert!(String::from_utf8(out.stdout).unwrap().contains("<loaded>\n<literal>\n"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn non_posix_login_shells_keep_the_existing_sh_launcher() {
        let home = Home::new();
        std::fs::write(home.0.join(".tcshrc"), "# scratch shell home\n").unwrap();
        let out = home.run(check(&["printf".into()]), Some("/bin/tcsh"));
        assert!(String::from_utf8(out.stdout).unwrap().contains("herdr-has:printf\n"));
        let out = home.run(launch(vec!["/usr/bin/printf".into(), "%s".into(), "literal 'argument'".into()]), Some("/bin/tcsh"));
        assert!(String::from_utf8(out.stdout).unwrap().contains("literal 'argument'"));
    }

    #[test]
    fn remote_commands_keep_windows_fallback_arguments() {
        let command = vec!["codex".into(), "a 'quoted' task".into(), "".into()];
        let argv = launch(command.clone());
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[4..], command);
    }
}

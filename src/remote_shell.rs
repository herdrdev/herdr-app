//! The shell used to check and launch agents on another machine.

pub fn check(commands: &[String]) -> Vec<String> {
    let posix = "for c; do unalias -- \"$c\" 2>/dev/null; command -v \"$c\" >/dev/null 2>&1 && printf 'herdr-has:%s\\n' \"$c\"; done; printf '\\nherdr-check-complete\\n'";
    in_shell("", posix, "for c in $argv[2..]; command -v $c >/dev/null 2>&1; and printf 'herdr-has:%s\\n' $c; end; printf '\\nherdr-check-complete\\n'", posix, commands.to_vec())
}

pub fn launch(command: Vec<String>) -> Vec<String> {
    launch_in_shell("\"$@\"; exec \"$0\" -li", "command $argv[2..]; exec $argv[1] -li", "\"$@\"; exec \"${SHELL:-sh}\" -i", command)
}

/// The shell becomes the program (`exec`), so it ends with the program's exit code. A
/// missing program fails as the core reports one: exit 127, `cannot start "x": ...`, and
/// it sets the title to `$HERDR_LAUNCH`, which the program never sees (`cannot_start`).
/// A program named like an option (`-x`) runs by its path: bash's and zsh's `exec`
/// would read it as one, dash's takes no `--`, and a script's `#!` interpreter gets the
/// path as its first argument. fish's `exec` reads no options.
pub const POSIX_EXEC: &str = r#"p=$1; case $p in -*) p=$(command -v -- "$p") && case $p in /*) ;; */*) p=./$p ;; *) false ;; esac ;; *) command -v -- "$p" ;; esac >/dev/null 2>&1 || { printf '\033]2;%s\007cannot start "%s": No such file or directory\n' "$HERDR_LAUNCH" "$1" >&2; exit 127; }; unset HERDR_LAUNCH; shift; exec "$p" "$@""#;
pub const FISH_EXEC: &str = r#"if not command -q -- $argv[2]; printf '\e]2;%s\acannot start "%s": No such file or directory\n' "$HERDR_LAUNCH" $argv[2] >&2; exit 127; end; set -e HERDR_LAUNCH; set -l p $argv[2]; string match -q -- '-*/*' $p; and set p ./$p; exec $p $argv[3..]"#;

/// `command` in that machine's shell, which then becomes it (`POSIX_EXEC`).
pub fn exec(command: Vec<String>) -> Vec<String> {
    launch_in_shell(POSIX_EXEC, FISH_EXEC, POSIX_EXEC, command)
}

// Capture the cwd before the user's rc files run, even when the core chose it
// (e.g. a remote retry in its home). The suffix preserves trailing newlines.
const CAPTURE_FOLDER: &str = r#"HERDR_START_DIR=$(pwd -P && printf .) || exit 127
export HERDR_START_DIR=${HERDR_START_DIR%??}
"#;

fn restore_folder(script: &str, fish: bool) -> String {
    let restore = if fish {
        r#"builtin cd -- "$HERDR_START_DIR"; or begin; test -n "$HERDR_LAUNCH"; and printf '\e]2;%s\a' "$HERDR_LAUNCH" >&2; exit 127; end; set -e HERDR_START_DIR; "#
    } else {
        r#"if command -v builtin >/dev/null 2>&1; then builtin cd -P "$HERDR_START_DIR"; else command cd -P "$HERDR_START_DIR"; fi || { [ -z "$HERDR_LAUNCH" ] || printf '\033]2;%s\007' "$HERDR_LAUNCH" >&2; exit 127; }; unset HERDR_START_DIR; "#
    };
    format!("{restore}{script}")
}

pub fn local(shell: String, flag: &str, script: &str, fish: bool, command: Vec<String>) -> Vec<String> {
    let script = quote(&restore_folder(script, fish));
    let outer = format!("{CAPTURE_FOLDER}exec \"$0\" {flag} '{script}' \"$0\" \"$@\"");
    let mut argv = vec!["/bin/sh".into(), "-c".into(), outer, shell];
    argv.extend(command);
    argv
}

fn launch_in_shell(posix: &str, fish: &str, fallback: &str, command: Vec<String>) -> Vec<String> {
    in_shell(CAPTURE_FOLDER, &restore_folder(posix, false), &restore_folder(fish, true), &restore_folder(fallback, false), command)
}

/// Whether a terminal's `POSIX_EXEC` or `FISH_EXEC` said it can't start its program: the
/// title in its `terminal.get` record is its launch token, which only the wrapper sees.
pub fn cannot_start(record: &serde_json::Value) -> bool {
    record["labels"]["herdr.launch"].as_str().is_some_and(|t| !t.is_empty() && record["title"] == t)
}

fn quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Select the shell there, not the viewer's shell. The core may have started
/// with only the SSH session's minimal PATH; load login and interactive profiles
/// before checking or launching. Only our fixed scripts become shell source.
fn in_shell(prefix: &str, posix: &str, fish: &str, fallback: &str, args: Vec<String>) -> Vec<String> {
    let script = format!(
        r#"{prefix}shell=${{SHELL:-/bin/sh}}
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
            self.run_in(argv, shell, &self.0)
        }

        fn run_in(&self, argv: Vec<String>, shell: Option<&str>, cwd: &std::path::Path) -> Output {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .current_dir(cwd)
                .env_clear()
                .env("HOME", &self.0)
                .env("ZDOTDIR", &self.0)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("TERM", "dumb")
                .env("HERDR_LAUNCH", "test-token")
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

    /// A split's program replaces the shell: its exit code is the terminal's,
    /// and a missing one fails like the core's own `cannot start`.
    #[test]
    fn remote_programs_replace_the_shell() {
        let home = Home::new();
        let config = home.0.join(".config/fish");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.fish"), "if status is-interactive\nset -gx PATH \"$HOME/agent bin\" $PATH\nend\n").unwrap();
        std::fs::write(home.0.join("agent bin/exits"), "#!/bin/sh\nprintf '<%s>\\n' \"$HERDR_LAUNCH\" \"$@\"\nexit 7\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home.0.join("agent bin/exits"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let arg = "a 'quoted' $HOME; $(not-a-command)\nnext line";
        for shell in [Some("/bin/bash"), Some("/bin/zsh"), Some("/usr/bin/fish"), None] {
            if shell.is_some_and(|s| !std::path::Path::new(s).exists()) {
                continue;
            }
            let out = home.run(exec(vec!["exits".into(), arg.into(), "".into()]), shell);
            let stdout = String::from_utf8_lossy(&out.stdout);
            if shell.is_some() {
                // The program never sees the launch token.
                assert!(stdout.ends_with(&format!("<>\n<{arg}>\n<>\n")), "{shell:?}: {stdout:?} {}", String::from_utf8_lossy(&out.stderr));
                assert_eq!(out.status.code(), Some(7), "{shell:?}");
            }
            let out = home.run(exec(vec!["no-such-program".into()]), shell);
            assert_eq!(out.status.code(), Some(127), "{shell:?}");
            assert!(String::from_utf8_lossy(&out.stderr).contains("cannot start \"no-such-program\": No such file or directory"), "{shell:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    /// The program is argv[0] even when it looks like an option: `--` is a missing
    /// program, not the end of the wrapper's options, and `-dashprog` still runs.
    #[test]
    fn programs_named_like_options_are_programs() {
        use std::os::unix::fs::PermissionsExt;
        let home = Home::new();
        let config = home.0.join(".config/fish");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("config.fish"), "if status is-interactive\nset -gx PATH \"$HOME/agent bin\" $PATH\nend\n").unwrap();
        // One on the rc PATH, one by a path relative to the folder it starts in.
        for path in [home.0.join("agent bin/-dashprog"), home.0.join("-dir/prog")] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "#!/bin/sh\nprintf '<%s>\\n' dashprog \"$@\"\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("PATH={}:/usr/bin:/bin", home.0.join("agent bin").display());
        let cases: [(&[&str], Result<&str, &str>); 5] = [
            (&["--", "/bin/echo", "X"], Err("--")),
            (&["--"], Err("--")),
            (&["-h"], Err("-h")),
            (&["-dashprog", "a"], Ok("<dashprog>\n<a>\n")),
            (&["-dir/prog", "b"], Ok("<dashprog>\n<b>\n")),
        ];
        for (cmd, want) in cases {
            let cmd = cmd.iter().map(|a| a.to_string()).collect::<Vec<_>>();
            let mut runs = vec![];
            for shell in ["/bin/bash", "/bin/zsh", "/usr/bin/fish"] {
                runs.push((Some(shell), exec(cmd.clone())));
            }
            // Each shell's own `command` and `exec`, as a local split runs them.
            for sh in ["/bin/sh", "/bin/dash", "/bin/bash", "/bin/zsh", "/usr/bin/fish"] {
                let script = if sh.ends_with("/fish") { FISH_EXEC } else { POSIX_EXEC };
                let mut argv = vec!["/usr/bin/env".to_owned(), path.clone(), sh.into(), "-c".into(), script.into(), sh.into()];
                argv.extend(cmd.clone());
                runs.push((Some(sh), argv));
            }
            for (shell, argv) in runs {
                if shell.is_some_and(|s| !std::path::Path::new(s).exists()) {
                    continue;
                }
                let out = home.run(argv.clone(), shell);
                let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                match want {
                    Ok(want) => {
                        assert!(stdout.ends_with(want), "{argv:?}: {stdout:?} {stderr}");
                        assert_eq!(out.status.code(), Some(0), "{argv:?}");
                    }
                    Err(name) => {
                        assert_eq!(out.status.code(), Some(127), "{argv:?}: {stdout:?}");
                        assert!(stderr.contains(&format!("\x1b]2;test-token\x07cannot start \"{name}\": No such file or directory")), "{argv:?}: {stderr:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn remote_commands_keep_windows_fallback_arguments() {
        let command = vec!["codex".into(), "a 'quoted' task".into(), "".into()];
        let argv = launch(command.clone());
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[4..], command);
    }

    #[test]
    fn commands_restore_the_core_working_directory_after_shell_startup() {
        let home = Home::new();
        let checkout = home.0.join("chosen checkout");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(home.0.join(".bash_profile"), "cd /\n").unwrap();
        for folder in [&checkout, &home.0] {
            let cmd = vec!["/bin/pwd".into(), "-P".into()];
            for argv in [exec(cmd.clone()), launch(cmd.clone()), local("/bin/bash".into(), "-lic", "\"$@\"; exec \"$0\" -li", false, cmd)] {
                let out = home.run_in(argv, Some("/bin/bash"), folder);
                let cwd = folder.canonicalize().unwrap();
                assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), cwd.to_str().unwrap(), "{}", String::from_utf8_lossy(&out.stderr));
            }
        }
        std::fs::write(home.0.join(".bash_profile"), "cd /; rmdir \"$HERDR_START_DIR\"\n").unwrap();
        let out = home.run_in(exec(vec!["/bin/pwd".into()]), Some("/bin/bash"), &checkout);
        assert_eq!(out.status.code(), Some(127));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("\x1b]2;test-token\x07"));
    }
}

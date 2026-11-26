//! Starting programs. The app has no console on Windows, so each console
//! program it starts (git, hc, curl) would open a window of its own.

use std::ffi::OsStr;
use std::process::Command;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Shells can open /dev/tty even with stdin redirected. Keep captured
/// commands from stopping the app's process group when it is a background job.
/// Without a terminal there is nothing to keep them from, and the `setsid`
/// hook would only make std fork the whole app instead of using posix_spawn.
pub fn detached_command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = command(program);
    #[cfg(unix)]
    if has_terminal() {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and runs only in the new child.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd
}

/// Whether the app has a controlling terminal. Opening it never stops a
/// background job; only reading or writing it can.
#[cfg(unix)]
fn has_terminal() -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open("/dev/tty")
        .is_ok()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::process::Stdio;

    /// With no terminal for a command to open, it stays in the app's session.
    /// A `setsid` hook would make std fork the whole app instead of using
    /// posix_spawn, for every command a mod runs.
    #[test]
    fn without_a_terminal_commands_stay_in_the_app_session() {
        const CHILD: &str = "HERDR_TEST_NO_TERMINAL";
        if std::env::var_os(CHILD).is_some() {
            let mut child = super::detached_command("sleep").arg("10").stdin(Stdio::null()).spawn().unwrap();
            // SAFETY: getsid only reads the session of a process this test owns.
            let (app, command) = unsafe { (libc::getsid(0), libc::getsid(child.id() as libc::pid_t)) };
            let _ = child.kill();
            let _ = child.wait();
            assert_eq!(command, app, "the command left the app's session");
            return;
        }

        // util-linux `setsid` runs the test in a new session with no controlling terminal.
        if std::process::Command::new("setsid").arg("--version").output().is_err() {
            eprintln!("skipped: util-linux setsid is not installed");
            return;
        }
        let output = std::process::Command::new("setsid")
            .args(["-w", "--"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "spawn::tests::without_a_terminal_commands_stay_in_the_app_session"])
            .env(CHILD, "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}\n{}{}", output.status, String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

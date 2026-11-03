//! A checker uses AppKit, but must not join its caller's agent coalition.
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const SENTINEL: &str = "--herdr-check-job";
const DIRECT: &str = "HERDR_CHECK_DIRECT";

#[derive(serde::Serialize, serde::Deserialize)]
struct Invocation {
    args: Vec<String>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    cwd: PathBuf,
}

/// Called before any AppKit or runtime initialization.
pub fn startup(args: &[String]) {
    if args.get(1).is_some_and(|arg| arg == SENTINEL) {
        let result = args.get(2).ok_or_else(|| io::Error::other("missing checker job directory"))
            .and_then(|dir| worker(Path::new(dir)));
        if let Err(error) = result {
            eprintln!("herdr-app: checker job failed: {error}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if args.get(1).is_none_or(|arg| arg != "check") {
        return;
    }
    // Only the job's immediate child may bypass submission, never a descendant.
    let direct = std::env::var(DIRECT).ok().and_then(|s| s.parse::<libc::pid_t>().ok())
        == Some(unsafe { libc::getppid() });
    if direct {
        // main has not started any threads yet.
        unsafe { std::env::remove_var(DIRECT) };
        return;
    }
    match submit(args) {
        Ok(code) => std::process::exit(code),
        Err(error) => eprintln!("herdr-app: launchd checker submission failed; checking in this process (shares the caller's coalition): {error}"),
    }
}

fn worker(dir: &Path) -> io::Result<()> {
    let mut ready = UnixStream::connect(dir.join("ready"))?;
    let invocation: Invocation = serde_json::from_reader(File::open(dir.join("invocation.json"))?)?;
    let status = Command::new(std::env::current_exe()?)
        .args(invocation.args)
        .env_clear()
        .envs(invocation.env.into_iter().map(|(key, value)| (OsString::from_vec(key), OsString::from_vec(value))))
        .env(DIRECT, std::process::id().to_string())
        .env("XPC_SERVICE_NAME", "0")
        .current_dir(invocation.cwd)
        .stdin(Stdio::null())
        .stdout(File::create(dir.join("stdout"))?)
        .stderr(File::create(dir.join("stderr"))?)
        .status()?;
    let code = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
    ready.write_all(&code.to_be_bytes())
}

struct Job {
    dir: PathBuf,
    target: String,
    loaded: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        if self.loaded {
            let _ = Command::new("/bin/launchctl").args(["bootout", &self.target]).output();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Errors before submission permit fallback; after submission never run a check twice.
fn submit(args: &[String]) -> io::Result<i32> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_nanos();
    let label = format!("dev.herdr.check.{}.{stamp}", std::process::id());
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let dir = PathBuf::from(format!("/tmp/herdr-check-{}-{stamp}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let mut job = Job { dir, target: format!("{domain}/{label}"), loaded: false };
    let invocation = Invocation {
        args: args[1..].to_vec(),
        env: std::env::vars_os().map(|(key, value)| (key.into_vec(), value.into_vec())).collect(),
        cwd: std::env::current_dir()?,
    };
    serde_json::to_writer(File::create(job.dir.join("invocation.json"))?, &invocation)?;
    let listener = UnixListener::bind(job.dir.join("ready"))?;
    let executable = std::env::current_exe()?;
    let executable = executable.to_str().ok_or_else(|| io::Error::other("non-UTF-8 executable path"))?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
         <key>Label</key><string>{label}</string>\
         <key>ProgramArguments</key><array><string>{}</string><string>{SENTINEL}</string><string>{}</string></array>\
         <key>RunAtLoad</key><true/><key>ProcessType</key><string>Background</string>\
         <key>StandardErrorPath</key><string>{}/worker-stderr</string></dict></plist>",
        xml(executable), xml(job.dir.to_str().unwrap()), xml(job.dir.to_str().unwrap()),
    );
    let path = job.dir.join("job.plist");
    fs::write(&path, plist)?;
    let output = Command::new("/bin/launchctl").args(["bootstrap", &domain]).arg(&path).output()?;
    if !output.status.success() {
        let _ = Command::new("/bin/launchctl").args(["bootout", &job.target]).output();
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()));
    }
    job.loaded = true;
    let result = (|| {
        let mut poll = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut poll, 1, 10_000) } <= 0 {
            return Err(io::Error::other("checker job did not connect within 10 seconds"));
        }
        let (mut ready, _) = listener.accept()?;
        let mut code = [0; 4];
        ready.read_exact(&mut code)?;
        Ok(i32::from_be_bytes(code))
    })();
    // The checker has exited. Relay the exact byte streams, including invalid UTF-8.
    let relay = |name: &str, out: &mut dyn Write| -> io::Result<()> {
        match File::open(job.dir.join(name)) {
            Ok(mut file) => { io::copy(&mut file, out)?; Ok(()) }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    };
    let result = result.and_then(|code| {
        relay("stdout", &mut io::stdout().lock())?;
        relay("stderr", &mut io::stderr().lock())?;
        Ok(code)
    });
    match result {
        Ok(code) => Ok(code),
        Err(error) => {
            let _ = relay("worker-stderr", &mut io::stderr().lock());
            eprintln!("herdr-app: isolated checker failed: {error}");
            Ok(1)
        }
    }
}

fn xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_preserves_arguments_and_unix_environment_bytes() {
        let invocation = Invocation {
            args: vec!["check".into(), "a mod/with 'quotes' & spaces".into()],
            env: vec![(b"KEY".to_vec(), vec![0xff, b'\n', b'&'])],
            cwd: PathBuf::from("/tmp/a folder"),
        };
        let encoded = serde_json::to_vec(&invocation).unwrap();
        let decoded: Invocation = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.args, invocation.args);
        assert_eq!(decoded.env, invocation.env);
        assert_eq!(decoded.cwd, invocation.cwd);
    }

    #[test]
    fn plist_paths_escape_xml_metacharacters() {
        assert_eq!(xml("/tmp/a&b<\"'>"), "/tmp/a&amp;b&lt;&quot;&apos;&gt;");
    }
}

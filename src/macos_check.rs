//! A checker uses AppKit, but must not join its caller's agent coalition.
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SENTINEL: &str = "--herdr-check-job";
const DIRECT: &str = "HERDR_CHECK_DIRECT";
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

extern "C" fn interrupt(signal: libc::c_int) {
    INTERRUPTED.store(signal, Ordering::Relaxed);
}

struct Interrupts {
    previous: [libc::sigaction; 2],
}

impl Interrupts {
    fn install() -> io::Result<Self> {
        let mut guard = Self { previous: unsafe { std::mem::zeroed() } };
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = interrupt as *const () as usize;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        for (index, signal) in [libc::SIGINT, libc::SIGTERM].into_iter().enumerate() {
            if unsafe { libc::sigaction(signal, &action, &mut guard.previous[index]) } != 0 {
                let error = io::Error::last_os_error();
                if index == 1 {
                    unsafe { libc::sigaction(libc::SIGINT, &guard.previous[0], std::ptr::null_mut()) };
                }
                std::mem::forget(guard);
                return Err(error);
            }
        }
        Ok(guard)
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        for (signal, action) in [libc::SIGINT, libc::SIGTERM].into_iter().zip(&self.previous) {
            unsafe { libc::sigaction(signal, action, std::ptr::null_mut()) };
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Invocation {
    args: Vec<String>,
    env: Vec<(Vec<u8>, Vec<u8>)>,
    cwd: PathBuf,
}

/// Called before any AppKit or runtime initialization.
pub fn startup(args: &[String]) {
    if args.get(1).is_some_and(|arg| arg == SENTINEL) {
        let result = args.get(2).zip(args.get(3)).ok_or_else(|| io::Error::other("missing checker job directory or target"))
            .and_then(|(dir, target)| worker(Path::new(dir), target));
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
        Err(error) => {
            let signal = INTERRUPTED.load(Ordering::Relaxed);
            if signal != 0 { std::process::exit(128 + signal); }
            eprintln!("herdr-app: launchd checker submission failed; checking in this process (shares the caller's coalition): {error}");
        }
    }
}

struct WorkerJob<'a> {
    dir: &'a Path,
    target: &'a str,
}

impl Drop for WorkerJob<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.dir);
        let _ = Command::new("/bin/launchctl").args(["bootout", self.target]).output();
    }
}

fn worker(dir: &Path, target: &str) -> io::Result<()> {
    let _job = WorkerJob { dir, target };
    let mut ready = UnixStream::connect(dir.join("ready"))?;
    let bytes = fs::read(dir.join("invocation.json"))?;
    fs::remove_file(dir.join("invocation.json"))?;
    let invocation: Invocation = serde_json::from_slice(&bytes)?;
    ready.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut start = [0];
    ready.read_exact(&mut start)?;
    if start != [1] {
        return Err(io::Error::other("invalid checker handshake"));
    }
    ready.set_read_timeout(None)?;
    let child = Command::new(std::env::current_exe()?)
        .args(invocation.args)
        .env_clear()
        .envs(invocation.env.into_iter().map(|(key, value)| (OsString::from_vec(key), OsString::from_vec(value))))
        .env(DIRECT, std::process::id().to_string())
        .env("XPC_SERVICE_NAME", "0")
        .current_dir(invocation.cwd)
        .stdin(Stdio::null())
        .stdout(File::create(dir.join("stdout"))?)
        .stderr(File::create(dir.join("stderr"))?)
        .process_group(0)
        .spawn()?;
    let Some(status) = wait_for_checker(child, &ready)? else { return Ok(()) };
    let code = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
    ready.write_all(&code.to_be_bytes())?;
    // Keep the output files until the caller has relayed them or gone away.
    wait_for_ack(&mut ready)
}

fn wait_for_ack(ready: &mut UnixStream) -> io::Result<()> {
    let mut ack = [0];
    loop {
        match ready.read(&mut ack) {
            Ok(0) => return Ok(()),
            Ok(1) if ack == [1] => return Ok(()),
            Ok(_) => return Err(io::Error::other("invalid checker acknowledgement")),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_checker(mut child: Child, ready: &UnixStream) -> io::Result<Option<ExitStatus>> {
    let result = (|| loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let mut poll = libc::pollfd { fd: ready.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let count = unsafe { libc::poll(&mut poll, 1, 50) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted { continue; }
            return Err(error);
        }
        if count > 0 {
            return Ok(None);
        }
    })();
    if !matches!(result, Ok(Some(_))) {
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
        child.wait()?;
    }
    result
}

struct Job {
    dir: PathBuf,
    target: String,
    loaded: bool,
    handed_off: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        if self.handed_off { return; }
        if self.loaded {
            let _ = Command::new("/bin/launchctl").args(["bootout", &self.target]).output();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Errors before submission permit fallback; after submission never run a check twice.
fn submit(args: &[String]) -> io::Result<i32> {
    let mut interrupts = Some(Interrupts::install()?);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_nanos();
    let label = format!("dev.herdr.check.{}.{stamp}", std::process::id());
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let dir = PathBuf::from(format!("/tmp/herdr-check-{}-{stamp}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let mut job = Job { dir, target: format!("{domain}/{label}"), loaded: false, handed_off: false };
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
         <key>ProgramArguments</key><array><string>{}</string><string>{SENTINEL}</string><string>{}</string><string>{}</string></array>\
         <key>RunAtLoad</key><true/><key>ProcessType</key><string>Background</string>\
         <key>StandardErrorPath</key><string>{}/worker-stderr</string></dict></plist>",
        xml(executable), xml(job.dir.to_str().unwrap()), xml(&job.target), xml(job.dir.to_str().unwrap()),
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
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let mut poll = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut poll, 1, 10_000) } <= 0 {
            return Err(io::Error::other("checker job did not connect within 10 seconds"));
        }
        let (mut ready, _) = listener.accept()?;
        ready.write_all(&[1])?;
        ready.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut code = [0; 4];
        let mut received = 0;
        while received < code.len() {
            if INTERRUPTED.load(Ordering::Relaxed) != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match ready.read(&mut code[received..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(count) => received += count,
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {},
                Err(error) => return Err(error),
            }
        }
        drop(interrupts.take());
        // During blocking output, termination closes the socket so the worker cleans up.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
        }
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        relay(&job.dir, "stdout", &mut io::stdout().lock())?;
        relay(&job.dir, "stderr", &mut io::stderr().lock())?;
        ready.write_all(&[1])?;
        // Only successful delivery leaves cleanup exclusively to the worker.
        job.handed_off = true;
        Ok(i32::from_be_bytes(code))
    })();
    let signal = INTERRUPTED.load(Ordering::Relaxed);
    if signal != 0 { return Ok(128 + signal); }
    match result {
        Ok(code) => Ok(code),
        Err(error) => {
            let _ = relay(&job.dir, "worker-stderr", &mut io::stderr().lock());
            eprintln!("herdr-app: isolated checker failed: {error}");
            Ok(1)
        }
    }
}

fn relay(dir: &Path, name: &str, out: &mut dyn Write) -> io::Result<()> {
    match File::open(dir.join(name)) {
        Ok(mut file) => { io::copy(&mut file, out)?; Ok(()) }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checker_exit_status_is_preserved() {
        let (worker, _caller) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sh").args(["-c", "exit 23"]).process_group(0).spawn().unwrap();
        assert_eq!(wait_for_checker(child, &worker).unwrap().unwrap().code(), Some(23));
    }

    #[test]
    fn disconnected_caller_kills_and_reaps_checker() {
        let (worker, caller) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        drop(caller);
        assert!(wait_for_checker(child, &worker).unwrap().is_none());
        assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    fn worker_waits_for_ack_or_disconnection() {
        for acknowledge in [true, false] {
            let (mut worker, mut caller) = UnixStream::pair().unwrap();
            std::thread::scope(|scope| {
                let task = scope.spawn(|| wait_for_ack(&mut worker).unwrap());
                std::thread::sleep(Duration::from_millis(150));
                assert!(!task.is_finished());
                if acknowledge { caller.write_all(&[1]).unwrap(); }
                drop(caller);
                task.join().unwrap();
            });
        }
    }

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

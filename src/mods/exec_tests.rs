use super::*;
use std::io::Write;
use std::time::Instant;

fn fixture_args(name: &str) -> Vec<String> {
    vec!["--exact".into(), format!("mods::exec_tests::{name}"), "--ignored".into(), "--nocapture".into()]
}

fn run_fixture(name: &str, limit: Duration) -> Result<(i32, String, String), String> {
    run_blocking(std::env::current_exe().unwrap().to_str().unwrap(), &fixture_args(name), None, limit)
}

#[test]
fn exited_child_with_inherited_pipes_times_out() {
    for fixture in ["hold_stdout", "hold_stderr", "hold_both"] {
        let start = Instant::now();
        let error = run_fixture(fixture, Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("output pipes did not close"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(4), "{fixture} waited for the descendant");
    }
}

#[test]
fn descendant_output_closes_before_deadline() {
    let (code, out, err) = run_fixture("short_descendant", Duration::from_secs(5)).unwrap();
    assert_eq!(code, 0);
    assert!(out.contains("descendant output"));
    assert!(err.contains("descendant error"));
}

#[test]
fn running_child_times_out() {
    let start = Instant::now();
    let error = run_fixture("sleep", Duration::from_secs(1)).unwrap_err();
    assert!(error.contains("was stopped"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(4));
    // A timeout must not leave the next invocation stuck.
    assert_eq!(run_fixture("output", Duration::from_secs(5)).unwrap().0, 7);
}

#[test]
fn output_and_exit_code_are_preserved() {
    let (code, out, err) = run_fixture("output", Duration::from_secs(5)).unwrap();
    assert_eq!(code, 7);
    assert!(out.contains("hello\u{fffd}"));
    assert_eq!(err, "error");
}

#[test]
fn output_is_truncated_but_both_pipes_are_drained() {
    let (code, out, err) = run_fixture("large_output", Duration::from_secs(10)).unwrap();
    assert_eq!(code, 0);
    assert_eq!(out.len(), EXEC_OUTPUT as usize);
    assert_eq!(err.len(), EXEC_OUTPUT as usize);
}

#[test]
fn missing_program_is_an_error() {
    let path = std::env::temp_dir().join(format!("herdr-no-such-executable-{}", std::process::id()));
    assert!(run_blocking(path.to_str().unwrap(), &[], None, Duration::from_secs(1)).unwrap_err().contains("cannot run"));
}

#[test]
fn dropping_reader_closes_pipe_and_joins_thread() {
    for iteration in 0..5 {
        let mut child = crate::spawn::command(std::env::current_exe().unwrap())
            .args(fixture_args("sleep"))
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let reader = exec_reader::Reader::new(child.stdout.take().unwrap(), EXEC_OUTPUT).unwrap();
        // Let the Windows reader enter a blocking read, also exercising the
        // cancellation race on the other iterations without a delay.
        if iteration == 0 { std::thread::sleep(Duration::from_millis(100)); }
        let start = Instant::now();
        drop(reader);
        let elapsed = start.elapsed();
        let alive = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        assert!(alive, "reader cancellation must not depend on the child exiting");
        assert!(elapsed < Duration::from_secs(2));
    }
}

// Subprocess fixtures use the test executable so they also run on Windows.
// Descendants terminate themselves even when testing the broken implementation.
fn descendant(fixture: &str, stdout: bool, stderr: bool) {
    crate::spawn::command(std::env::current_exe().unwrap())
        .args(fixture_args(fixture))
        .stdin(Stdio::null())
        .stdout(if stdout { Stdio::inherit() } else { Stdio::null() })
        .stderr(if stderr { Stdio::inherit() } else { Stdio::null() })
        .spawn().unwrap();
}

#[test] #[ignore]
fn hold_stdout() { descendant("sleep", true, false); }
#[test] #[ignore]
fn hold_stderr() { descendant("sleep", false, true); }
#[test] #[ignore]
fn hold_both() { descendant("sleep", true, true); }
#[test] #[ignore]
fn sleep() { std::thread::sleep(Duration::from_secs(8)); }
#[test] #[ignore]
fn output() {
    std::io::stdout().write_all(b"hello\xff").unwrap();
    std::io::stdout().flush().unwrap();
    std::io::stderr().write_all(b"error").unwrap();
    std::io::stderr().flush().unwrap();
    std::process::exit(7);
}
#[test] #[ignore]
fn large_output() {
    let err = std::thread::spawn(|| std::io::stderr().write_all(&vec![b'e'; EXEC_OUTPUT as usize + 8192]).unwrap());
    std::io::stdout().write_all(&vec![b'o'; EXEC_OUTPUT as usize + 8192]).unwrap();
    err.join().unwrap();
}

#[test] #[ignore]
fn short_descendant() { descendant("delayed_output", true, true); }
#[test] #[ignore]
fn delayed_output() {
    std::thread::sleep(Duration::from_millis(100));
    println!("descendant output");
    eprintln!("descendant error");
}

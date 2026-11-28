//! `herdr-app do <command> [--arg value ... | json]`, `herdr-app ls`, `herdr-app commands`
//! and `herdr-app status`: drive a running window from a shell (agents, tests)
//! over `$HERDR_APP_HOME/herdr-app.sock`, one JSON line each way.

use crate::mods::{self, Request};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

fn socket() -> PathBuf {
    mods::app_dir().join("herdr-app.sock")
}

/// Serves the socket for this window. A live socket means another window owns this home.
pub fn listen(tx: async_channel::Sender<Request>) {
    let path = socket();
    if UnixStream::connect(&path).is_ok() {
        tracing::warn!("another Herdr is serving {}; `herdr-app do` goes there", path.display());
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return tracing::warn!("cannot serve {}: {e}", path.display()),
    };
    let _ = std::thread::Builder::new().name("herdr-app-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    });
}

fn serve(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(cmd) => {
            let timeout = Duration::from_secs(if cmd.get("shot").is_some() { 30 } else { 5 });
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let _ = tx.send_blocking(Request::Control { cmd, reply: reply_tx });
            reply_rx.recv_timeout(timeout).unwrap_or_else(|_| json!({ "error": "the window did not answer" }))
        }
        Err(e) => json!({ "error": format!("bad request: {e}") }),
    };
    send_reply(&stream, &reply);
}

/// A non-blocking socket takes one send buffer's worth (8 KiB on macOS) and
/// refuses the rest, which cut a large `ls` reply short mid-JSON.
fn send_reply(stream: &UnixStream, reply: &Value) {
    let _ = stream.set_nonblocking(false);
    if let Err(e) = (&*stream).write_all(format!("{reply}\n").as_bytes()) {
        tracing::warn!("control reply not sent whole: {e}");
    }
}

/// One Herdr per home: a second launch brings the running window to the
/// front and quits. True when one is running (even one too busy to answer:
/// a second window would ring every alert twice).
pub fn raise_running() -> bool {
    raise_running_at(&socket())
}

fn raise_running_at(path: &std::path::Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    if stream.write_all(format!("{}\n", json!({ "raise": true })).as_bytes()).is_ok() {
        let _ = BufReader::new(&stream).read_line(&mut String::new());
    }
    true
}

/// The client side; returns the process exit code.
pub fn client(args: &[String]) -> i32 {
    if args == ["commands"] {
        println!("{}", serde_json::to_string_pretty(&crate::commands::listing()).unwrap());
        return 0;
    }
    let cmd = match args {
        [verb, rest @ ..] if verb == "shot" && rest.len() <= 1 => {
            let path = PathBuf::from(rest.first().map(String::as_str).unwrap_or("herdr-shot.png"));
            let path = match std::path::absolute(path) {
                Ok(path) => path,
                Err(e) => return fail(&format!("cannot resolve screenshot path: {e}")),
            };
            if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("png")) {
                return fail("screenshot path must end in .png");
            }
            json!({ "shot": path })
        },
        [verb] if verb == "status" => json!({ "status": true }),
        [verb] if verb == "ls" => json!({ "ls": true }),
        [verb, message] if verb == "feedback" => json!({ "do": "app.feedback", "args": { "message": message, "from": "agent" } }),
        [verb, command, rest @ ..] if verb == "do" => match parse_args(rest) {
            Ok(a) => json!({ "do": command, "args": a }),
            Err(e) => return fail(&e),
        },
        _ => return fail("usage: herdr-app ls | herdr-app commands | herdr-app status | herdr-app shot [file.png] | herdr-app do <command> [--arg value ... | json]"),
    };
    let stream = match UnixStream::connect(socket()) {
        Ok(s) => s,
        Err(_) if args.first().is_some_and(|v| v == "feedback") => {
            let body = crate::feedback::draft(&args[1], "", 0, "agent", "other");
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            eprintln!("Herdr must be open to review and send this feedback. Nothing was sent; core version and layout will reflect the open window.");
            return 1;
        }
        Err(e) => return fail(&format!("no Herdr window at {}: {e}", socket().display())),
    };
    let reply = match exchange(stream, &cmd, Some(Duration::from_secs(35))) {
        Ok(reply) => reply,
        Err(e) => return fail(&e),
    };
    if args.first().is_some_and(|v| v == "shot") {
        if let Some(error) = reply.get("error") {
            return fail(error.as_str().unwrap_or("screenshot failed"));
        }
        let Some(path) = reply.get("path").and_then(Value::as_str) else {
            return fail("the window returned no screenshot path");
        };
        println!("{path}");
    } else {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    }
    if reply.get("error").is_some() { 1 } else { 0 }
}

/// Sends one request line and reads the reply line. A window that closes the
/// socket without a line, or answers with something that isn't JSON, is an error,
/// never a `null` reply.
fn exchange(mut stream: UnixStream, cmd: &Value, timeout: Option<Duration>) -> Result<Value, String> {
    let _ = stream.set_read_timeout(timeout);
    let mut line = String::new();
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() || BufReader::new(&stream).read_line(&mut line).map_or(true, |n| n == 0) {
        return Err("the window hung up".into());
    }
    serde_json::from_str(&line).map_err(|e| format!("bad reply from the window: {e}"))
}

/// `--name api --argv '["pi"]'` (values that parse as JSON are JSON), or one JSON object.
fn parse_args(rest: &[String]) -> Result<Value, String> {
    match rest {
        [] => Ok(Value::Null),
        [raw] if raw.trim_start().starts_with('{') => serde_json::from_str(raw).map_err(|e| format!("args are not JSON: {e}")),
        _ => {
            let mut out = serde_json::Map::new();
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let key = flag.strip_prefix("--").ok_or_else(|| format!("expected --<arg>, got {flag:?}"))?;
                let value = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
                let v = serde_json::from_str::<Value>(value).ok().filter(|v| !v.is_string()).unwrap_or_else(|| Value::String(value.clone()));
                out.insert(key.to_owned(), v);
            }
            Ok(Value::Object(out))
        }
    }
}

/// Encode before opening the destination: GPU errors must not truncate a good shot.
pub fn save_shot<C: gpui::AppContext>(
    window: gpui::AnyWindowHandle,
    cx: &mut C,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let image = window.render_current_to_image(cx)?;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        encoder.write_header()?.write_image_data(image.as_raw())?;
    }
    // A sibling temporary file keeps publication on the same filesystem.
    let temporary = path.with_file_name(format!(".herdr-shot-{}.png", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn fail(message: &str) -> i32 {
    eprintln!("herdr-app: {message}");
    2
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `herdr-app ls` once printed `null` for a reply that wasn't a whole JSON
    /// line: an empty or non-JSON reply is an error instead.
    #[test]
    fn a_reply_that_is_not_json_is_an_error_not_null() {
        let answer = |reply: &'static [u8]| {
            let (client, server) = UnixStream::pair().unwrap();
            let window = std::thread::spawn(move || {
                BufReader::new(&server).read_line(&mut String::new()).unwrap();
                (&server).write_all(reply).unwrap();
            });
            let got = exchange(client, &json!({ "ls": true }), Some(Duration::from_secs(5)));
            window.join().unwrap();
            got
        };
        assert_eq!(answer(b""), Err("the window hung up".into()));
        assert!(answer(b"{\"spaces\":[").unwrap_err().starts_with("bad reply from the window"));
        assert_eq!(answer(b"{\"spaces\":[]}\n"), Ok(json!({ "spaces": [] })));
    }

    /// An `ls` reply larger than the socket's send buffer once stopped at
    /// exactly 8 KiB: the server writes it whole even to a non-blocking socket.
    #[test]
    fn a_reply_larger_than_the_send_buffer_arrives_whole() {
        let (client, server) = UnixStream::pair().unwrap();
        (&client).write_all(b"{\"ls\":true}\n").unwrap();
        server.set_nonblocking(true).unwrap();
        let (tx, rx) = async_channel::unbounded();
        let served = std::thread::spawn(move || serve(server, tx));
        let Ok(Request::Control { reply, .. }) = rx.recv_blocking() else { panic!("no control request") };
        let big = json!({ "spaces": "x".repeat(4 << 20) });
        reply.send(big.clone()).unwrap();
        // Let the server fill the buffer before anything drains it.
        std::thread::sleep(Duration::from_millis(200));
        let mut line = String::new();
        BufReader::new(&client).read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), big);
        served.join().unwrap();
    }

    #[test]
    fn flag_values_that_parse_as_json_are_json() {
        let args: Vec<String> = ["--name", "eng-123", "--focus", "false", "--argv", r#"["claude"]"#].map(String::from).into();
        assert_eq!(parse_args(&args).unwrap(), json!({ "name": "eng-123", "focus": false, "argv": ["claude"] }));
    }

    #[test]
    fn a_second_launch_raises_the_running_window() {
        let dir = std::env::temp_dir().join(format!("herdr-app-raise-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("herdr-app.sock");
        // Nothing running: this launch opens its own window.
        assert!(!raise_running_at(&path));
        // One running: it gets asked to come to the front.
        let listener = UnixListener::bind(&path).unwrap();
        let asked = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"ok\":true}\n").unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        assert!(raise_running_at(&path));
        assert_eq!(asked.join().unwrap(), json!({ "raise": true }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

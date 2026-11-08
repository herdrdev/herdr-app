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
pub fn listen(tx: async_channel::Sender<Request>) -> Result<(), String> {
    let path = socket();
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("another Herdr window is already serving {}", path.display()));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("cannot serve {}: {e}", path.display()))?;
    std::thread::Builder::new().name("herdr-app-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    }).map_err(|e| format!("cannot start Herdr control listener: {e}"))?;
    Ok(())
}

fn serve(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(cmd) => {
            let timeout = if cmd.get("open").is_some() { crate::open::WAIT + Duration::from_secs(5) }
                else { Duration::from_secs(if cmd.get("shot").is_some() { 30 } else { 5 }) };
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let _ = tx.send_blocking(Request::Control { cmd, reply: reply_tx });
            reply_rx.recv_timeout(timeout).unwrap_or_else(|_| json!({ "error": "the window did not answer" }))
        }
        Err(e) => json!({ "error": format!("bad request: {e}") }),
    };
    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
}

/// One Herdr per home: a second launch brings the running window to the
/// front and quits. True when one is running (even one too busy to answer:
/// a second window would ring every alert twice).
pub fn raise_running() -> bool {
    raise_running_at(&socket())
}

/// None means no listener. Once connected, an uncertain reply must never start another window.
pub fn open_running(folder: &str) -> Result<Option<Value>, String> {
    open_running_at(&socket(), folder)
}

fn open_running_at(path: &std::path::Path, folder: &str) -> Result<Option<Value>, String> {
    let mut stream = match UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => return Ok(None),
        Err(e) => return Err(format!("cannot contact Herdr: {e}")),
    };
    stream.set_read_timeout(Some(crate::open::WAIT + Duration::from_secs(10))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    stream.write_all(format!("{}\n", crate::open::request(folder)).as_bytes()).map_err(|e| format!("cannot send folder to Herdr: {e}"))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| format!("Herdr did not confirm opening the folder: {e}"))?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| format!("invalid reply from Herdr: {e}"))?;
    if reply.get("space").is_none() && reply.get("error").is_none() { return Err("Herdr did not confirm opening the folder".into()); }
    Ok(Some(reply))
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
    let mut stream = match UnixStream::connect(socket()) {
        Ok(s) => s,
        Err(_) if args.first().is_some_and(|v| v == "feedback") => {
            let body = crate::feedback::draft(&args[1], "", 0, "agent", "other");
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            eprintln!("Herdr must be open to review and send this feedback. Nothing was sent; core version and layout will reflect the open window.");
            return 1;
        }
        Err(e) => return fail(&format!("no Herdr window at {}: {e}", socket().display())),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(35)));
    let mut line = String::new();
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() || BufReader::new(&stream).read_line(&mut line).is_err() {
        return fail("the window hung up");
    }
    let reply: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
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
pub fn save_shot(window: &gpui::Window, path: &std::path::Path) -> anyhow::Result<()> {
    let image = window.render_to_image()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_passes_the_literal_folder_and_does_not_retry_a_broken_reply() {
        let dir = std::env::temp_dir().join(format!("herdr-open-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.sock");
        assert_eq!(open_running_at(&path, ".").unwrap(), None);
        let listener = UnixListener::bind(&path).unwrap();
        let asked = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            stream.write_all(b"broken reply\n").unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        let folder = "D:\\project & 'quoted'\\тест\\";
        assert!(open_running_at(&path, folder).is_err());
        let request = asked.join().unwrap();
        assert_eq!(request["open"], folder);
        assert!(request["open_until"].as_u64().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_preserves_success_and_errors_and_rejects_empty_confirmations() {
        let dir = std::env::temp_dir().join(format!("herdr-open-replies-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            for reply in ["{\"space\":\"sp1\"}\n", "{\"error\":\"bad cwd\"}\n", "null\n"] {
                let (mut stream, _) = listener.accept().unwrap();
                BufReader::new(&stream).read_line(&mut String::new()).unwrap();
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        assert_eq!(open_running_at(&path, ".").unwrap(), Some(json!({ "space": "sp1" })));
        assert_eq!(open_running_at(&path, ".").unwrap(), Some(json!({ "error": "bad cwd" })));
        assert!(open_running_at(&path, ".").is_err());
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
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

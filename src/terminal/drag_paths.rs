//! Routing and pasting paths without treating another machine's files as local.

use gpui_shell::DraggedPaths;
use std::path::PathBuf;

fn validate_paths(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Result<(), &'static str> {
    let mut any = false;
    for path in paths {
        any = true;
        let path = path.as_ref();
        // Even inside shell quotes, a newline becomes Enter in an unbracketed paste.
        if path.is_empty() || path.chars().any(char::is_control) {
            return Err("Paths containing control characters cannot be pasted safely");
        }
    }
    if any { Ok(()) } else { Err("No files were dropped") }
}

pub(super) fn same_machine(paths: &DraggedPaths, target: &str) -> Result<bool, &'static str> {
    validate_paths(&paths.paths)?;
    match paths.on.as_deref() {
        None => Ok(false),
        Some(source) if target.split_once('/').map(|(machine, _)| machine) == Some(source) => Ok(true),
        Some(_) => Err("Drop these files into a terminal on their own machine. Copying from another machine is not supported yet."),
    }
}

pub(super) fn validate_upload(paths: &[PathBuf]) -> Result<(), &'static str> {
    validate_paths(paths.iter().map(|p| p.to_string_lossy()))?;
    if paths.iter().any(|p| p.is_dir()) {
        return Err("Copying local folders to another machine is not supported yet. Drop the folder into a terminal on this computer instead.");
    }
    Ok(())
}

/// Paths as the destination shell reads them, with a trailing space, never Enter.
pub(super) fn quote_paths(paths: impl IntoIterator<Item = String>, target: &str, local_windows: bool) -> Result<String, &'static str> {
    let paths: Vec<_> = paths.into_iter().collect();
    validate_paths(&paths)?;
    // Remote Files and file transfers currently target Unix machines, even from Windows.
    let windows = local_windows && !target.contains('/');
    let quote = |p: String| {
        if p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%~".contains(c) || (!windows && c == ',')) {
            return p;
        }
        if windows { format!("'{}'", p.replace('\'', "''")) } else { format!("'{}'", p.replace('\'', "'\\''")) }
    };
    let mut out = paths.into_iter().map(quote).collect::<Vec<_>>().join(" ");
    out.push(' ');
    Ok(out)
}

pub(super) fn unavailable(body: &str) {
    crate::mods::request(crate::mods::Request::OpenOverlay {
        id: crate::mods::new_overlay_id(),
        spec: serde_json::json!({"kind":"toast", "notification":{"title":"File drop unavailable", "body":body, "level":"info"}}),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(on: Option<&str>) -> DraggedPaths {
        DraggedPaths { paths: vec!["/work/中文 O'Brien/notes.txt".into(), "/work/a folder".into()], on: on.map(str::to_owned) }
    }

    #[test]
    fn local_sources_keep_existing_local_or_upload_handling() {
        assert_eq!(same_machine(&paths(None), "t1"), Ok(false));
        assert_eq!(same_machine(&paths(None), "remote/t1"), Ok(false));
    }

    #[test]
    fn remote_paths_are_pasted_only_on_their_source_machine() {
        assert_eq!(same_machine(&paths(Some("remote")), "remote/t1"), Ok(true));
        for target in ["t1", "another/t1", "remote-copy/t1"] {
            assert!(same_machine(&paths(Some("remote")), target).is_err(), "{target}");
        }
    }

    #[test]
    fn dropped_paths_are_typed_as_the_shell_reads_them() {
        let q = |v: &[&str]| quote_paths(v.iter().map(|s| s.to_string()), "t1", false).unwrap();
        assert_eq!(q(&["/tmp/shot.png"]), "/tmp/shot.png ");
        assert_eq!(q(&["/tmp/my shot.png", "/a/b"]), "'/tmp/my shot.png' /a/b ");
        assert_eq!(q(&["/tmp/it's.png"]), "'/tmp/it'\\''s.png' ");
        assert_eq!(q(&["/tmp/a,b.txt"]), "/tmp/a,b.txt ");
    }

    #[test]
    fn remote_paths_use_posix_quoting_from_either_viewer() {
        for local_windows in [false, true] {
            assert_eq!(
                quote_paths(vec!["/remote/O'Brien.png".into()], "remote/t1", local_windows).unwrap(),
                "'/remote/O'\\''Brien.png' ",
            );
        }
    }

    #[test]
    fn powershell_paths_quote_apostrophes_and_commas() {
        let q = |p: &str| quote_paths(vec![p.into()], "t1", true).unwrap();
        assert_eq!(q("C:/Project/O'Brien.png"), "'C:/Project/O''Brien.png' ");
        assert_eq!(q("C:/Project/a,b.txt"), "'C:/Project/a,b.txt' ");
        assert_eq!(q("C:/Project/notes.txt"), "C:/Project/notes.txt ");
        assert_eq!(q("C:\\Project\\a,b.txt"), "'C:\\Project\\a,b.txt' ");
    }

    #[test]
    fn path_pastes_quote_special_names_without_enter_in_either_paste_mode() {
        let files = paths(None);
        for local_windows in [false, true] {
            for target in ["t1", "remote/t1"] {
                let text = quote_paths(files.paths.clone(), target, local_windows).unwrap();
                let apostrophe = if local_windows && target == "t1" { "''" } else { "'\\''" };
                assert_eq!(text, format!("'/work/中文 O{apostrophe}Brien/notes.txt' '/work/a folder' "));
                let mut vt = crate::vt::Vt::new(80, 24);
                for bracketed in [false, true] {
                    if bracketed { vt.write(b"\x1b[?2004h"); }
                    let bytes = vt.paste_bytes(&text);
                    assert!(!bytes.contains(&b'\r') && !bytes.contains(&b'\n'));
                    let expected = if bracketed { format!("\x1b[200~{text}\x1b[201~").into_bytes() } else { text.as_bytes().to_vec() };
                    assert_eq!(bytes, expected);
                }
            }
        }
    }

    #[test]
    fn control_characters_are_rejected_at_the_shared_paste_boundary() {
        for path in ["/work/a\nb", "/work/a\rb", "/work/\x1b[201~", "/work/\tfile", "/work/\x7ffile", ""] {
            let files = DraggedPaths { paths: vec![path.into()], on: None };
            assert!(same_machine(&files, "t1").is_err(), "{path:?}");
            assert!(validate_upload(&[PathBuf::from(path)]).is_err(), "{path:?}");
            for target in ["t1", "remote/t1"] {
                for local_windows in [false, true] {
                    assert!(quote_paths(files.paths.clone(), target, local_windows).is_err(), "{path:?}");
                    assert!(quote_paths(vec!["/work/safe.txt".into(), path.into()], target, local_windows).is_err(), "{path:?}");
                }
            }
        }
        assert!(same_machine(&DraggedPaths { paths: vec![], on: None }, "t1").is_err());
        assert!(quote_paths(vec![], "t1", false).is_err());
        assert!(validate_upload(&[]).is_err());
    }

    #[test]
    fn folder_uploads_are_rejected_before_any_transfer() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = Scratch(std::env::temp_dir().join(format!("herdr-paths-{}-{stamp}", std::process::id())));
        std::fs::create_dir(&root.0).unwrap();
        let file = root.0.join("O'Brien 中文.txt");
        std::fs::write(&file, "test").unwrap();
        assert!(validate_upload(std::slice::from_ref(&file)).is_ok());
        let error = validate_upload(std::slice::from_ref(&root.0)).unwrap_err();
        assert!(error.contains("Copying local folders"));
        assert_eq!(validate_upload(&[file, root.0.clone()]), Err(error));
        // Reject only the transfer: local and same-machine remote folder paths still paste.
        assert!(quote_paths(vec![root.0.to_string_lossy().into_owned()], "t1", cfg!(windows)).is_ok());
        assert!(quote_paths(vec!["/remote/a folder".into()], "remote/t1", true).is_ok());
        #[cfg(unix)]
        {
            let link = root.0.join("folder-link");
            std::os::unix::fs::symlink(&root.0, &link).unwrap();
            assert_eq!(validate_upload(&[link]), Err(error));
        }
    }
}

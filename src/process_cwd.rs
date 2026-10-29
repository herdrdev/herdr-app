use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub fn read(pid: u64) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "macos")]
pub fn read(pid: u64) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    // The kernel writes a fixed-size record; reject failed or partial reads.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let bytes: Vec<u8> = info
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|c| *c as u8)
        .collect();
    let end = bytes.iter().position(|b| *b == 0)?;
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        bytes[..end].to_vec(),
    )))
}

#[cfg(target_os = "openbsd")]
pub fn read(pid: u64) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC_CWD, pid];
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let mut size = buf.len();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buf.truncate(size);
    let end = buf.iter().position(|b| *b == 0)?;
    buf.truncate(end);
    (!buf.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "openbsd")))]
pub fn read(_pid: u64) -> Option<PathBuf> {
    None
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "openbsd")
))]
mod tests {
    use super::read;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    #[test]
    fn follows_a_running_process_between_folders() {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "while IFS= read -r dir; do cd \"$dir\" || exit; printf 'ready\\n'; done",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut observed = Vec::new();
        let expected = [
            std::path::PathBuf::from("/"),
            std::env::temp_dir().canonicalize().unwrap(),
        ];
        for folder in &expected {
            writeln!(input, "{}", folder.display()).unwrap();
            let mut ready = String::new();
            output.read_line(&mut ready).unwrap();
            observed.push(read(child.id().into()));
        }
        drop(input);
        child.wait().unwrap();
        assert_eq!(observed, expected.into_iter().map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn missing_process_has_no_folder() {
        assert_eq!(read(i32::MAX as u64), None);
        assert_eq!(read(u64::MAX), None);
    }
}

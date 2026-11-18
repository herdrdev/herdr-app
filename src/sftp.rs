//! Native SFTP v3 service reusing herdr-core's configured SSH machines.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const SSH_FXP_INIT: u8 = 1;
const SSH_FXP_VERSION: u8 = 2;
const SSH_FXP_OPEN: u8 = 3;
const SSH_FXP_CLOSE: u8 = 4;
const SSH_FXP_READ: u8 = 5;
const SSH_FXP_WRITE: u8 = 6;
const SSH_FXP_LSTAT: u8 = 7;
const SSH_FXP_MKDIR: u8 = 14;
const SSH_FXP_RMDIR: u8 = 15;
const SSH_FXP_REALPATH: u8 = 16;
const SSH_FXP_STAT: u8 = 17;
const SSH_FXP_RENAME: u8 = 18;
const SSH_FXP_OPENDIR: u8 = 11;
const SSH_FXP_READDIR: u8 = 12;
const SSH_FXP_REMOVE: u8 = 13;

const SSH_FXP_STATUS: u8 = 101;
const SSH_FXP_HANDLE: u8 = 102;
const SSH_FXP_DATA: u8 = 103;
const SSH_FXP_NAME: u8 = 104;
const SSH_FXP_ATTRS: u8 = 105;

const SSH_FX_OK: u32 = 0;
const SSH_FX_EOF: u32 = 1;
const SSH_FX_NO_SUCH_FILE: u32 = 2;
const SSH_FX_PERMISSION_DENIED: u32 = 3;
const SSH_FX_FAILURE: u32 = 4;
const SSH_FX_NO_CONNECTION: u32 = 6;
const SSH_FX_CONNECTION_LOST: u32 = 7;

const SSH_FXF_READ: u32 = 0x0000_0001;
const SSH_FXF_WRITE: u32 = 0x0000_0002;
const SSH_FXF_CREAT: u32 = 0x0000_0008;
const SSH_FXF_TRUNC: u32 = 0x0000_0010;

const SSH_FILEXFER_ATTR_SIZE: u32 = 0x0000_0001;
const SSH_FILEXFER_ATTR_UIDGID: u32 = 0x0000_0002;
const SSH_FILEXFER_ATTR_PERMISSIONS: u32 = 0x0000_0004;
const SSH_FILEXFER_ATTR_ACMODTIME: u32 = 0x0000_0008;
const SSH_FILEXFER_ATTR_EXTENDED: u32 = 0x8000_0000;

const MAX_PACKET_BYTES: usize = 16 << 20;
const CHUNK_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified_at: Option<u64>,
    pub permissions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadedFile {
    pub local_path: String,
    pub remote_path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpError {
    Transport(String),
    Status { code: u32, message: String },
    InvalidPath(String),
    LocalIo(String),
}

impl std::fmt::Display for SftpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "{msg}"),
            Self::Status { code: SSH_FX_PERMISSION_DENIED, message } => {
                if message.contains("EACCES") {
                    write!(f, "{message}")
                } else {
                    write!(f, "EACCES: Permission denied ({message})")
                }
            }
            Self::Status { code: SSH_FX_NO_SUCH_FILE, message } => {
                write!(f, "ENOENT: No such file or directory ({message})")
            }
            Self::Status { code, message } => write!(f, "SFTP error {code}: {message}"),
            Self::InvalidPath(msg) | Self::LocalIo(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SftpError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineTarget {
    pub name: String,
    pub target: String,
    pub via: bool,
}

/// Normalizes any remote Linux path using strict POSIX `/` separators so
/// Windows backslashes never leak into SFTP requests.
pub fn normalize_posix_path(raw: &str) -> String {
    let slash = raw.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in slash.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Joins a remote POSIX directory and a single child entry name safely.
pub fn join_posix_path(dir: &str, name: &str) -> Result<String, SftpError> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(SftpError::InvalidPath(format!("Invalid remote entry name: {name:?}")));
    }
    let base = normalize_posix_path(dir);
    if base == "/" {
        Ok(format!("/{name}"))
    } else {
        Ok(format!("{base}/{name}"))
    }
}

fn validate_remote_path(raw: &str) -> Result<String, SftpError> {
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err(SftpError::InvalidPath("Remote path must not be empty".to_owned()));
    }
    Ok(normalize_posix_path(raw))
}

fn ssh_target(target: String) -> String {
    let host = target.rsplit_once('@').map_or(target.as_str(), |(_, host)| host);
    match host.split_once(':') {
        Some((h, port))
            if !target.starts_with("ssh://")
                && !h.is_empty()
                && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("ssh://{target}")
        }
        _ => target,
    }
}

fn parse_machine_target(json_text: &str, machine_id: &str) -> Result<MachineTarget, String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("Cannot parse machine list: {e}"))?;
    let list = v["machines"].as_array().ok_or_else(|| "No machines returned by herdr-core".to_owned())?;
    let m = list
        .iter()
        .find(|m| m["name"].as_str() == Some(machine_id) || m["id"].as_str() == Some(machine_id))
        .ok_or_else(|| format!("Unknown remote machine {machine_id:?}"))?;
    let name = m["name"].as_str().unwrap_or(machine_id).to_owned();
    let via = m["via"].as_bool().unwrap_or(false);
    let raw_target = m["target"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Machine {name:?} has no SSH target configured"))?
        .to_owned();
    if raw_target.chars().any(char::is_control) {
        return Err(format!("Invalid SSH target for {name:?}"));
    }
    if !via && (raw_target.starts_with('-') || raw_target.chars().any(char::is_whitespace)) {
        return Err(format!("Invalid SSH target {raw_target:?}"));
    }
    let target = if via { raw_target } else { ssh_target(raw_target) };
    Ok(MachineTarget { name, target, via })
}

fn resolve_machine(machine_id: &str) -> Result<MachineTarget, String> {
    let (code, stdout, stderr) = crate::mods::run_blocking(
        "hc",
        &["machine".into(), "ls".into(), "--json".into()],
        None,
        Duration::from_secs(10),
    )?;
    if code != 0 {
        let msg = stderr.trim();
        return Err(if msg.is_empty() {
            format!("hc machine ls failed with exit code {code}")
        } else {
            msg.to_owned()
        });
    }
    parse_machine_target(&stdout, machine_id)
}

#[cfg(unix)]
fn control_socket_for(target: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    target.hash(&mut h);
    let name = format!("ssh-{:012x}", h.finish() & 0xffff_ffff_ffff);
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(dir).join("herdr-core").join(&name));
    }
    if let Some(dir) = std::env::var_os("TMPDIR").filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(dir).join("herdr-core").join(&name));
    }
    candidates.push(std::env::temp_dir().join("herdr-core").join(&name));
    candidates.into_iter().find(|p| p.exists())
}

fn spawn_sftp_process(machine: &MachineTarget) -> Result<(Child, Arc<Mutex<String>>), SftpError> {
    let mut cmd = if machine.via {
        let remote = "exec /usr/lib/openssh/sftp-server 2>/dev/null || exec /usr/libexec/openssh/sftp-server 2>/dev/null || exec sftp-server";
        #[cfg(unix)]
        {
            let mut c = crate::spawn::command("sh");
            c.arg("-c").arg(format!("{} '{}'", machine.target, remote));
            c
        }
        #[cfg(windows)]
        {
            let mut c = crate::spawn::command("cmd");
            c.args(["/d", "/c", &format!("{} \"{}\"", machine.target, remote)]);
            c
        }
    } else if let Some(fake) = std::env::var_os("HC_SSH") {
        let mut c = crate::spawn::command(fake);
        c.arg(&machine.target).args(["-s", "sftp"]);
        c
    } else {
        let mut c = crate::spawn::command("ssh");
        #[cfg(unix)]
        if let Some(sock) = control_socket_for(&machine.target) {
            c.arg("-S").arg(sock).args(["-o", "ControlMaster=no"]);
        }
        c.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            &machine.target,
            "-s",
            "sftp",
        ]);
        c
    };

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SftpError::Transport(format!("Cannot start SSH SFTP process for {}: {e}", machine.name)))?;

    let stderr_buf = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        std::thread::Builder::new()
            .name(format!("sftp-stderr-{}", machine.name))
            .spawn(move || {
                let mut tmp = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut tmp) {
                    if n == 0 {
                        break;
                    }
                    let chunk = String::from_utf8_lossy(&tmp[..n]);
                    let mut guard = buf.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if guard.len() < 16 * 1024 {
                        guard.push_str(&chunk);
                    }
                }
            })
            .ok();
    }
    Ok((child, stderr_buf))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct FileAttrs {
    size: Option<u64>,
    permissions: Option<u32>,
    mtime: Option<u32>,
}

impl FileAttrs {
    fn is_dir(&self) -> bool {
        self.permissions.is_some_and(|m| (m & 0o170000) == 0o040000)
    }

    fn is_symlink(&self) -> bool {
        self.permissions.is_some_and(|m| (m & 0o170000) == 0o120000)
    }

    fn permissions_string(&self, is_dir: bool, is_symlink: bool) -> String {
        let Some(mode) = self.permissions else {
            return if is_dir { "drwxr-xr-x".into() } else { "-rw-r--r--".into() };
        };
        let kind = if is_symlink {
            'l'
        } else if is_dir || (mode & 0o170000) == 0o040000 {
            'd'
        } else {
            '-'
        };
        let bit = |mask: u32, ch: char| if (mode & mask) != 0 { ch } else { '-' };
        let mut s = String::with_capacity(10);
        s.push(kind);
        s.push(bit(0o400, 'r'));
        s.push(bit(0o200, 'w'));
        s.push(if (mode & 0o4000) != 0 {
            if (mode & 0o100) != 0 { 's' } else { 'S' }
        } else {
            bit(0o100, 'x')
        });
        s.push(bit(0o040, 'r'));
        s.push(bit(0o020, 'w'));
        s.push(if (mode & 0o2000) != 0 {
            if (mode & 0o010) != 0 { 's' } else { 'S' }
        } else {
            bit(0o010, 'x')
        });
        s.push(bit(0o004, 'r'));
        s.push(bit(0o002, 'w'));
        s.push(if (mode & 0o1000) != 0 {
            if (mode & 0o001) != 0 { 't' } else { 'T' }
        } else {
            bit(0o001, 'x')
        });
        s
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn u32(&mut self) -> Result<u32, SftpError> {
        let end = self
            .pos
            .checked_add(4)
            .ok_or_else(|| SftpError::Transport("SFTP packet offset overflow".into()))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| SftpError::Transport("Unexpected end of SFTP packet (u32)".into()))?;
        self.pos = end;
        Ok(u32::from_be_bytes(slice.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, SftpError> {
        let end = self
            .pos
            .checked_add(8)
            .ok_or_else(|| SftpError::Transport("SFTP packet offset overflow".into()))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| SftpError::Transport("Unexpected end of SFTP packet (u64)".into()))?;
        self.pos = end;
        Ok(u64::from_be_bytes(slice.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], SftpError> {
        let len = self.u32()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| SftpError::Transport("SFTP packet length overflow".into()))?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| SftpError::Transport("Unexpected end of SFTP packet (bytes)".into()))?;
        self.pos = end;
        Ok(slice)
    }

    fn string(&mut self) -> Result<String, SftpError> {
        let b = self.bytes()?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }

    fn attrs(&mut self) -> Result<FileAttrs, SftpError> {
        let flags = self.u32()?;
        let mut attrs = FileAttrs::default();
        if (flags & SSH_FILEXFER_ATTR_SIZE) != 0 {
            attrs.size = Some(self.u64()?);
        }
        if (flags & SSH_FILEXFER_ATTR_UIDGID) != 0 {
            let _uid = self.u32()?;
            let _gid = self.u32()?;
        }
        if (flags & SSH_FILEXFER_ATTR_PERMISSIONS) != 0 {
            attrs.permissions = Some(self.u32()?);
        }
        if (flags & SSH_FILEXFER_ATTR_ACMODTIME) != 0 {
            let _atime = self.u32()?;
            attrs.mtime = Some(self.u32()?);
        }
        if (flags & SSH_FILEXFER_ATTR_EXTENDED) != 0 {
            let count = self.u32()?;
            for _ in 0..count {
                let _k = self.bytes()?;
                let _v = self.bytes()?;
            }
        }
        Ok(attrs)
    }
}

struct PacketBuilder {
    buf: Vec<u8>,
}

impl PacketBuilder {
    fn new(packet_type: u8) -> Self {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&[0, 0, 0, 0, packet_type]);
        Self { buf }
    }

    fn u32(mut self, v: u32) -> Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    fn u64(mut self, v: u64) -> Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    fn bytes(mut self, b: &[u8]) -> Self {
        self.buf.extend_from_slice(&(b.len() as u32).to_be_bytes());
        self.buf.extend_from_slice(b);
        self
    }

    fn str(self, s: &str) -> Self {
        self.bytes(s.as_bytes())
    }

    fn empty_attrs(self) -> Self {
        self.u32(0)
    }

    fn mode_attrs(self, mode: u32) -> Self {
        self.u32(SSH_FILEXFER_ATTR_PERMISSIONS).u32(mode)
    }

    fn finish(mut self) -> Vec<u8> {
        let len = (self.buf.len() - 4) as u32;
        self.buf[..4].copy_from_slice(&len.to_be_bytes());
        self.buf
    }
}

pub struct SftpClient<R: Read, W: Write> {
    reader: R,
    writer: W,
    next_id: u32,
}

impl<R: Read, W: Write> SftpClient<R, W> {
    pub fn connect(reader: R, writer: W) -> Result<Self, SftpError> {
        let mut client = Self { reader, writer, next_id: 1 };
        client.handshake()?;
        Ok(client)
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    fn write_packet(&mut self, packet: &[u8]) -> Result<(), SftpError> {
        self.writer
            .write_all(packet)
            .and_then(|()| self.writer.flush())
            .map_err(|e| SftpError::Transport(format!("SFTP write failed: {e}")))
    }

    fn read_packet(&mut self) -> Result<(u8, Vec<u8>), SftpError> {
        let mut len_buf = [0u8; 4];
        self.reader
            .read_exact(&mut len_buf)
            .map_err(|e| SftpError::Transport(format!("SFTP connection closed: {e}")))?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 || len > MAX_PACKET_BYTES {
            return Err(SftpError::Transport(format!("Invalid SFTP packet size: {len}")));
        }
        let mut payload = vec![0u8; len];
        self.reader
            .read_exact(&mut payload)
            .map_err(|e| SftpError::Transport(format!("SFTP read failed: {e}")))?;
        let ptype = payload[0];
        Ok((ptype, payload))
    }

    fn handshake(&mut self) -> Result<(), SftpError> {
        let init = PacketBuilder::new(SSH_FXP_INIT).u32(3).finish();
        self.write_packet(&init)?;
        let (ptype, payload) = self.read_packet()?;
        if ptype != SSH_FXP_VERSION {
            return Err(SftpError::Transport(format!("Expected SSH_FXP_VERSION, got {ptype}")));
        }
        let mut r = Reader::new(&payload[1..]);
        let version = r.u32()?;
        if version < 3 {
            return Err(SftpError::Transport(format!("Unsupported SFTP server version {version}")));
        }
        Ok(())
    }

    fn request(&mut self, packet: &[u8], expected_id: u32) -> Result<(u8, Vec<u8>), SftpError> {
        self.write_packet(packet)?;
        let (ptype, payload) = self.read_packet()?;
        let mut r = Reader::new(&payload[1..]);
        let id = r.u32()?;
        if id != expected_id {
            return Err(SftpError::Transport(format!(
                "SFTP response id mismatch (expected {expected_id}, got {id})"
            )));
        }
        Ok((ptype, payload))
    }

    fn parse_status(payload: &[u8]) -> Result<SftpError, SftpError> {
        let mut r = Reader::new(&payload[1..]);
        let _id = r.u32()?;
        let code = r.u32()?;
        let message = r.string().unwrap_or_default();
        if code == SSH_FX_NO_CONNECTION || code == SSH_FX_CONNECTION_LOST {
            return Ok(SftpError::Transport(if message.is_empty() {
                "SFTP connection lost".into()
            } else {
                message
            }));
        }
        Ok(SftpError::Status { code, message })
    }

    fn expect_ok(&mut self, packet: &[u8], id: u32) -> Result<(), SftpError> {
        let (ptype, payload) = self.request(packet, id)?;
        if ptype != SSH_FXP_STATUS {
            return Err(SftpError::Transport(format!("Expected SSH_FXP_STATUS, got {ptype}")));
        }
        match Self::parse_status(&payload)? {
            SftpError::Status { code: SSH_FX_OK, .. } => Ok(()),
            err => Err(err),
        }
    }

    fn expect_handle(&mut self, packet: &[u8], id: u32) -> Result<Vec<u8>, SftpError> {
        let (ptype, payload) = self.request(packet, id)?;
        match ptype {
            SSH_FXP_HANDLE => {
                let mut r = Reader::new(&payload[1..]);
                let _id = r.u32()?;
                Ok(r.bytes()?.to_vec())
            }
            SSH_FXP_STATUS => Err(Self::parse_status(&payload)?),
            other => Err(SftpError::Transport(format!("Expected SSH_FXP_HANDLE, got {other}"))),
        }
    }

    fn close_handle(&mut self, handle: &[u8]) {
        let id = self.alloc_id();
        let pkt = PacketBuilder::new(SSH_FXP_CLOSE).u32(id).bytes(handle).finish();
        let _ = self.expect_ok(&pkt, id);
    }

    pub fn realpath(&mut self, path: &str) -> Result<String, SftpError> {
        let clean = if path == "." { ".".to_owned() } else { validate_remote_path(path)? };
        let id = self.alloc_id();
        let pkt = PacketBuilder::new(SSH_FXP_REALPATH).u32(id).str(&clean).finish();
        let (ptype, payload) = self.request(&pkt, id)?;
        match ptype {
            SSH_FXP_NAME => {
                let mut r = Reader::new(&payload[1..]);
                let _id = r.u32()?;
                let count = r.u32()?;
                if count == 0 {
                    return Err(SftpError::Transport("Empty REALPATH response".into()));
                }
                let resolved = r.string()?;
                Ok(normalize_posix_path(&resolved))
            }
            SSH_FXP_STATUS => Err(Self::parse_status(&payload)?),
            other => Err(SftpError::Transport(format!("Expected SSH_FXP_NAME, got {other}"))),
        }
    }

    fn stat_kind(&mut self, path: &str, follow_links: bool) -> Result<FileAttrs, SftpError> {
        let clean = validate_remote_path(path)?;
        let id = self.alloc_id();
        let ptype = if follow_links { SSH_FXP_STAT } else { SSH_FXP_LSTAT };
        let pkt = PacketBuilder::new(ptype).u32(id).str(&clean).finish();
        let (rtype, payload) = self.request(&pkt, id)?;
        match rtype {
            SSH_FXP_ATTRS => {
                let mut r = Reader::new(&payload[1..]);
                let _id = r.u32()?;
                r.attrs()
            }
            SSH_FXP_STATUS => Err(Self::parse_status(&payload)?),
            other => Err(SftpError::Transport(format!("Expected SSH_FXP_ATTRS, got {other}"))),
        }
    }

    pub fn list_dir(&mut self, remote_path: &str) -> Result<Vec<SftpEntry>, SftpError> {
        let dir = validate_remote_path(remote_path)?;
        let open_id = self.alloc_id();
        let open_pkt = PacketBuilder::new(SSH_FXP_OPENDIR).u32(open_id).str(&dir).finish();
        let handle = self.expect_handle(&open_pkt, open_id)?;

        let mut raw_entries: Vec<(String, String, FileAttrs)> = Vec::new();
        let read_res = (|| -> Result<(), SftpError> {
            loop {
                let id = self.alloc_id();
                let pkt = PacketBuilder::new(SSH_FXP_READDIR).u32(id).bytes(&handle).finish();
                let (ptype, payload) = self.request(&pkt, id)?;
                match ptype {
                    SSH_FXP_NAME => {
                        let mut r = Reader::new(&payload[1..]);
                        let _id = r.u32()?;
                        let count = r.u32()?;
                        if count == 0 {
                            break;
                        }
                        for _ in 0..count {
                            let name = r.string()?;
                            let _longname = r.string()?;
                            let attrs = r.attrs()?;
                            if name == "." || name == ".." {
                                continue;
                            }
                            if let Ok(full_path) = join_posix_path(&dir, &name) {
                                raw_entries.push((name, full_path, attrs));
                            }
                        }
                    }
                    SSH_FXP_STATUS => match Self::parse_status(&payload)? {
                        SftpError::Status { code: SSH_FX_EOF, .. } => break,
                        err => return Err(err),
                    },
                    other => return Err(SftpError::Transport(format!("Expected SSH_FXP_NAME, got {other}"))),
                }
            }
            Ok(())
        })();
        self.close_handle(&handle);
        read_res?;

        let mut entries = Vec::with_capacity(raw_entries.len());
        for (name, path, attrs) in raw_entries {
            let is_symlink = attrs.is_symlink();
            let is_directory = if is_symlink {
                self.stat_kind(&path, true).map(|target| target.is_dir()).unwrap_or(false)
            } else {
                attrs.is_dir()
            };
            let permissions = attrs.permissions_string(is_directory, is_symlink);
            entries.push(SftpEntry {
                name,
                path,
                is_directory,
                is_symlink,
                size: attrs.size.unwrap_or(0),
                modified_at: attrs.mtime.map(|s| u64::from(s) * 1000),
                permissions,
            });
        }
        entries.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    pub fn download_to_writer<Out: Write>(&mut self, remote_path: &str, out: &mut Out) -> Result<u64, SftpError> {
        let path = validate_remote_path(remote_path)?;
        let open_id = self.alloc_id();
        let open_pkt = PacketBuilder::new(SSH_FXP_OPEN)
            .u32(open_id)
            .str(&path)
            .u32(SSH_FXF_READ)
            .empty_attrs()
            .finish();
        let handle = self.expect_handle(&open_pkt, open_id)?;

        let mut offset = 0u64;
        let res = (|| -> Result<u64, SftpError> {
            loop {
                let id = self.alloc_id();
                let pkt = PacketBuilder::new(SSH_FXP_READ)
                    .u32(id)
                    .bytes(&handle)
                    .u64(offset)
                    .u32(CHUNK_BYTES as u32)
                    .finish();
                let (ptype, payload) = self.request(&pkt, id)?;
                match ptype {
                    SSH_FXP_DATA => {
                        let mut r = Reader::new(&payload[1..]);
                        let _id = r.u32()?;
                        let data = r.bytes()?;
                        if data.is_empty() {
                            break;
                        }
                        out.write_all(data)
                            .map_err(|e| SftpError::LocalIo(format!("Cannot write downloaded file: {e}")))?;
                        offset += data.len() as u64;
                    }
                    SSH_FXP_STATUS => match Self::parse_status(&payload)? {
                        SftpError::Status { code: SSH_FX_EOF, .. } => break,
                        err => return Err(err),
                    },
                    other => return Err(SftpError::Transport(format!("Expected SSH_FXP_DATA, got {other}"))),
                }
            }
            out.flush()
                .map_err(|e| SftpError::LocalIo(format!("Cannot flush downloaded file: {e}")))?;
            Ok(offset)
        })();
        self.close_handle(&handle);
        res
    }

    pub fn upload_from_reader<In: Read>(&mut self, mut reader: In, remote_path: &str, mode: Option<u32>) -> Result<u64, SftpError> {
        let path = validate_remote_path(remote_path)?;
        let open_id = self.alloc_id();
        let pkt = PacketBuilder::new(SSH_FXP_OPEN)
            .u32(open_id)
            .str(&path)
            .u32(SSH_FXF_WRITE | SSH_FXF_CREAT | SSH_FXF_TRUNC)
            .mode_attrs(mode.unwrap_or(0o644) & 0o7777)
            .finish();
        let handle = self.expect_handle(&pkt, open_id)?;

        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut offset = 0u64;
        let res = (|| -> Result<u64, SftpError> {
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| SftpError::LocalIo(format!("Cannot read local file: {e}")))?;
                if n == 0 {
                    break;
                }
                let id = self.alloc_id();
                let write_pkt = PacketBuilder::new(SSH_FXP_WRITE)
                    .u32(id)
                    .bytes(&handle)
                    .u64(offset)
                    .bytes(&buf[..n])
                    .finish();
                self.expect_ok(&write_pkt, id)?;
                offset += n as u64;
            }
            Ok(offset)
        })();
        self.close_handle(&handle);
        res
    }

    pub fn create_directory(&mut self, remote_path: &str) -> Result<(), SftpError> {
        let path = validate_remote_path(remote_path)?;
        let id = self.alloc_id();
        let pkt = PacketBuilder::new(SSH_FXP_MKDIR).u32(id).str(&path).mode_attrs(0o755).finish();
        self.expect_ok(&pkt, id)
    }

    pub fn rename_entry(&mut self, old_path: &str, new_path: &str) -> Result<(), SftpError> {
        let old_clean = validate_remote_path(old_path)?;
        let new_clean = validate_remote_path(new_path)?;
        let id = self.alloc_id();
        let pkt = PacketBuilder::new(SSH_FXP_RENAME).u32(id).str(&old_clean).str(&new_clean).finish();
        self.expect_ok(&pkt, id)
    }

    pub fn delete_entry(&mut self, remote_path: &str, is_directory: bool) -> Result<(), SftpError> {
        let path = validate_remote_path(remote_path)?;
        if path == "/" {
            return Err(SftpError::InvalidPath("Refusing to delete root directory '/'".into()));
        }
        // Check lstat first so symlinks pointing to directories are unlinked, not traversed.
        let is_symlink = self.stat_kind(&path, false).map(|a| a.is_symlink()).unwrap_or(false);
        if !is_directory || is_symlink {
            let id = self.alloc_id();
            let pkt = PacketBuilder::new(SSH_FXP_REMOVE).u32(id).str(&path).finish();
            return self.expect_ok(&pkt, id);
        }

        let rmdir_id = self.alloc_id();
        let rmdir_pkt = PacketBuilder::new(SSH_FXP_RMDIR).u32(rmdir_id).str(&path).finish();
        match self.expect_ok(&rmdir_pkt, rmdir_id) {
            Ok(()) => Ok(()),
            Err(SftpError::Status { code: SSH_FX_FAILURE, .. }) => {
                let children = self.list_dir(&path)?;
                for child in children {
                    self.delete_entry(&child.path, !child.is_symlink && child.is_directory)?;
                }
                let retry_id = self.alloc_id();
                let retry_pkt = PacketBuilder::new(SSH_FXP_RMDIR).u32(retry_id).str(&path).finish();
                self.expect_ok(&retry_pkt, retry_id)
            }
            Err(err) => Err(err),
        }
    }
}

struct SftpSession {
    child: Child,
    stderr: Arc<Mutex<String>>,
    client: SftpClient<BufReader<ChildStdout>, BufWriter<ChildStdin>>,
}

impl SftpSession {
    fn connect(machine: &MachineTarget) -> Result<Self, SftpError> {
        let (mut child, stderr) = spawn_sftp_process(machine)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SftpError::Transport("Missing stdin pipe for SFTP process".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SftpError::Transport("Missing stdout pipe for SFTP process".into()))?;
        let client = match SftpClient::connect(BufReader::new(stdout), BufWriter::new(stdin)) {
            Ok(client) => client,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let ssh_err = stderr.lock().unwrap_or_else(std::sync::PoisonError::into_inner).trim().to_owned();
                let detail = if ssh_err.is_empty() {
                    err.to_string()
                } else {
                    ssh_err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or(&ssh_err).trim().to_owned()
                };
                return Err(SftpError::Transport(detail));
            }
        };
        Ok(Self { child, stderr, client })
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn enrich_transport_error(&self, err: SftpError) -> SftpError {
        if let SftpError::Transport(msg) = &err {
            let ssh_err = self.stderr.lock().unwrap_or_else(std::sync::PoisonError::into_inner).trim().to_owned();
            if !ssh_err.is_empty() && !msg.contains(&ssh_err) {
                let last = ssh_err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or(&ssh_err).trim();
                return SftpError::Transport(format!("{msg}: {last}"));
            }
        }
        err
    }
}

#[derive(Default)]
pub struct SftpService {
    sessions: Mutex<HashMap<String, Arc<Mutex<Option<SftpSession>>>>>,
}

pub fn service() -> &'static SftpService {
    static SERVICE: OnceLock<SftpService> = OnceLock::new();
    SERVICE.get_or_init(SftpService::default)
}

impl SftpService {
    fn slot_for(&self, machine_id: &str) -> Arc<Mutex<Option<SftpSession>>> {
        let mut map = self.sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(map.entry(machine_id.to_owned()).or_insert_with(|| Arc::new(Mutex::new(None))))
    }

    pub fn disconnect_machine(&self, machine_id: &str) {
        let slot = {
            let mut map = self.sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            map.remove(machine_id)
        };
        if let Some(slot) = slot
            && let Some(session) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
        {
            session.close();
        }
    }

    pub fn disconnect_all(&self) {
        let slots: Vec<Arc<Mutex<Option<SftpSession>>>> = {
            let mut map = self.sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            map.drain().map(|(_, s)| s).collect()
        };
        for slot in slots {
            if let Some(session) = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                session.close();
            }
        }
    }

    fn with_session<T>(
        &self,
        machine_id: &str,
        mut op: impl FnMut(&mut SftpClient<BufReader<ChildStdout>, BufWriter<ChildStdin>>) -> Result<T, SftpError>,
    ) -> Result<T, String> {
        let slot = self.slot_for(machine_id);
        let mut guard = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let machine = resolve_machine(machine_id)?;

        for attempt in 0..2 {
            if let Some(session) = guard.as_mut()
                && !session.is_alive()
            {
                if let Some(dead) = guard.take() {
                    dead.close();
                }
            }
            if guard.is_none() {
                *guard = Some(SftpSession::connect(&machine).map_err(|e| e.to_string())?);
            }
            let session = guard.as_mut().unwrap();
            match op(&mut session.client) {
                Ok(value) => return Ok(value),
                Err(err @ SftpError::Transport(_)) if attempt == 0 => {
                    let _ = session.enrich_transport_error(err);
                    if let Some(stale) = guard.take() {
                        stale.close();
                    }
                    continue;
                }
                Err(err) => {
                    let enriched = session.enrich_transport_error(err);
                    if matches!(enriched, SftpError::Transport(_))
                        && let Some(stale) = guard.take()
                    {
                        stale.close();
                    }
                    return Err(enriched.to_string());
                }
            }
        }
        Err(format!("Failed to establish SFTP session for {machine_id}"))
    }

    pub fn resolve_initial_path(&self, machine_id: &str, workspace_path: Option<&str>) -> Result<String, String> {
        if let Some(configured) = workspace_path.map(str::trim).filter(|p| !p.is_empty()) {
            let norm = normalize_posix_path(configured);
            return self.with_session(machine_id, |c| c.realpath(&norm));
        }
        self.with_session(machine_id, |c| c.realpath("."))
    }

    pub fn list_dir(&self, machine_id: &str, remote_path: &str) -> Result<Vec<SftpEntry>, String> {
        self.with_session(machine_id, |c| c.list_dir(remote_path))
    }

    pub fn download_file(&self, machine_id: &str, remote_path: &str, local_path: &Path) -> Result<u64, String> {
        let mut file = BufWriter::new(
            File::create(local_path).map_err(|e| format!("Cannot create {}: {e}", local_path.display()))?,
        );
        let result = self.with_session(machine_id, |c| c.download_to_writer(remote_path, &mut file));
        if result.is_err() {
            drop(file);
            let _ = std::fs::remove_file(local_path);
        }
        result
    }

    pub fn upload_files(
        &self,
        machine_id: &str,
        local_file_paths: &[PathBuf],
        remote_target_dir: &str,
    ) -> Result<Vec<UploadedFile>, String> {
        let target_dir = validate_remote_path(remote_target_dir).map_err(|e| e.to_string())?;
        self.with_session(machine_id, |client| {
            let mut uploaded = Vec::new();
            for local_path in local_file_paths {
                upload_path_recursive(client, local_path, &target_dir, &mut uploaded)?;
            }
            Ok(uploaded)
        })
    }

    pub fn create_directory(&self, machine_id: &str, remote_path: &str) -> Result<(), String> {
        self.with_session(machine_id, |c| c.create_directory(remote_path))
    }

    pub fn rename_entry(&self, machine_id: &str, old_path: &str, new_path: &str) -> Result<(), String> {
        self.with_session(machine_id, |c| c.rename_entry(old_path, new_path))
    }

    pub fn delete_entry(&self, machine_id: &str, remote_path: &str, is_directory: bool) -> Result<(), String> {
        self.with_session(machine_id, |c| c.delete_entry(remote_path, is_directory))
    }
}

fn upload_path_recursive<R: Read, W: Write>(
    client: &mut SftpClient<R, W>,
    local_path: &Path,
    remote_dir: &str,
    uploaded: &mut Vec<UploadedFile>,
) -> Result<(), SftpError> {
    let meta = std::fs::metadata(local_path)
        .map_err(|e| SftpError::LocalIo(format!("Cannot access {}: {e}", local_path.display())))?;
    let name = local_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SftpError::InvalidPath(format!("Invalid local file name: {}", local_path.display())))?;
    let remote_path = join_posix_path(remote_dir, name)?;
    if meta.is_dir() {
        match client.create_directory(&remote_path) {
            Ok(()) | Err(SftpError::Status { code: SSH_FX_FAILURE, .. }) => {}
            Err(err) => return Err(err),
        }
        let entries = std::fs::read_dir(local_path)
            .map_err(|e| SftpError::LocalIo(format!("Cannot read directory {}: {e}", local_path.display())))?;
        for entry in entries.flatten() {
            upload_path_recursive(client, &entry.path(), &remote_path, uploaded)?;
        }
        return Ok(());
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = None;

    let file = File::open(local_path)
        .map_err(|e| SftpError::LocalIo(format!("Cannot open {}: {e}", local_path.display())))?;
    let bytes = client.upload_from_reader(BufReader::new(file), &remote_path, mode)?;
    uploaded.push(UploadedFile {
        local_path: local_path.to_string_lossy().into_owned(),
        remote_path,
        bytes,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(ptype: u8, body: &[u8]) -> Vec<u8> {
        let len = (1 + body.len()) as u32;
        let mut out = Vec::with_capacity(4 + 1 + body.len());
        out.extend_from_slice(&len.to_be_bytes());
        out.push(ptype);
        out.extend_from_slice(body);
        out
    }

    fn put_str(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn normalizes_windows_and_posix_paths_strictly() {
        assert_eq!(normalize_posix_path("\\var\\log\\nginx\\"), "/var/log/nginx");
        assert_eq!(normalize_posix_path("///home/user/../admin/./app//"), "/home/admin/app");
        assert_eq!(normalize_posix_path("\\"), "/");
        assert_eq!(normalize_posix_path(""), "/");
        assert_eq!(join_posix_path("/home/user", "test.txt").unwrap(), "/home/user/test.txt");
        assert_eq!(join_posix_path("\\", "etc").unwrap(), "/etc");
        assert!(join_posix_path("/home/user", "../etc").is_err());
        assert!(join_posix_path("/home/user", "a\\b").is_err());
        assert!(join_posix_path("/home/user", "a/b").is_err());
    }

    #[test]
    fn parses_machine_target_from_hc_json() {
        let json = r#"{"machines":[{"name":"vps","target":"codex@10.0.0.5:2222","via":false},{"name":"tunnel","target":"fly ssh console -a demo","via":true}]}"#;
        let vps = parse_machine_target(json, "vps").unwrap();
        assert_eq!(vps.target, "ssh://codex@10.0.0.5:2222");
        assert!(!vps.via);

        let tunnel = parse_machine_target(json, "tunnel").unwrap();
        assert_eq!(tunnel.target, "fly ssh console -a demo");
        assert!(tunnel.via);

        assert!(parse_machine_target(json, "missing").is_err());
    }

    #[test]
    fn sftp_client_parses_realpath_readdir_and_download() {
        let mut server_rx = Vec::new();
        // 1. VERSION 3
        server_rx.extend_from_slice(&frame(SSH_FXP_VERSION, &3u32.to_be_bytes()));
        // 2. REALPATH -> /home/codex
        let mut rp = Vec::new();
        rp.extend_from_slice(&1u32.to_be_bytes());
        rp.extend_from_slice(&1u32.to_be_bytes());
        put_str(&mut rp, "/home/codex");
        put_str(&mut rp, "/home/codex");
        rp.extend_from_slice(&0u32.to_be_bytes());
        server_rx.extend_from_slice(&frame(SSH_FXP_NAME, &rp));
        // 3. OPENDIR -> handle "d1"
        let mut h1 = Vec::new();
        h1.extend_from_slice(&2u32.to_be_bytes());
        put_str(&mut h1, "d1");
        server_rx.extend_from_slice(&frame(SSH_FXP_HANDLE, &h1));
        // 4. READDIR -> ".", "src" dir, "notes.txt" file
        let mut rd = Vec::new();
        rd.extend_from_slice(&3u32.to_be_bytes());
        rd.extend_from_slice(&3u32.to_be_bytes());
        put_str(&mut rd, ".");
        put_str(&mut rd, ".");
        rd.extend_from_slice(&0u32.to_be_bytes());
        put_str(&mut rd, "notes.txt");
        put_str(&mut rd, "-rw-r--r-- ...");
        rd.extend_from_slice(&(SSH_FILEXFER_ATTR_SIZE | SSH_FILEXFER_ATTR_PERMISSIONS | SSH_FILEXFER_ATTR_ACMODTIME).to_be_bytes());
        rd.extend_from_slice(&42u64.to_be_bytes());
        rd.extend_from_slice(&0o100644u32.to_be_bytes());
        rd.extend_from_slice(&100u32.to_be_bytes());
        rd.extend_from_slice(&1700000000u32.to_be_bytes());
        put_str(&mut rd, "src");
        put_str(&mut rd, "drwxr-xr-x ...");
        rd.extend_from_slice(&(SSH_FILEXFER_ATTR_SIZE | SSH_FILEXFER_ATTR_PERMISSIONS | SSH_FILEXFER_ATTR_ACMODTIME).to_be_bytes());
        rd.extend_from_slice(&4096u64.to_be_bytes());
        rd.extend_from_slice(&0o040755u32.to_be_bytes());
        rd.extend_from_slice(&100u32.to_be_bytes());
        rd.extend_from_slice(&1700000010u32.to_be_bytes());
        server_rx.extend_from_slice(&frame(SSH_FXP_NAME, &rd));
        // 5. READDIR -> EOF
        let mut eof = Vec::new();
        eof.extend_from_slice(&4u32.to_be_bytes());
        eof.extend_from_slice(&SSH_FX_EOF.to_be_bytes());
        put_str(&mut eof, "EOF");
        put_str(&mut eof, "");
        server_rx.extend_from_slice(&frame(SSH_FXP_STATUS, &eof));
        // 6. CLOSE dir -> OK
        let mut ok5 = Vec::new();
        ok5.extend_from_slice(&5u32.to_be_bytes());
        ok5.extend_from_slice(&SSH_FX_OK.to_be_bytes());
        put_str(&mut ok5, "");
        put_str(&mut ok5, "");
        server_rx.extend_from_slice(&frame(SSH_FXP_STATUS, &ok5));

        let mut client_tx = Vec::new();
        let mut client = SftpClient::connect(Cursor::new(server_rx), &mut client_tx).unwrap();
        assert_eq!(client.realpath(".").unwrap(), "/home/codex");
        let entries = client.list_dir("/home/codex").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "src");
        assert!(entries[0].is_directory);
        assert_eq!(entries[0].permissions, "drwxr-xr-x");
        assert_eq!(entries[1].name, "notes.txt");
        assert!(!entries[1].is_directory);
        assert_eq!(entries[1].size, 42);
        assert_eq!(entries[1].modified_at, Some(1700000000 * 1000));
    }

    #[test]
    fn maps_permission_denied_to_eacces() {
        let mut server_rx = Vec::new();
        server_rx.extend_from_slice(&frame(SSH_FXP_VERSION, &3u32.to_be_bytes()));
        let mut denied = Vec::new();
        denied.extend_from_slice(&1u32.to_be_bytes());
        denied.extend_from_slice(&SSH_FX_PERMISSION_DENIED.to_be_bytes());
        put_str(&mut denied, "Permission denied");
        put_str(&mut denied, "");
        server_rx.extend_from_slice(&frame(SSH_FXP_STATUS, &denied));

        let mut client_tx = Vec::new();
        let mut client = SftpClient::connect(Cursor::new(server_rx), &mut client_tx).unwrap();
        let err = client.create_directory("/root/secret").unwrap_err().to_string();
        assert!(err.contains("EACCES"), "{err}");
    }
}

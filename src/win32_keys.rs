//! Windows' ConPTY asks its viewer for win32-input-mode (`CSI ?9001h`): key
//! events with their modifiers, which it otherwise loses (Shift+Enter as
//! Ghostty sends it never reaches the program). herdr-core replays the request
//! to every new viewer. Used for Escape and modified Enter without Kitty;
//! negotiated Kitty input stays with Ghostty's encoder.

const ON: &[u8] = b"\x1b[?9001h";
const OFF: &[u8] = b"\x1b[?9001l";

/// Whether the program asked, read from its output as it streams by.
#[derive(Default)]
pub struct Win32Input {
    pub on: bool,
    /// The end of the last write, for a request split across two.
    tail: Vec<u8>,
}

impl Win32Input {
    pub fn watch(&mut self, bytes: &[u8]) {
        const KEEP: usize = ON.len() - 1;
        // The seam: the last write's end and this one's start.
        let mut edge = std::mem::take(&mut self.tail);
        edge.extend_from_slice(&bytes[..bytes.len().min(KEEP)]);
        if let Some(on) = latest(bytes).or_else(|| latest(&edge)) {
            self.on = on;
        }
        let end = if bytes.len() >= KEEP { bytes } else { &edge };
        self.tail = end[end.len().saturating_sub(KEEP)..].to_vec();
    }
}

/// The last request in `bytes`: on or off.
fn latest(bytes: &[u8]) -> Option<bool> {
    let at = |seq: &[u8]| bytes.windows(seq.len()).rposition(|w| w == seq);
    match (at(ON), at(OFF)) {
        (Some(on), off) => Some(off.is_none_or(|off| on > off)),
        (None, off) => off.map(|_| false),
    }
}

/// Enter pressed and released, as win32-input-mode key events:
/// `CSI Vk;Sc;Uc;Kd;Cs;Rc _` (VK_RETURN, its scan code, the character
/// Windows types, down or up, Shift/left Ctrl/left Alt held, once).
pub fn enter(shift: bool, ctrl: bool, alt: bool) -> Vec<u8> {
    let typed = if ctrl { 10 } else { 13 };
    key(13, 28, typed, shift, ctrl, alt)
}

pub fn escape(shift: bool, ctrl: bool, alt: bool) -> Vec<u8> {
    key(27, 1, 27, shift, ctrl, alt)
}

fn key(vk: u32, scan: u32, typed: u32, shift: bool, ctrl: bool, alt: bool) -> Vec<u8> {
    let held = (shift as u32) * 0x10 | (ctrl as u32) * 0x08 | (alt as u32) * 0x02;
    [1, 0].iter().flat_map(|down| format!("\x1b[{vk};{scan};{typed};{down};{held};1_").into_bytes()).collect()
}

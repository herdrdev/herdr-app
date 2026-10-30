//! COMPOUND_TEXT encoding and decoding.

#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

#[cfg(feature = "std")]
use std::io::{self, Write};

const UTF8_START: &[u8] = &[0x1B, 0x25, 0x47];
const UTF8_END: &[u8] = &[0x1B, 0x25, 0x40];

/// Wrapper for reduce allocation
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct CText<'s> {
    utf8: &'s str,
}

impl<'s> fmt::Debug for CText<'s> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.utf8)
    }
}

impl<'s> fmt::Display for CText<'s> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.utf8)
    }
}

impl<'s> CText<'s> {
    pub const fn new(utf8: &'s str) -> Self {
        Self { utf8 }
    }

    pub const fn len(self) -> usize {
        self.utf8.len() + UTF8_START.len() + UTF8_END.len()
    }

    #[cfg(feature = "std")]
    pub fn write(self, mut out: impl Write) -> io::Result<usize> {
        let mut writed = 0;
        writed += out.write(UTF8_START)?;
        writed += out.write(self.utf8.as_bytes())?;
        writed += out.write(UTF8_END)?;
        Ok(writed)
    }
}

/// Encoding utf8 to COMPOUND_TEXT with utf8 escape
pub fn utf8_to_compound_text(text: &str) -> Vec<u8> {
    let mut ret = Vec::with_capacity(text.len() + 6);
    ret.extend_from_slice(UTF8_START);
    ret.extend_from_slice(text.as_bytes());
    ret.extend_from_slice(UTF8_END);
    ret
}

#[derive(Debug, Clone)]
pub enum DecodeError {
    InvalidEncoding,
    UnsupportedEncoding,
    Utf8Error(alloc::string::FromUtf8Error),
}

impl From<alloc::string::FromUtf8Error> for DecodeError {
    fn from(err: alloc::string::FromUtf8Error) -> Self {
        DecodeError::Utf8Error(err)
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEncoding => write!(f, "Invalid compound text"),
            Self::UnsupportedEncoding => write!(f, "This encoding is not supported yet"),
            Self::Utf8Error(e) => write!(f, "Not a valid utf8 {}", e),
        }
    }
}

// HERDR: COMPOUND_TEXT starts in ASCII + ISO-8859-1, not UTF-8.
// A designation changes the high-byte charset until the next designation.
pub fn compound_text_to_utf8(mut bytes: &[u8]) -> Result<String, DecodeError> {
    let mut out = String::new();
    let mut charset: Option<&'static encoding_rs::Encoding> = None;
    let mut gl: Option<&'static encoding_rs::Encoding> = None;
    while !bytes.is_empty() {
        if bytes.starts_with(UTF8_START) {
            bytes = &bytes[UTF8_START.len()..];
            let end = bytes.windows(UTF8_END.len()).position(|w| w == UTF8_END);
            let len = end.unwrap_or(bytes.len());
            out.push_str(&String::from_utf8(bytes[..len].to_vec())?);
            bytes = if end.is_some() {
                &bytes[len + UTF8_END.len()..]
            } else {
                &[]
            };
        } else if bytes.starts_with(b"\x1b(B") {
            gl = None;
            bytes = &bytes[3..];
        } else if bytes.starts_with(b"\x1b-") {
            let designation = *bytes.get(2).ok_or(DecodeError::InvalidEncoding)?;
            charset = match designation {
                b'A' => None,
                b'B' => Some(encoding_rs::ISO_8859_2),
                b'C' => Some(encoding_rs::ISO_8859_3),
                b'D' => Some(encoding_rs::ISO_8859_4),
                b'L' => Some(encoding_rs::ISO_8859_5),
                b'G' => Some(encoding_rs::ISO_8859_6),
                b'F' => Some(encoding_rs::ISO_8859_7),
                b'H' => Some(encoding_rs::ISO_8859_8),
                b'M' => Some(encoding_rs::WINDOWS_1254), // ISO-8859-9 in GR
                b'V' => Some(encoding_rs::ISO_8859_10),
                b'T' => Some(encoding_rs::WINDOWS_874), // ISO-8859-11 in GR
                b'Y' => Some(encoding_rs::ISO_8859_13),
                b'_' => Some(encoding_rs::ISO_8859_14),
                b'b' => Some(encoding_rs::ISO_8859_15),
                b'f' => Some(encoding_rs::ISO_8859_16),
                _ => return Err(DecodeError::UnsupportedEncoding),
            };
            bytes = &bytes[3..];
        } else if bytes.starts_with(b"\x1b$") {
            let (index, right) = match bytes.get(2) {
                Some(b'(') => (3, false),
                Some(b')') => (3, true),
                _ => (2, false),
            };
            let encoding = match bytes.get(index).ok_or(DecodeError::InvalidEncoding)? {
                b'A' => encoding_rs::GBK,
                b'B' => encoding_rs::EUC_JP,
                b'C' => encoding_rs::EUC_KR,
                _ => return Err(DecodeError::UnsupportedEncoding),
            };
            if right {
                charset = Some(encoding);
            } else {
                gl = Some(encoding);
            }
            bytes = &bytes[index + 1..];
        } else if bytes[0] == 0x1b {
            return Err(DecodeError::UnsupportedEncoding);
        } else if bytes[0] < 0x20 {
            out.push(char::from(bytes[0]));
            bytes = &bytes[1..];
        } else {
            let right = bytes[0] >= 0x80;
            let len = bytes.iter().position(|b| *b < 0x20 || (*b >= 0x80) != right).unwrap_or(bytes.len());
            let run = &bytes[..len];
            if let Some(encoding) = if right { charset } else { gl } {
                // EUC encodings use the high bit for COMPOUND_TEXT's GL pairs.
                let high;
                let encoded = if right { run } else {
                    high = run.iter().map(|b| b | 0x80).collect::<Vec<_>>();
                    &high
                };
                let (decoded, errors) = encoding.decode_without_bom_handling(encoded);
                if errors {
                    return Err(DecodeError::InvalidEncoding);
                }
                out.push_str(&decoded);
            } else {
                out.extend(run.iter().map(|byte| char::from(*byte)));
            }
            bytes = &bytes[len..];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn latin1_is_the_default() {
        assert_eq!(crate::compound_text_to_utf8(b"\xb4").unwrap(), "´");
        assert_eq!(crate::compound_text_to_utf8(b"caf\xe9").unwrap(), "café");
        assert_eq!(
            crate::compound_text_to_utf8(b"plain ASCII").unwrap(),
            "plain ASCII"
        );
    }

    #[test]
    fn charset_designations_and_utf8_segments() {
        assert_eq!(
            crate::compound_text_to_utf8(b"\x1b-B\xa1\x1b-A\xe1").unwrap(),
            "Ąá"
        );
        assert_eq!(
            crate::compound_text_to_utf8(b"\xe1\x1b%G\xe2\x82\xac\x1b%@\xe9").unwrap(),
            "á€é"
        );
        assert_eq!(
            crate::compound_text_to_utf8(b"\x1b%G\xc3\xa1").unwrap(),
            "á"
        );
        assert_eq!(crate::compound_text_to_utf8(b"\x1b%G\x1b%@").unwrap(), "");
    }

    #[test]
    fn cjk_designations_can_switch_mid_commit() {
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$(ADc\x1b$(B9%\x1b(B!").unwrap(), "你好!");
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$(ADc:C\x1b(B!").unwrap(), "你好!");
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$)A\xc4\xe3\xba\xc3!").unwrap(), "你好!");
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$(C0!\x1b(B!").unwrap(), "가!");
    }

    #[test]
    fn controls_do_not_change_the_graphic_charset() {
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$(ADc\n:C").unwrap(), "你\n好");
        assert_eq!(crate::compound_text_to_utf8(b"\x1b$(BEl\t5~").unwrap(), "東\t京");
    }

    #[test]
    fn malformed_and_truncated_input_never_panics() {
        for bytes in [
            b"\x1b%Ghello\x1b%@".as_slice(),
            b"\x1b-B\xa1",
            b"\x1b$(BEl5~",
        ] {
            for len in 0..=bytes.len() {
                let _ = crate::compound_text_to_utf8(&bytes[..len]);
            }
        }
        assert!(crate::compound_text_to_utf8(b"\x1b%G\xff\x1b%@").is_err());
        assert!(crate::compound_text_to_utf8(b"\x1b-Z").is_err());
    }

    #[test]
    fn korean() {
        const UTF8: &str = "가나다";
        const COMP: &[u8] = &[
            27, 37, 71, 234, 176, 128, 235, 130, 152, 235, 139, 164, 27, 37, 64,
        ];
        assert_eq!(crate::utf8_to_compound_text(UTF8), COMP);
        assert_eq!(crate::compound_text_to_utf8(COMP).unwrap(), UTF8);
    }

    #[test]
    fn iso_2011_jp() {
        const UTF8: &str = "東京";
        const COMP: &[u8] = &[27, 36, 40, 66, 69, 108, 53, 126];
        assert_eq!(crate::compound_text_to_utf8(COMP).unwrap(), UTF8);
    }
}

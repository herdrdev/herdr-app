#![cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};

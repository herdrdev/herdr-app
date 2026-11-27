#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::upper_case_acronyms)]
#![allow(clippy::uninlined_format_args)]
#![no_std]

use ::core::ptr;

/// Common error message for converting between C `size_t` and Rust `usize`;
pub const SIZE_T_ERROR: &str =
    "conversion between C type 'size_t' and Rust type 'usize' overflowed.";

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// Build-time metadata generated from QuickJS's `quickjs-opcode.h` macros.
#[cfg(feature = "jit-abi")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitGeneratedOpcode {
    pub opcode: u8,
    pub size: u8,
    pub n_pop: u8,
    pub n_push: u8,
    pub format: u8,
    pub format_name: &'static str,
    pub name: &'static str,
}

#[cfg(feature = "jit-abi")]
include!(concat!(env!("OUT_DIR"), "/quickjs-jit-opcodes.rs"));

/// Build-time metadata generated from the canonical helper X-macro table.
#[cfg(feature = "jit-abi")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitGeneratedHelper {
    pub id: u16,
    pub name: &'static str,
    pub abi_types: &'static [u8],
    pub value_arity: u8,
    pub value_ownership: &'static [u8],
    pub output_ownership: u8,
    pub flags: u32,
}

#[cfg(feature = "jit-abi")]
include!(concat!(env!("OUT_DIR"), "/quickjs-jit-helpers.rs"));

#[doc(hidden)]
#[cfg(all(feature = "jit-test-support", feature = "bindgen"))]
pub const JIT_BINDGEN_BINDINGS: Option<&str> =
    Some(include_str!(concat!(env!("OUT_DIR"), "/bindings.rs")));

#[doc(hidden)]
#[cfg(all(feature = "jit-test-support", not(feature = "bindgen")))]
pub const JIT_BINDGEN_BINDINGS: Option<&str> = None;

#[cfg(not(feature = "bindgen"))]
include!(concat!("bindings/", bindings_env!("TARGET"), ".rs"));

#[cfg(target_pointer_width = "64")]
include!("inlines/ptr_64.rs");

#[cfg(target_pointer_width = "32")]
include!("inlines/ptr_32_nan_boxing.rs");

include!("inlines/common.rs");

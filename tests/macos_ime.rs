#![cfg(target_os = "macos")]

// Keep the vendored platform's routing regression in the app's cargo test suite.
#[path = "../vendor/gpui-pre-macos/src/key_routing.rs"]
mod key_routing;

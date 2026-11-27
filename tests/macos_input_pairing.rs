#![cfg(target_os = "macos")]

// The vendored platform's tests are not part of this workspace: run the input
// method pairing rule's tests with the app's.
#[path = "../vendor/gpui-pre-macos/src/input_pairing.rs"]
mod input_pairing;

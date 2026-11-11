# macOS keyboard suggestions repair

Version: v0.1.16-local.1 (contribution checkpoint, not an upstream release)

Based on stable v0.1.15. Restores native displayed-row text/cursor context for Accessibility Keyboard prediction and bounded matching-prefix completion. Includes the terminal-role/native-view bridge from PR #771 (credit: BenjaminG), focus synchronization, accessibility cache invalidation and text geometry. Plain Escape cancels keyboard-layout dead-key composition without leaking terminal ESC; full IME and unknown-source routing stays unchanged.

## Verification

macOS 27.2.1, Apple Silicon, ABC input source. Owner confirmed suggestions and selected-word insertion in isolated QA and subsequently confirmed the installed optimized repair works. Cursor/backspace editing, accent commit, dead-key cancellation followed by Z, and Find focus isolation passed. Accent tests used Option-as-Alt off. Optimized and debug builds passed; Rust suite 413 passed, 3 ignored; seven unchanged scripting suites 75 passed. Cache regression fails without the correction. Two independent review passes cleared the bounded changes. All six implementation/test files in this contribution match that reviewed source byte-for-byte.

## Remaining scope

CI and integration with newer upstream master remain pending; this is a draft contribution. Generic autocorrection requiring deletion is unsupported; prefix extension only. Full CJK IMEs and ignored tests remain unverified. Context freshness is not a per-input acknowledgment. Soft-wrap text and pathological combining-cell boundaries have limitations. Custom multi-pane workspace AX inspection still exposes only window/group despite owner-confirmed keyboard predictions; broad accessibility support is not claimed. Bounded output/idle responsiveness passed; no comparative performance benchmark was run.

No unrelated project architecture or upstream release/version changes are proposed. No private terminal contents, user paths, binaries, or raw system decompilation are included.

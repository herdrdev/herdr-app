# macOS keyboard and bidirectional terminal contribution

Version: v0.1.16-local.2 (contribution checkpoint, not an upstream release)

Based on stable v0.1.15. Retains the Accessibility Keyboard repair from the
previous contribution, including PR #771's terminal-role/native-view bridge
(credit: BenjaminG), focus/cache handling and bounded matching-prefix completion.

Adds opt-in `terminal_bidi` row display mapping while preserving logical terminal
storage, input and copied text. Cursor, selection, search, links and native text
geometry use the same mapping. Combining/wide clusters stay atomic. CoreText
may emit niqqud before the base letter: the cell-positioning pass now anchors
using the earliest source index across font runs. Optional bundled Noto Serif
Hebrew and Noto Rashi Hebrew faces retain Latin/code typography and fit Hebrew
cells, including the inverse block-cursor glyph.

## Verification

macOS 27.2.1, Apple Silicon. Full Rust suite: 421 passed plus native GPU readback,
3 ignored. Ten Bun scripting suites: 86 passed with canonical temporary paths.
Mark-before-base and wide-cell pointer regressions fail without their fixes.
Actual Ghostty selection test preserves logical copied Hebrew and niqqud.
Optimized app rendering inspected with square/Rashi text, Hebrew/Latin/numbers,
punctuation, colored runs, wide emoji, niqqud, cantillation and block cursor.
Independent source review cleared all findings. Owner reported substantially
improved rendering and authorized local installation and this contribution.

## Limits and remaining integration

Draft contribution: upstream integration and CI remain pending. No public binary
release. Row-based fixed-LTR paragraph mapping is not document paragraph reflow;
applications that already reorder text should leave the option off. Full CJK IME
and three ignored tests remain unverified. Real pointer/native keyboard tests of
the new package were not separately recorded; earlier keyboard acceptance and
current automated tests do not prove every interaction. Generic autocorrection
requiring deletion remains unsupported. Custom-layout accessibility and bounded
context limitations from the original contribution remain.

Source, font licenses and tests only: no private paths, terminal records, local
QA bundles or system decompilation are included. Upstream version unchanged.

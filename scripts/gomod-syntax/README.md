# Go module syntax

The Files mod selects `go.mod` for files named `go.mod`. The shell loads a
small, independent Syntect syntax set from `gomod.packdump`; other languages
keep the default set. Runtime dependencies and Cargo.lock are unchanged.

The source is `vendor/gpui-kit/crates/shell/src/syntax/Gomod.sublime-syntax`,
adapted from mitranim/sublime-gomod at
`e0200f024a394d6e7d6b7b37a070bb8ef324b19b` (WTFPL; upstream notice in
`licenses/sublime-gomod.txt`). Local changes:

- Select only the `go.mod` token; Files handles exact filenames.
- Assign paths and versions to separate theme categories.
- Recognize quoted paths and keep token separators on the same line.
- Add version scopes for `go`, `toolchain`, and `retract`, and handle
  `godebug` blocks.

Generate the checked-in binary from the repository root:

```sh
cargo run --locked --manifest-path scripts/gomod-syntax/Cargo.toml
cargo run --locked --manifest-path scripts/gomod-syntax/Cargo.toml -- --check
cargo test --locked --manifest-path scripts/gomod-syntax/Cargo.toml
```

This standalone developer tool enables Syntect's YAML loader and dump writer;
the application does not. Its lockfile pins the generator dependencies. Keep
its Syntect version aligned with the app's lockfile when upgrading. Tests use
the same binary resource and Rust module as the application, including
`fancy-regex` parsing, without requiring GPUI or a running desktop.

After changing the grammar, regenerate the resource and run the checks above,
`bun test scripts/files.test.ts`, and the application's normal checks. The
application-level tests also verify scope-to-theme mapping.

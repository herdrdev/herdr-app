[windows]
set shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

check: test

build:
    cargo build --locked

test:
    cargo test --locked

[linux]
install:
    bash scripts/install-linux

[macos]
install:
    bash scripts/install-macos

[windows]
install:
    cargo install --path . --locked

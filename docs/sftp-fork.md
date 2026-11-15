# SFTP browser: Windows preview build

This is an experimental build of the ECidno fork. Its Files panel includes an
SFTP browser, transfers and a read-only text preview. It requires the modified
application in this branch: copying the Files mod into stock Herdr is not enough,
because the SFTP API and drop handling are implemented in the application.

## Run the Windows build

1. Install a compatible [herdr-core](https://github.com/herdrdev/herdr-core)
   and register an SSH machine in Herdr. OpenSSH must be able to connect without
   an interactive password prompt; verify the server's host key first.
2. Download and extract the Windows preview ZIP from this fork's GitHub release.
   Keep `LICENSE` and `THIRD-PARTY-NOTICES` with the application.
3. Close any open Herdr window, then run `herdr-app.exe` from the extracted
   folder. This source build does not update itself. Windows may warn that the
   executable is unsigned. The build uses your existing Herdr configuration;
   back it up if you want a way to revert before trying this preview.
4. Open a space on the remote machine, open **Files**, then select **SFTP**.
   Click folders to navigate and a text file to preview it. Use **Back** to
   return. The menu provides download, rename, copy path and delete; upload
   uses the picker or drag and drop.

The text preview is read-only, uses the remote herdr-core file reader, and is
limited to files of at most 1 MiB; long text is truncated in the display.
Binary files cannot be previewed. Transfers and destructive actions should be
tested on disposable files first. This preview has not had a full end-to-end
test across all platforms or server configurations.

In the text preview, a nonempty mouse selection is copied on release when the
Herdr setting `copy_on_select` is enabled. Release the mouse inside the preview.

## Build from source

On x64 Windows, install Git, Rust 1.95.0, Zig 0.16.0, LLVM/libclang, and Visual
Studio 2022 C++ Build Tools including a Windows SDK. Then:

```powershell
git clone --branch feat/sftp-browser https://github.com/ECidno/herdr-app.git
cd herdr-app
git submodule update --init --recursive
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
cargo +1.95.0 build --release --locked
.\target\release\herdr-app.exe
```

Ensure Zig 0.16.0 is first on `PATH` for this build. The compiled executable
is under `target\release`. The branch is a fork preview, not an official Herdr
release. See the main README and LICENSE for upstream documentation and terms.

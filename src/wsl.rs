//! WSL2 reaches the GPU only through Direct3D 12 (`/dev/dxg`). Mesa's d3d12
//! OpenGL driver gets there, but Mesa only uses it when asked by name, so
//! wgpu falls back to llvmpipe and draws on the CPU.

use std::path::Path;

/// Asks Mesa for d3d12 when WSL2 has all of it; otherwise nothing changes,
/// since a named Mesa driver that fails doesn't fall back to the others.
/// Call before any thread starts.
pub fn use_gpu() {
    if std::env::var_os("GALLIUM_DRIVER").is_none() && has_d3d12(&|path| path.exists()) {
        // SAFETY: called at startup, before other threads read the environment.
        unsafe { std::env::set_var("GALLIUM_DRIVER", "d3d12") };
    }
}

/// WSL2's GPU channel, Direct3D 12 from Windows, and Mesa's driver for it.
fn has_d3d12(exists: &dyn Fn(&Path) -> bool) -> bool {
    let multiarch = format!("/usr/lib/{}-linux-gnu/dri", std::env::consts::ARCH);
    exists(Path::new("/dev/dxg"))
        && exists(Path::new("/usr/lib/wsl/lib/libd3d12.so"))
        && [multiarch.as_str(), "/usr/lib64/dri", "/usr/lib/dri"].iter().any(|dir| exists(&Path::new(dir).join("d3d12_dri.so")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |path| files.iter().any(|f| Path::new(f) == path)
    }

    #[test]
    fn needs_every_part() {
        assert!(has_d3d12(&only(&["/dev/dxg", "/usr/lib/wsl/lib/libd3d12.so", "/usr/lib64/dri/d3d12_dri.so"])));
        // Native Linux or WSL1, no Direct3D 12, a Mesa without d3d12.
        assert!(!has_d3d12(&only(&["/usr/lib/wsl/lib/libd3d12.so", "/usr/lib64/dri/d3d12_dri.so"])));
        assert!(!has_d3d12(&only(&["/dev/dxg", "/usr/lib64/dri/d3d12_dri.so"])));
        assert!(!has_d3d12(&only(&["/dev/dxg", "/usr/lib/wsl/lib/libd3d12.so"])));
    }
}

//! The optional Windows Explorer command is installed separately from the app.

pub fn offer() {
    // Scratch homes must never register an extension in the person's Explorer.
    if std::env::var_os("HERDR_APP_HOME").is_some() { return; }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(root) = exe.parent() else { return };
    let script = root.join("windows-explorer.ps1");
    if !script.is_file() { return; }
    if let Err(error) = crate::spawn::command("powershell.exe")
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script).args(["-Action", "Offer", "-InstallRoot"]).arg(root)
        .spawn()
    {
        tracing::warn!("cannot offer Explorer integration: {error}");
    }
}

//! Extra mod checks run inside gpui-shell's existing command-line checker.
use gpui_shell::HostValue;
use std::time::Duration;

pub fn run() {
    crate::mods::set_condition_context(crate::conditions::Context {
        folder: Some(crate::mods::home().to_string_lossy().into_owned()),
        machine: None,
    });
    gpui_shell::host::main_with_components_and_check(
        gpui_shell::FrozenComponentRegistry::default(),
        |application, window, cx| {
            if !application.has_export("when", window, cx)? {
                return Ok(None);
            }
            crate::conditions::watch(application.export("watch", window, cx)?)
                .map_err(gpui_shell::anyhow::Error::msg)?;
            let context = crate::mods::condition_context();
            let call = application.call(
                "when",
                crate::mods::to_host(&serde_json::json!({
                    "folder": context.folder, "machine": context.machine, "props": null,
                })),
                window,
                cx,
            )?;
            Ok(Some(cx.spawn(async move |_| {
                let answer = call.resolve(Duration::from_secs(5)).await?;
                let HostValue::Bool(answer) = answer else {
                    gpui_shell::anyhow::bail!("when must return boolean or Promise<boolean>");
                };
                println!("when: {answer}");
                Ok(())
            })))
        },
    );
}

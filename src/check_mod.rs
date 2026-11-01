//! The mod checker also validates optional module-level conditions.
use gpui::{AppContext, WindowOptions};
use gpui_shell::{HostValue, ShellRuntime};
use std::{path::PathBuf, time::Duration};

pub fn run(args: &[String]) {
    let Some(path) = args.get(2).filter(|path| !path.starts_with('-')) else {
        eprintln!("usage: herdr-app check <mod folder> [--print-spec]");
        std::process::exit(2);
    };
    if args.iter().skip(3).any(|arg| arg != "--print-spec") {
        eprintln!("unknown check argument");
        std::process::exit(2);
    }
    let print_spec = args.iter().any(|arg| arg == "--print-spec");
    let directory = PathBuf::from(path);
    let root = match gpui_shell::resolve_app_root(&directory, "main.js") {
        Ok(root) => root,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    };
    gpui_platform::application()
        .with_assets(gpui_shell::AppAssets::new(root.clone()))
        .run(move |cx| {
            gpui_shell::init(cx);
            let result =
                (|| -> gpui_shell::anyhow::Result<_> {
                    gpui_shell::host::configure_local_check(&root, cx)?;
                    let runtime = ShellRuntime::new(cx)?;
                    let window = cx.open_window(
                        WindowOptions {
                            show: false,
                            focus: false,
                            ..Default::default()
                        },
                        |_, cx| cx.new(|_| gpui::Empty),
                    )?;
                    let (application, call) =
                    window.update(cx, |_, window, cx| -> gpui_shell::anyhow::Result<_> {
                        crate::mods::set_condition_context(crate::conditions::Context {
                            folder: Some(crate::mods::home().to_string_lossy().into_owned()),
                            machine: None,
                        });
                        let entry = if root.join("gpui-shell.json").is_file() {
                            gpui_shell::plugin::PluginManifest::read(&root)?.entry().to_owned()
                        } else { "main.js".to_owned() };
                        let application = runtime.prepare_application(&root, &entry, window, cx)?;
                        let call = if application.has_export("when", window, cx)? {
                            crate::conditions::watch(application.export("watch", window, cx)?)
                                .map_err(gpui_shell::anyhow::Error::msg)?;
                            let context = crate::mods::condition_context();
                            Some(application.call("when", crate::mods::to_host(&serde_json::json!({
                            "folder": context.folder, "machine": context.machine, "props": null,
                        })), window, cx)?)
                        } else {
                            None
                        };
                        Ok((application, call))
                    })??;
                    Ok((window, application, call))
                })();
            let (window, mut application, call) = match result {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("check failed: {error:#}");
                    std::process::exit(1);
                }
            };
            cx.spawn(async move |cx| {
                let result = async {
                    if let Some(call) = call {
                        let answer = call.resolve(Duration::from_secs(5)).await?;
                        let HostValue::Bool(answer) = answer else {
                            gpui_shell::anyhow::bail!(
                                "when must return boolean or Promise<boolean>"
                            );
                        };
                        println!("when: {answer}");
                    }
                    let description =
                        window.update(cx, |_, window, cx| application.check(window, cx))??;
                    if print_spec {
                        println!("{description}");
                    }
                    Ok::<_, gpui_shell::anyhow::Error>(())
                }
                .await;
                match result {
                    Ok(()) => {
                        println!("check passed: {}", directory.display());
                        std::process::exit(0);
                    }
                    Err(error) => {
                        eprintln!("check failed: {error:#}");
                        std::process::exit(1);
                    }
                }
            })
            .detach();
        });
}

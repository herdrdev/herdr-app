//! The frame's core screens, shown instead of the main window until a core
//! that can serve this app runs:
//! - none found (core_bin.rs): install it for me (the core's own installer),
//!   or point at one;
//! - too old (or missing a method the app calls): update it (`hc update`,
//!   terminals kept);
//! - a newer generation than this app speaks: update the app.
//! Then the main window opens.

use crate::hc::{Core, Mismatch};
use crate::mods::Request;
use crate::settings::{self, color as c};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;

const ICON: &[u8] = include_bytes!("../assets/icons/herdr-ui-icon-clean.png");
#[cfg(not(windows))]
const APP_INSTALL: &str = "curl -fsSL https://releases.herdr.dev/app/install.sh | sh";
#[cfg(windows)]
const APP_INSTALL: &str = "powershell -c \"irm https://releases.herdr.dev/app/install.ps1 | iex\"";
const LINUX_CORE_INSTALL: &str = "curl -fsSL https://releases.herdr.dev/core/install.sh | sh";
#[cfg(not(windows))]
const CORE_INSTALL: &str = LINUX_CORE_INSTALL;
#[cfg(windows)]
const CORE_INSTALL: &str = "powershell -c \"irm https://releases.herdr.dev/core/install.ps1 | iex\"";
const CORE_PLACE: &str = if cfg!(windows) { r"%LOCALAPPDATA%\Programs\herdr-core" } else { "~/.local/bin" };

/// What's missing before the main window can open.
pub enum Need {
    Install,
    Connect(String),
    Fix(Mismatch),
}

impl Need {
    pub fn connection(error: anyhow::Error) -> Self {
        match error.downcast_ref::<crate::hc::ConnectError>() {
            Some(error) => Self::Connect(error.to_string()),
            None => Self::Install,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Need;

    #[test]
    fn unreadable_core_connections_offer_installation_instead_of_retry() {
        let error = anyhow::anyhow!("bad hello reply from core: EOF");
        assert!(matches!(Need::connection(error), Need::Install));
    }

    #[test]
    fn structured_core_errors_keep_the_recovery_hint_and_retry() {
        let message = "the core pipe cannot be accessed\n\ntry again without elevation";
        let error = anyhow::Error::new(crate::hc::ConnectError(message.into())).context("connecting");
        assert!(matches!(Need::connection(error), Need::Connect(text) if text == message));
    }
}

enum State {
    Ready,
    Busy(&'static str),
    Failed(String),
}

pub struct Setup {
    need: Need,
    state: State,
    requests: Option<async_channel::Receiver<Request>>,
    icon: Arc<Image>,
}

pub fn open(need: Need, requests: async_channel::Receiver<Request>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(620.0), px(460.0)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Herdr".into()), ..Default::default() }),
        app_id: Some("herdr-app".into()),
        ..Default::default()
    };
    let icon = Arc::new(Image::from_bytes(ImageFormat::Png, ICON.to_vec()));
    cx.open_window(options, |_, cx| cx.new(|_| Setup { need, state: State::Ready, requests: Some(requests), icon }))
        .expect("cannot open the Herdr window");
    cx.activate(true);
}

impl Setup {
    /// All discovery, version probes and connection handshakes run off the UI thread.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Connecting to herdr-core…", || Ok(None), window, cx);
    }

    /// Runs `work` and reconnects off the UI thread, then applies the result once.
    fn run(&mut self, label: &'static str, work: impl FnOnce() -> Result<Option<PathBuf>, String> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state, State::Busy(_)) { return; }
        self.state = State::Busy(label);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                work().map(|bin| {
                    if let Some(bin) = bin { crate::core_bin::use_core(bin); }
                    connect()
                })
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(core)) => {
                        let Some(requests) = this.requests.take() else { return; };
                        window.remove_window();
                        crate::open_main(core, requests, cx);
                        return;
                    }
                    Ok(Err(need)) => {
                        this.need = need;
                        this.state = State::Ready;
                    }
                    Err(error) => this.state = State::Failed(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Installing herdr-core…", || crate::core_bin::install().map(Some), window, cx);
    }

    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run("Updating herdr-core… your terminals keep running.", || crate::core_bin::update().map(|_| None), window, cx);
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state, State::Busy(_)) { return; }
        self.state = State::Busy("Choosing herdr-core…");
        cx.notify();
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this herdr-core".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.state = State::Ready;
                cx.notify();
                let Ok(Ok(Some(paths))) = paths else { return; };
                let Some(bin) = paths.into_iter().next() else { return; };
                this.run("Checking herdr-core…", move || choose_core(bin).map(Some), window, cx);
            });
        })
        .detach();
    }

    fn use_windows(&mut self, cx: &mut Context<Self>) {
        let result = settings::load_person().and_then(|mut person| {
            person.wsl = None;
            settings::write_person(&person).map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            self.state = State::Failed(format!("Cannot save settings: {e}"));
            cx.notify();
            return;
        }
        if !crate::update::relaunch() {
            self.state = State::Failed("Cannot restart Herdr. Close and reopen it to continue without WSL.".into());
            cx.notify();
            return;
        }
        cx.quit();
    }

    /// Title, what's going on, and the line under the buttons.
    fn words(&self) -> (String, String, String) {
        let wsl = crate::wsl::active();
        let (place, installer) = if wsl.is_some() { ("~/.local/bin", LINUX_CORE_INSTALL) } else { (CORE_PLACE, CORE_INSTALL) };
        match &self.need {
            Need::Install => (
                match wsl {
                    Some(d) => format!("Herdr needs herdr-core in {d}"),
                    None => "Herdr needs herdr-core".into(),
                },
                match wsl {
                    Some(d) => format!("herdr-core runs your terminals and agents, and keeps them alive when this window closes. Herdr couldn't find it in WSL ({d})."),
                    None => "herdr-core runs your terminals and agents, and keeps them alive when this window closes. Herdr couldn't find it on this computer.".into(),
                },
                format!("Install it for me puts it in {place} (with hc), like its own installer: {installer}"),
            ),
            Need::Connect(error) => (
                wsl.map_or_else(|| "Herdr can't connect to herdr-core".into(), |d| format!("Herdr can't reach WSL ({d}) or its herdr-core")),
                error.clone(),
                "Once you've resolved this, try again.".into(),
            ),
            Need::Fix(Mismatch::TooOld { have, need }) => (
                "Herdr needs a newer herdr-core".into(),
                format!("Yours is {have}; this Herdr needs {need} or newer. Updating keeps your terminals running."),
                "Or update it yourself: hc update".into(),
            ),
            Need::Fix(Mismatch::Missing(methods)) => (
                "Herdr needs a newer herdr-core".into(),
                format!("Your herdr-core can't do everything this Herdr needs ({}). Updating keeps your terminals running.", methods.join(", ")),
                "Or update it yourself: hc update".into(),
            ),
            Need::Fix(Mismatch::Newer { generation }) => (
                "This Herdr is older than your herdr-core".into(),
                format!("Your herdr-core speaks a newer protocol (generation {generation}) than this Herdr understands. Update Herdr; your terminals keep running."),
                format!("Update Herdr: {APP_INSTALL}"),
            ),
        }
    }
}

/// Blocking setup work, shared by retry, install, update and file selection.
fn connect() -> Result<Arc<Core>, Need> {
    if crate::wsl::active().is_some() {
        let configured = settings::load_person().ok().and_then(|p| p.core);
        let bin = crate::core_bin::find(configured.as_deref())
            .map_err(Need::Connect)?.ok_or(Need::Install)?;
        crate::core_bin::use_core(bin);
    }
    let core = Core::connect().map_err(Need::connection)?;
    crate::hc::check(&core.hello).map_err(Need::Fix)?;
    Ok(core)
}

fn choose_core(bin: PathBuf) -> Result<PathBuf, String> {
    let bin = PathBuf::from(crate::wsl::linux_path_if_active(&bin)?);
    let version = if crate::wsl::active().is_some() {
        crate::wsl::version(&bin)?
    } else {
        crate::core_bin::version(&bin)
    };
    if version.is_none() {
        return Err(format!("{} isn't herdr-core (it should answer `--version` with `hc …`)", bin.display()));
    }
    // Remember only a validated executable, leaving the previous choice on failure.
    settings::load_person().and_then(|mut person| {
        person.core = Some(bin.to_string_lossy().into_owned());
        settings::write_person(&person).map_err(|e| e.to_string())
    }).map_err(|e| format!("Cannot save settings: {e}"))?;
    Ok(bin)
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(38.0))
        .px(px(18.0))
        .flex()
        .items_center()
        .rounded(px(9.0))
        .cursor_pointer()
        .text_size(px(14.0))
        .font_weight(FontWeight::SEMIBOLD)
        .when(primary, |d| d.bg(c("accent")).text_color(c("accent_foreground")).hover(|s| s.opacity(0.9)))
        .when(!primary, |d| d.border_1().border_color(c("border")).text_color(c("foreground")).hover(|s| s.bg(c("raised"))))
        .child(label)
}

impl Render for Setup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = matches!(self.state, State::Busy(_));
        let (title, body, foot) = self.words();
        let wsl = crate::wsl::active().is_some();
        let actions = match &self.need {
            Need::Install => div()
                .flex()
                .gap(px(10.0))
                .child(button("install", "Install it for me", true).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.install(w, cx)))))
                .child(button("choose", "I have it somewhere…", false).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.choose(w, cx)))))
                .when(wsl, |d| d.child(button("use-windows", "Continue without WSL", false).when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.use_windows(cx)))))),
            Need::Connect(_) => div()
                .flex()
                .gap(px(10.0))
                .child(button("retry", "Retry", true).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.start(w, cx)))))
                .when(wsl, |d| d.child(button("use-windows", "Continue without WSL", false).when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.use_windows(cx)))))),
            Need::Fix(Mismatch::Newer { .. }) => div()
                .flex()
                .gap(px(10.0))
                .child(button("quit", "Quit", false).on_click(cx.listener(|_, _, _, cx| cx.quit())))
                .when(wsl, |d| d.child(button("use-windows", "Continue without WSL", false).on_click(cx.listener(|this, _, _, cx| this.use_windows(cx))))),
            Need::Fix(_) => div()
                .flex()
                .gap(px(10.0))
                .child(button("update", "Update herdr-core", true).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.update(w, cx)))))
                .when(wsl, |d| d.child(button("use-windows", "Continue without WSL", false).when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.use_windows(cx)))))),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(c("background"))
            .text_color(c("foreground"))
            .font_family(settings::get().ui_font_family.clone())
            .child(
                div()
                    .w(px(520.0))
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .child(img(self.icon.clone()).w(px(48.0)).h(px(48.0)).flex_shrink_0())
                            // Constrain the title to the space beside the icon so long
                            // WSL distro names wrap instead of widening the whole row.
                            .child(div().flex_1().min_w_0().text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child(title)),
                    )
                    .child(div().text_size(px(14.0)).line_height(px(21.0)).text_color(c("muted")).child(body))
                    .child(actions)
                    .child(div().text_size(px(12.5)).line_height(px(19.0)).text_color(c("dim")).child(foot))
                    .child(match &self.state {
                        State::Ready => div(),
                        State::Busy(what) => div().text_size(px(13.0)).text_color(c("accent")).child(*what),
                        State::Failed(e) => div().text_size(px(13.0)).text_color(c("blocked")).child(e.clone()),
                    }),
            )
    }
}

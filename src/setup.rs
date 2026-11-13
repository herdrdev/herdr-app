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
#[cfg(not(windows))]
const CORE_INSTALL: &str = "curl -fsSL https://releases.herdr.dev/core/install.sh | sh";
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
    Busy(String),
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
    /// Connects to the core; if it serves this app, closes this window and
    /// opens the main one, else says what's still needed.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Core::connect() {
            Ok(core) => match crate::hc::check(&core.hello) {
                Ok(()) => {
                    let Some(requests) = self.requests.take() else { return };
                    window.remove_window();
                    crate::open_main(core, requests, cx);
                    return;
                }
                Err(m) => {
                    self.need = Need::Fix(m);
                    self.state = State::Ready;
                }
            },
            Err(e) => {
                self.need = Need::connection(e);
                self.state = State::Ready;
            }
        }
        cx.notify();
    }

    /// Runs `work` off the UI thread, then `start` again (or shows the error).
    fn run(&mut self, label: String, work: fn() -> Result<Option<PathBuf>, String>, window: &mut Window, cx: &mut Context<Self>) {
        self.state = State::Busy(label);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(bin) => {
                    if let Some(bin) = bin {
                        crate::core_bin::use_core(bin);
                    }
                    this.start(window, cx);
                }
                Err(e) => {
                    this.state = State::Failed(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(crate::i18n::text("setup-installing"), || crate::core_bin::install().map(Some), window, cx);
    }

    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(crate::i18n::text("setup-updating"), || crate::core_bin::update().map(|_| None), window, cx);
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(crate::i18n::text("setup-choose").into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(bin) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if crate::core_bin::version(&bin).is_none() {
                    this.state = State::Failed(crate::i18n::translate("setup-invalid-binary", &serde_json::json!({"path": bin.display().to_string()})));
                    cx.notify();
                    return;
                }
                // Remembered: next time Herdr looks there first.
                let mut person = settings::load_person().unwrap_or_default();
                person.core = Some(bin.to_string_lossy().into_owned());
                let _ = settings::write_person(&person);
                crate::core_bin::use_core(bin);
                this.start(window, cx);
            });
        })
        .detach();
    }

    /// Title, what's going on, and the line under the buttons.
    fn words(&self) -> (String, String, String) {
        match &self.need {
            Need::Install => (
                crate::i18n::text("setup-required"),
                crate::i18n::text("setup-missing"),
                crate::i18n::translate("setup-install-location", &serde_json::json!({"path": CORE_PLACE, "command": CORE_INSTALL})),
            ),
            Need::Connect(error) => (
                crate::i18n::text("setup-connection"),
                error.clone(),
                crate::i18n::text("setup-retry-hint"),
            ),
            Need::Fix(Mismatch::TooOld { have, need }) => (
                crate::i18n::text("setup-newer-required"),
                crate::i18n::translate("setup-version", &serde_json::json!({"have": have, "need": need})),
                crate::i18n::text("setup-manual-update"),
            ),
            Need::Fix(Mismatch::Missing(methods)) => (
                crate::i18n::text("setup-newer-required"),
                crate::i18n::translate("setup-capabilities", &serde_json::json!({"methods": methods.join(", ")})),
                crate::i18n::text("setup-manual-update"),
            ),
            Need::Fix(Mismatch::Newer { generation }) => (
                crate::i18n::text("setup-app-older"),
                crate::i18n::translate("setup-protocol", &serde_json::json!({"generation": generation})),
                crate::i18n::translate("setup-update-command", &serde_json::json!({"command": APP_INSTALL})),
            ),
        }
    }
}

fn button(id: &'static str, label: String, primary: bool) -> Stateful<Div> {
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
        let actions = match &self.need {
            Need::Install => div()
                .flex()
                .gap(px(10.0))
                .child(button("install", crate::i18n::text("setup-install-button"), true).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.install(w, cx)))))
                .child(button("choose", crate::i18n::text("setup-choose-button"), false).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.choose(w, cx))))),
            Need::Connect(_) => div().child(button("retry", crate::i18n::text("setup-retry"), true).on_click(cx.listener(|this, _, w, cx| this.start(w, cx)))),
            Need::Fix(Mismatch::Newer { .. }) => div()
                .flex()
                .gap(px(10.0))
                .child(button("quit", crate::i18n::text("setup-quit"), false).on_click(cx.listener(|_, _, _, cx| cx.quit()))),
            Need::Fix(_) => div()
                .flex()
                .gap(px(10.0))
                .child(button("update", crate::i18n::text("setup-update-button"), true).when(!busy, |d| d.on_click(cx.listener(|this, _, w, cx| this.update(w, cx))))),
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
                            .child(img(self.icon.clone()).w(px(48.0)).h(px(48.0)))
                            .child(div().text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child(title)),
                    )
                    .child(div().text_size(px(14.0)).line_height(px(21.0)).text_color(c("muted")).child(body))
                    .child(actions)
                    .child(div().text_size(px(12.5)).line_height(px(19.0)).text_color(c("dim")).child(foot))
                    .child(match &self.state {
                        State::Ready => div(),
                        State::Busy(what) => div().text_size(px(13.0)).text_color(c("accent")).child(what.clone()),
                        State::Failed(e) => div().text_size(px(13.0)).text_color(c("blocked")).child(e.clone()),
                    }),
            )
    }
}

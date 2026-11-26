// HERDR: a mod answers a code editor's language requests with LSP's own JSON.
use std::rc::{Rc, Weak};

use anyhow::{Result, anyhow};
use gpui::{App, Entity, SharedString, Task, Window};
use gpui_base::input::lsp_types::{
    self, CodeAction, CodeActionOrCommand, CompletionContext, CompletionResponse,
    GotoDefinitionResponse, Hover, LocationLink, NumberOrString, ShowDocumentParams,
};
use gpui_base::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, Diagnostic, DiagnosticSeverity,
    EditorState, HoverProvider, Rope, RopeExt as _,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

pub(crate) type ScriptReply = async_channel::Sender<std::result::Result<String, String>>;

pub(crate) struct ScriptHandler {
    name: &'static str,
    call: Box<dyn Fn(String, ScriptReply, &mut Window, &mut App)>,
}

impl ScriptHandler {
    pub(crate) fn new(
        name: &'static str,
        call: impl Fn(String, ScriptReply, &mut Window, &mut App) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            name,
            call: Box::new(call),
        })
    }
}

/// Owned by the editor's store record. Everything else holds these weakly: they
/// keep script functions alive, and those must go before the script runtime
/// does, while GPUI may drop the editor entity later than that.
#[derive(Default)]
pub(crate) struct LanguageHandlers {
    pub hover: Option<Rc<ScriptHandler>>,
    pub completion: Option<Rc<ScriptHandler>>,
    pub completion_accepted: Option<Rc<ScriptHandler>>,
    pub definition: Option<Rc<ScriptHandler>>,
    pub code_actions: Option<Rc<ScriptHandler>>,
    pub perform_code_action: Option<Rc<ScriptHandler>>,
    pub show_document: Option<Rc<ScriptHandler>>,
    pub completion_triggers: Vec<String>,
}

fn weak(handler: &Option<Rc<ScriptHandler>>) -> Option<Weak<ScriptHandler>> {
    handler.as_ref().map(Rc::downgrade)
}

pub(crate) fn ask<R: DeserializeOwned + 'static>(
    handler: &Weak<ScriptHandler>,
    params: impl Serialize,
    window: &mut Window,
    cx: &mut App,
) -> Task<Result<R>> {
    let Some(name) = handler.upgrade().map(|handler| handler.name) else {
        return Task::ready(Err(anyhow!("the editor state has been released")));
    };
    let params = match serde_json::to_string(&params) {
        Ok(params) => params,
        Err(error) => return Task::ready(Err(error.into())),
    };
    let (reply, answer) = async_channel::bounded(1);
    let handler = handler.clone();
    // The editor asks from inside its own update, and a handler that reads the
    // editor would panic there.
    window.defer(cx, move |window, cx| {
        if let Some(handler) = handler.upgrade() {
            (handler.call)(params, reply, window, cx);
        }
    });
    cx.spawn(async move |_| {
        let Ok(answer) = answer.recv().await else {
            return Err(anyhow!("{name}: the mod stopped before answering"));
        };
        let parsed = answer
            .map_err(|message| anyhow!(message))
            .and_then(|json| serde_json::from_str(&json).map_err(Into::into));
        if let Err(error) = &parsed {
            tracing::error!("error in editor lsp {name}: {error:#}");
        }
        parsed
    })
}

fn map<T: 'static, U: 'static>(
    task: Task<Result<T>>,
    convert: impl FnOnce(T) -> U + 'static,
    cx: &mut App,
) -> Task<Result<U>> {
    cx.spawn(async move |_| task.await.map(convert))
}

struct ScriptHover(Weak<ScriptHandler>);

impl HoverProvider for ScriptHover {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let params = json!({ "position": text.offset_to_position(offset) });
        ask(&self.0, params, window, cx)
    }
}

struct ScriptCompletion {
    handler: Weak<ScriptHandler>,
    triggers: Vec<String>,
}

impl CompletionProvider for ScriptCompletion {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let params = json!({ "position": text.offset_to_position(offset), "context": trigger });
        let answer = ask::<Option<CompletionResponse>>(&self.handler, params, window, cx);
        map(
            answer,
            |answer| answer.unwrap_or(CompletionResponse::Array(Vec::new())),
            cx,
        )
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        self.triggers.iter().any(|trigger| trigger == new_text)
            || new_text.chars().any(|ch| ch.is_alphanumeric() || ch == '_')
    }
}

struct ScriptDefinition(Weak<ScriptHandler>);

impl DefinitionProvider for ScriptDefinition {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let params = json!({ "position": text.offset_to_position(offset) });
        let answer = ask::<Option<GotoDefinitionResponse>>(&self.0, params, window, cx);
        map(answer, location_links, cx)
    }
}

fn location_links(answer: Option<GotoDefinitionResponse>) -> Vec<LocationLink> {
    let link = |location: lsp_types::Location| LocationLink {
        origin_selection_range: None,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    };
    match answer {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
        Some(GotoDefinitionResponse::Array(locations)) => locations.into_iter().map(link).collect(),
        Some(GotoDefinitionResponse::Link(links)) => links,
    }
}

struct ScriptCodeActions {
    list: Weak<ScriptHandler>,
    perform: Weak<ScriptHandler>,
}

impl CodeActionProvider for ScriptCodeActions {
    fn id(&self) -> SharedString {
        "script".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let state = state.read(cx);
        // A diagnostic's `data` round-trips here, which is how a server finds
        // the quick fix for the squiggle under the caret.
        let diagnostics: Vec<_> = state
            .diagnostics()
            .into_iter()
            .flat_map(|set| set.iter())
            .filter(|entry| entry.range.start <= range.end && range.start <= entry.range.end)
            .map(|entry| lsp_diagnostic(entry))
            .collect();
        let params = json!({
            "range": lsp_types::Range::new(
                state.text().offset_to_position(range.start),
                state.text().offset_to_position(range.end),
            ),
            "context": { "diagnostics": diagnostics },
        });
        let answer = ask::<Option<Vec<CodeActionOrCommand>>>(&self.list, params, window, cx);
        map(answer, code_action_list, cx)
    }

    fn perform_code_action(
        &self,
        _state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let answer = ask::<serde_json::Value>(&self.perform, action, window, cx);
        map(answer, |_| (), cx)
    }
}

/// The menu can show one kind of row, and a disabled action has nothing to run.
fn code_action_list(answer: Option<Vec<CodeActionOrCommand>>) -> Vec<CodeAction> {
    answer
        .unwrap_or_default()
        .into_iter()
        .map(|entry| match entry {
            CodeActionOrCommand::CodeAction(action) => action,
            CodeActionOrCommand::Command(command) => CodeAction {
                title: command.title.clone(),
                command: Some(command),
                ..Default::default()
            },
        })
        .filter(|action| action.disabled.is_none())
        .collect()
}

fn lsp_diagnostic(diagnostic: &Diagnostic) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: lsp_types::Range::new(diagnostic.range.start, diagnostic.range.end),
        severity: Some(match diagnostic.severity {
            DiagnosticSeverity::Error => lsp_types::DiagnosticSeverity::ERROR,
            DiagnosticSeverity::Warning => lsp_types::DiagnosticSeverity::WARNING,
            DiagnosticSeverity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
            DiagnosticSeverity::Hint => lsp_types::DiagnosticSeverity::HINT,
        }),
        code: diagnostic
            .code
            .as_ref()
            .map(|code| NumberOrString::String(code.to_string())),
        code_description: diagnostic.code_description.clone(),
        source: diagnostic.source.as_ref().map(ToString::to_string),
        message: diagnostic.message.to_string(),
        related_information: diagnostic.related_information.clone(),
        tags: diagnostic.tags.clone(),
        data: diagnostic.data.clone(),
    }
}

pub(crate) fn install(state: &Entity<EditorState>, handlers: &LanguageHandlers, cx: &mut App) {
    let editor = state.downgrade();
    let show_document = weak(&handlers.show_document);
    let hook: Option<gpui_base::input::ShowDocumentHandler> =
        show_document.map(|handler| -> gpui_base::input::ShowDocumentHandler {
            Rc::new(
                move |params: &ShowDocumentParams, window: &mut Window, cx: &mut App| {
                    // Base wants the answer now, from inside its update, where a
                    // handler that reads the editor would panic. So claim the
                    // document and fall back once the mod has really answered.
                    let params = params.clone();
                    let shown = ask::<Option<bool>>(&handler, params.clone(), window, cx);
                    let editor = editor.clone();
                    window
                        .spawn(cx, async move |cx| {
                            if matches!(shown.await, Ok(Some(true))) {
                                return;
                            }
                            editor
                                .update(cx, |editor, cx| {
                                    editor.show_document_default(&params, cx);
                                })
                                .ok();
                        })
                        .detach();
                    true
                },
            )
        });
    state.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        lsp.hover_provider = weak(&handlers.hover)
            .map(|handler| Rc::new(ScriptHover(handler)) as Rc<dyn HoverProvider>);
        lsp.completion_provider = weak(&handlers.completion).map(|handler| {
            Rc::new(ScriptCompletion {
                handler,
                triggers: handlers.completion_triggers.clone(),
            }) as Rc<dyn CompletionProvider>
        });
        lsp.definition_provider = weak(&handlers.definition)
            .map(|handler| Rc::new(ScriptDefinition(handler)) as Rc<dyn DefinitionProvider>);
        lsp.code_action_providers = weak(&handlers.code_actions)
            .zip(weak(&handlers.perform_code_action))
            .map(|(list, perform)| {
                Rc::new(ScriptCodeActions { list, perform }) as Rc<dyn CodeActionProvider>
            })
            .into_iter()
            .collect();
        lsp.show_document = hook;
        state.refresh(cx);
    });
}

#[derive(Deserialize)]
pub(crate) struct SemanticToken {
    range: lsp_types::Range,
    #[serde(rename = "type")]
    token_type: String,
}

/// Diagnostics and tokens come from a server working on a recent version of the
/// document, so stale positions are expected and only ever clamp.
pub(crate) fn set_diagnostics(state: &mut EditorState, mut list: Vec<lsp_types::Diagnostic>) {
    for diagnostic in &mut list {
        let range = &mut diagnostic.range;
        if range.end < range.start {
            std::mem::swap(&mut range.start, &mut range.end);
        }
    }
    // The set seeks by start position.
    list.sort_by_key(|diagnostic| diagnostic.range.start);
    let text = state.text().clone();
    if let Some(set) = state.diagnostics_mut() {
        set.reset(&text);
        set.extend(list);
    }
}

pub(crate) fn set_semantic_tokens(state: &mut EditorState, list: Vec<SemanticToken>) {
    state.lsp_mut().set_semantic_tokens(
        list.into_iter()
            .map(|token| (token.range, token.token_type.into()))
            .collect(),
    );
}

pub(crate) fn cursor_json(state: &EditorState) -> String {
    json!(state.cursor_position()).to_string()
}

pub(crate) fn selection_json(state: &EditorState) -> String {
    let range = state.selected_range();
    if range.is_empty() {
        return "null".to_owned();
    }
    json!(lsp_types::Range::new(
        state.text().offset_to_position(range.start),
        state.text().offset_to_position(range.end),
    ))
    .to_string()
}

pub(crate) fn cursor_event_json(state: &EditorState) -> String {
    format!(
        r#"{{"position":{},"selection":{}}}"#,
        cursor_json(state),
        selection_json(state)
    )
}

pub(crate) fn select_range(
    state: &mut EditorState,
    range: lsp_types::Range,
    cx: &mut gpui::Context<EditorState>,
) {
    let start = state.text().position_to_offset(&range.start);
    let end = state.text().position_to_offset(&range.end);
    state.set_selected_range(start.min(end)..start.max(end), cx);
}

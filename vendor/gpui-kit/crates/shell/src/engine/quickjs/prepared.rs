//! Host-owned modules whose View may be constructed later.
use super::*;
use crate::{ShellRoot, host_modules::HostValue};
use std::{path::PathBuf, time::Duration};

struct Settled(Result<HostValue>);
impl<'js> FromJs<'js> for Settled {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> JsResult<Self> {
        Ok(Self(super::host_modules::from_js(ctx, value, 0).map_err(
            |error| anyhow!("{}", super::describe(ctx, error)),
        )))
    }
}

struct Rejected(String);
impl<'js> FromJs<'js> for Rejected {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> JsResult<Self> {
        let message = if let Some(error) = value.as_exception() {
            error
                .stack()
                .or_else(|| error.message())
                .unwrap_or_else(|| "application Promise rejected".into())
        } else if let Some(text) = value.as_string() {
            text.to_string()?
        } else {
            ctx.json_stringify(value)?
                .and_then(|s| s.to_string().ok())
                .unwrap_or_else(|| "application Promise rejected".into())
        };
        Ok(Self(message))
    }
}

/// An evaluated application, with its own policy and module generation.
/// Dropping an unmounted application cancels its asynchronous work.
pub struct PreparedApplication {
    runtime: Rc<ShellRuntime>,
    view_type: ViewType,
    policy: Rc<Policy>,
    directory: PathBuf,
    entry: String,
    mounted: bool,
}

/// One invocation. Dropping it makes any late result inert.
pub struct ApplicationCall {
    receiver: async_channel::Receiver<Result<HostValue>>,
    live: Rc<Cell<bool>>,
    application: Option<Rc<ApplicationGeneration>>,
    _promise: Option<Persistent<Value<'static>>>,
}

impl ApplicationCall {
    /// Waits without polling. Only a pending invocation has a deadline wakeup.
    pub async fn resolve(self, timeout: Duration) -> Result<HostValue> {
        let result = smol::future::or(
            async {
                self.receiver
                    .recv()
                    .await
                    .map_err(|_| anyhow!("application invocation was cancelled"))?
            },
            async {
                smol::Timer::after(timeout).await;
                Err(anyhow!("application invocation timed out"))
            },
        )
        .await;
        anyhow::ensure!(
            self.application.as_ref().is_none_or(|app| app.is_active()),
            "application invocation belongs to a retired generation"
        );
        result
    }
}

impl Drop for ApplicationCall {
    fn drop(&mut self) {
        self.live.set(false);
    }
}

impl Drop for PreparedApplication {
    fn drop(&mut self) {
        if !self.mounted {
            if let Some(application) = &self.view_type.application {
                self.runtime
                    .release_application_generation_without_context(application);
            }
        }
    }
}

impl ShellRuntime {
    /// Evaluates an entry module, without constructing its default View.
    /// The host's current policy is captured for evaluation, calls and mounting.
    pub fn prepare_application(
        self: &Rc<Self>,
        directory: impl AsRef<Path>,
        entry: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<PreparedApplication> {
        let directory = crate::runtime::resolve_app_root(directory.as_ref(), entry)?;
        self.prepare_application_with_policy(directory, entry, crate::policy::default(), window, cx)
    }

    fn prepare_application_with_policy(
        self: &Rc<Self>,
        directory: PathBuf,
        entry: &str,
        policy: Rc<Policy>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<PreparedApplication> {
        let view_type = {
            let (_guard, _) =
                scope::enter_with_runtime(self, window, cx, ScopePhase::Task, None, policy.clone());
            self.load_app(&directory, entry)?
        };
        Ok(PreparedApplication {
            runtime: self.clone(),
            view_type,
            policy,
            directory,
            entry: entry.to_owned(),
            mounted: false,
        })
    }
}

impl PreparedApplication {
    fn enter<'a>(&self, window: &'a mut Window, cx: &'a mut App) -> (scope::CallScopeGuard, u64) {
        scope::enter_with_application(
            &self.runtime,
            window,
            cx,
            ScopePhase::Task,
            None,
            self.policy.clone(),
            self.view_type.application.clone(),
        )
    }

    /// Reads a named export as plain host data; absent exports return None.
    pub fn export(
        &self,
        name: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Option<HostValue>> {
        let (_guard, _) = self.enter(window, cx);
        self.runtime.with_js(|ctx| {
            let namespace = self
                .view_type
                .namespace
                .as_ref()
                .expect("prepared module")
                .clone()
                .restore(ctx)?;
            if !namespace.contains_key(name)? {
                return Ok(None);
            }
            super::host_modules::from_js(ctx, namespace.get(name)?, 0).map(Some)
        })
    }

    /// Tests export presence without converting (or calling) the export.
    pub fn has_export(&self, name: &str, window: &mut Window, cx: &mut App) -> Result<bool> {
        let (_guard, _) = self.enter(window, cx);
        self.runtime.with_js(|ctx| {
            self.view_type
                .namespace
                .as_ref()
                .expect("prepared module")
                .clone()
                .restore(ctx)?
                .contains_key(name)
        })
    }

    /// Calls a named function under this application's policy and generation.
    /// Both plain values and Promises use the same asynchronous result handle.
    pub fn call(
        &self,
        name: &str,
        argument: HostValue,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<ApplicationCall> {
        anyhow::ensure!(
            self.view_type
                .application
                .as_ref()
                .is_none_or(|app| app.is_active()),
            "prepared application has been retired"
        );
        let (sender, receiver) = async_channel::bounded(1);
        let live = Rc::new(Cell::new(true));
        let (_guard, _) = self.enter(window, cx);
        let promise = self.runtime.with_js(|ctx| {
            let namespace = self
                .view_type
                .namespace
                .as_ref()
                .expect("prepared module")
                .clone()
                .restore(ctx)?;
            let function: Function = namespace.get(name)?;
            let argument = super::host_modules::into_js(ctx, argument)?;
            let value: Value = function.call((argument,))?;
            if let Some(promise) = value.as_promise() {
                let success_sender = sender.clone();
                let success_live = live.clone();
                let success = Function::new(ctx.clone(), move |result: Settled| {
                    if success_live.get() {
                        let _ = success_sender.try_send(result.0);
                    }
                })?;
                let failure_live = live.clone();
                let failure = Function::new(ctx.clone(), move |error: Rejected| {
                    if failure_live.get() {
                        let _ = sender.try_send(Err(anyhow!("{}", error.0)));
                    }
                })?;
                let _: Value = promise
                    .then()?
                    .call((This(promise.clone()), success, failure))?;
                Ok(Some(Persistent::save(ctx, value)))
            } else {
                let result = super::host_modules::from_js(ctx, value, 0)?;
                let _ = sender.try_send(Ok(result));
                Ok(None)
            }
        })?;
        scheduler::drain_runtime_jobs(&self.runtime, window, cx);
        Ok(ApplicationCall {
            receiver,
            live,
            application: self.view_type.application.clone(),
            _promise: promise,
        })
    }

    /// Mounts exactly the class evaluated by prepare, without a second import.
    pub fn mount(&mut self, window: &mut Window, cx: &mut App) -> Result<Entity<ShellRoot>> {
        anyhow::ensure!(
            !self.mounted,
            "prepared application has already been mounted"
        );
        // A failed constructor also retires its generation; do not retry it.
        self.mounted = true;
        let view = self.runtime.instantiate_view_with_policy(
            &self.view_type,
            self.policy.clone(),
            window,
            cx,
        )?;
        Ok(cx.new(|cx| {
            ShellRoot::with_application(
                view,
                self.directory.clone(),
                self.entry.clone(),
                self.policy.clone(),
                window,
                cx,
            )
        }))
    }

    /// Replaces an unmounted module after a source change. A failed replacement
    /// leaves the previous generation available. Hosts supply file notifications.
    pub fn reload(&mut self, window: &mut Window, cx: &mut App) -> Result<()> {
        anyhow::ensure!(
            !self.mounted,
            "reload mounted applications through their root"
        );
        let replacement = self.runtime.prepare_application_with_policy(
            self.directory.clone(),
            &self.entry,
            self.policy.clone(),
            window,
            cx,
        )?;
        *self = replacement;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::{
        ops::Deref,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Source(PathBuf);
    impl Source {
        fn new(source: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = if cfg!(target_os = "linux") {
                PathBuf::from("/var/tmp")
            } else {
                std::env::temp_dir()
            };
            let dir = base.join(format!(
                "prepared-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let result = Self(dir);
            result.write(source);
            result
        }
        fn write(&self, source: &str) {
            std::fs::write(self.0.join("main.js"), source).unwrap();
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    const SOURCE: &str = r#"
        import { View, div } from "gpui-kit";
        let constructed = 0;
        export const watch = ["folder"];
        export function when() { return constructed === 0; }
        export async function later() { return true; }
        export function bad() { return Promise.reject(new Error("probe failed")); }
        export function never() { return new Promise(() => {}); }
        export default class App extends View {
            constructor(...args) { super(...args); constructed++; }
            render() { return div(); }
        }
    "#;

    #[gpui::test]
    fn prepared_application_defers_construction_and_mounts_once(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(SOURCE);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let mut prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            assert!(prepared.has_export("when", window, cx).unwrap());
            assert!(!prepared.has_export("missing", window, cx).unwrap());
            assert_eq!(
                prepared.export("watch", window, cx).unwrap(),
                Some(HostValue::Array(vec![HostValue::Str("folder".into())]))
            );
            let call = prepared.call("when", HostValue::Null, window, cx).unwrap();
            assert_eq!(
                call.receiver.try_recv().unwrap().unwrap(),
                HostValue::Bool(true)
            );
            let root = prepared.mount(window, cx).unwrap();
            let call = prepared.call("when", HostValue::Null, window, cx).unwrap();
            assert_eq!(
                call.receiver.try_recv().unwrap().unwrap(),
                HostValue::Bool(false)
            );
            assert!(prepared.mount(window, cx).is_err());
            drop(root);
        });
    }

    #[gpui::test]
    fn prepared_calls_accept_promises_and_report_rejections(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(SOURCE);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let call = prepared.call("later", HostValue::Null, window, cx).unwrap();
            assert_eq!(
                call.receiver.try_recv().unwrap().unwrap(),
                HostValue::Bool(true)
            );
            let call = prepared.call("bad", HostValue::Null, window, cx).unwrap();
            assert!(call.receiver.try_recv().unwrap().is_err());
            let call = prepared.call("never", HostValue::Null, window, cx).unwrap();
            assert!(
                smol::block_on(call.resolve(Duration::ZERO))
                    .unwrap_err()
                    .to_string()
                    .contains("timed out")
            );
        });
    }

    #[gpui::test]
    fn prepared_reload_preserves_old_module_on_failure_and_retires_old_calls(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let source = Source::new(SOURCE);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let mut prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let old = prepared.call("later", HostValue::Null, window, cx).unwrap();
            source.write("not valid javascript");
            assert!(prepared.reload(window, cx).is_err());
            assert!(prepared.has_export("when", window, cx).unwrap());
            source.write(SOURCE);
            prepared.reload(window, cx).unwrap();
            assert!(
                smol::block_on(old.resolve(Duration::ZERO))
                    .unwrap_err()
                    .to_string()
                    .contains("retired")
            );
            let generation = prepared.view_type.application.clone().unwrap();
            drop(prepared);
            assert!(!generation.is_active());
        });
    }
}

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
            let message = error
                .message()
                .unwrap_or_else(|| "application Promise rejected".into());
            match error.stack() {
                Some(stack) => format!("{message}\n{stack}"),
                None => message,
            }
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
/// Hosts must not enter this facade from inside a JavaScript/host callback.
pub struct PreparedApplication {
    view_type: ViewType,
    policy: Rc<Policy>,
    directory: PathBuf,
    entry: String,
    mounted: bool,
    runtime: Rc<ShellRuntime>,
}

/// One invocation. Dropping it makes any late result inert.
pub struct ApplicationCall {
    receiver: async_channel::Receiver<Result<HostValue>>,
    live: Rc<Cell<bool>>,
    application: Option<Rc<ApplicationGeneration>>,
    _promise: Option<Persistent<Value<'static>>>,
    _runtime: Rc<ShellRuntime>,
    started: std::time::Instant,
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
                smol::Timer::after(timeout.saturating_sub(self.started.elapsed())).await;
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
        let policy = Rc::new(crate::policy::default().duplicate());
        let result =
            self.prepare_application_with_policy(directory, entry, policy.clone(), window, cx);
        if result.is_err() {
            super::cancel_policy_tasks(&policy);
        }
        result
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
            self.load_app_prepared(&directory, entry)
        };
        let view_type = view_type?;
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
        let promise = sandbox::with_deadline(Duration::from_millis(50), || {
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
                    let _: Value =
                        promise
                            .then()?
                            .call((This(promise.clone()), success, failure))?;
                    Ok(Some(Persistent::save(ctx, value)))
                } else {
                    let result = super::host_modules::from_js(ctx, value, 0)?;
                    let _ = sender.try_send(Ok(result));
                    Ok(None)
                }
            });
            scheduler::drain_runtime_jobs(&self.runtime, window, cx);
            promise
        })?;
        Ok(ApplicationCall {
            receiver,
            live,
            application: self.view_type.application.clone(),
            _promise: promise,
            _runtime: self.runtime.clone(),
            started: std::time::Instant::now(),
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
        let view = match self.runtime.instantiate_view_with_policy(
            &self.view_type,
            self.policy.clone(),
            window,
            cx,
        ) {
            Ok(view) => view,
            Err(error) => {
                super::cancel_policy_tasks(&self.policy);
                return Err(error);
            }
        };
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

    /// Constructs and checks this application's eager native description.
    pub fn check(&mut self, window: &mut Window, cx: &mut App) -> Result<String> {
        let root = self.mount(window, cx)?;
        self.runtime.check_root(&root, window, cx)
    }

    /// Follows a root reloaded by the ordinary shell watcher. Named calls use
    /// the new module without replacing the root or constructing another View.
    pub fn sync_mounted(
        &mut self,
        root: &Entity<ShellRoot>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<bool> {
        anyhow::ensure!(self.mounted, "prepared application has not been mounted");
        let view = root
            .read(cx)
            .application()
            .context("root has no application")?
            .view
            .clone();
        anyhow::ensure!(
            Rc::ptr_eq(&view.read(cx).runtime(), &self.runtime),
            "root belongs to a different runtime"
        );
        let object = view.read(cx).object().clone();
        let actual = object.application_generation();
        if actual
            .as_ref()
            .zip(self.view_type.application.as_ref())
            .is_some_and(|(actual, expected)| Rc::ptr_eq(actual, expected))
        {
            return Ok(false);
        }
        let namespace = object
            .namespace
            .clone()
            .context("root has no module exports")?;
        let (_guard, _) = scope::enter_with_application(
            &self.runtime,
            window,
            cx,
            ScopePhase::Task,
            Some(view.clone()),
            self.policy.clone(),
            actual.clone(),
        );
        let value = self.runtime.with_js(|ctx| {
            let namespace = namespace.clone().restore(ctx)?;
            Ok(Persistent::save(
                ctx,
                namespace.get::<_, Object>("default")?,
            ))
        })?;
        self.view_type = ViewType {
            value,
            namespace: Some(namespace),
            module_lease: object.module_lease.clone(),
            application: actual,
        };
        Ok(true)
    }

    /// Replaces an unmounted or retired module after a source change. A failed
    /// replacement keeps the previous generation. A failed mount retires its
    /// generation too, so fixing its source can construct a fresh View.
    pub fn reload(&mut self, window: &mut Window, cx: &mut App) -> Result<()> {
        anyhow::ensure!(
            !self.mounted
                || self
                    .view_type
                    .application
                    .as_ref()
                    .is_some_and(|application| !application.is_active()),
            "reload active mounted applications through their root"
        );
        let replacement = self.runtime.prepare_application_with_policy(
            self.directory.clone(),
            &self.entry,
            self.policy.clone(),
            window,
            cx,
        )?;
        if let Some(previous) = &self.view_type.application {
            self.runtime.release_application_generation(previous, cx);
        }
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

    fn probe_policy(
        source: &Source,
        receiver: async_channel::Receiver<()>,
        seen: Rc<RefCell<Vec<String>>>,
    ) {
        let module = crate::HostModule::new("probe")
            .async_function("pending", move |_| {
                let receiver = receiver.clone();
                Ok(async move {
                    receiver
                        .recv()
                        .await
                        .map_err(|_| crate::HostError::new("probe closed"))?;
                    Ok(HostValue::Null)
                })
            })
            .function("mark", move |args| {
                seen.borrow_mut().push(args.string(0)?.to_owned());
                Ok(HostValue::Null)
            });
        crate::policy::set_default(
            Policy::new()
                .with_application("prepared-equivalence")
                .with_storage_path(source.0.join("store.json"))
                .with_host_module(module)
                .unwrap(),
        );
    }

    #[gpui::test]
    fn plain_panels_match_try_load_and_keep_independent_timers_and_host_work(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let plain = |name| {
            format!(
                r#"
            import {{ View, div }} from "gpui-kit";
            import {{ pending, mark }} from "probe";
            export default class App extends View {{
                init(props, cx) {{
                    this.timer = cx.timer.every(10, () => mark("{name}-timer"));
                    pending().then(() => mark("{name}-fetch"));
                }}
                render() {{ return div(); }}
            }}
        "#
            )
        };
        let a = Source::new(&plain("a"));
        let b = Source::new(&plain("b"));
        let standard = Source::new(
            r#"import { View, div } from "gpui-kit"; export default class App extends View { render() { return div(); } }"#,
        );
        let (tx, rx) = async_channel::unbounded();
        let seen = Rc::new(RefCell::new(Vec::new()));
        probe_policy(&a, rx, seen.clone());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let (prepared_a, root_a, prepared_b, root_b) = context.update(|window, cx| {
            let mut a = runtime
                .prepare_application(&a.0, "main.js", window, cx)
                .unwrap();
            let mut b = runtime
                .prepare_application(&b.0, "main.js", window, cx)
                .unwrap();
            assert!(!a.has_export("when", window, cx).unwrap());
            assert!(!Rc::ptr_eq(&a.policy, &b.policy));
            let ordinary = runtime.try_load(&standard.0, window, cx).unwrap();
            let ordinary_view = ordinary.read(cx).application().unwrap().view.clone();
            let ordinary_policy = ordinary_view.read(cx).policy();
            assert_eq!(ordinary_policy.application(), a.policy.application());
            assert!(
                ordinary_view.read(cx).object().namespace.is_none(),
                "legacy loads must not retain exports"
            );
            a.policy
                .with_local_storage(|store| {
                    store.set("equivalence".into(), serde_json::json!("shared"))
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                ordinary_policy
                    .with_local_storage(|store| store.values().unwrap()["equivalence"].clone()),
                Some(serde_json::json!("shared"))
            );
            let root_a = a.mount(window, cx).unwrap();
            let root_b = b.mount(window, cx).unwrap();
            (a, root_a, b, root_b)
        });
        context.run_until_parked();
        drop(root_b);
        drop(prepared_b);
        context.update(|_, _| {});
        context.run_until_parked();
        tx.try_send(()).unwrap();
        context.executor().advance_clock(Duration::from_millis(30));
        context.run_until_parked();
        assert!(
            seen.borrow().iter().any(|item| item == "a-timer"),
            "{:?}",
            seen.borrow()
        );
        assert!(
            seen.borrow().iter().any(|item| item == "a-fetch"),
            "{:?}",
            seen.borrow()
        );
        assert!(
            !seen.borrow().iter().any(|item| item.starts_with("b-")),
            "{:?}",
            seen.borrow()
        );
        drop(root_a);
        drop(prepared_a);
        context.update(|_, _| {});
        crate::policy::set_default(Policy::new());
    }

    #[gpui::test]
    fn prepared_and_pending_call_can_be_the_last_runtime_owners(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(SOURCE);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let weak = Rc::downgrade(&runtime);
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let (prepared, call) = context.update(|window, cx| {
            let prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let call = prepared.call("never", HostValue::Null, window, cx).unwrap();
            (prepared, call)
        });
        drop(runtime);
        drop(prepared);
        assert!(
            weak.upgrade().is_some(),
            "call must keep the VM alive for its Persistent"
        );
        drop(call);
        assert!(weak.upgrade().is_none());

        let runtime = ShellRuntime::new_isolated().unwrap();
        let weak = Rc::downgrade(&runtime);
        let prepared = context.update(|window, cx| {
            runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap()
        });
        drop(runtime);
        drop(prepared);
        assert!(
            weak.upgrade().is_none(),
            "prepared handles must drop before their last runtime owner"
        );
    }

    #[gpui::test]
    fn synchronous_call_has_a_tighter_deadline_without_changing_task_budget(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let source = Source::new(
            r#"
            import { View } from "gpui-kit";
            export function when() { for (;;) {} }
            export default class App extends View {}
        "#,
        );
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let started = std::time::Instant::now();
            let error = match prepared.call("when", HostValue::Null, window, cx) {
                Ok(_) => panic!("infinite when must be interrupted"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("interrupt"), "{error:#}");
            assert!(
                started.elapsed() < Duration::from_millis(400),
                "condition used the 500 ms Task budget"
            );
            let (_guard, _) = scope::enter_with_runtime(
                &runtime,
                window,
                cx,
                ScopePhase::Task,
                None,
                prepared.policy.clone(),
            );
            runtime
                .with_js(|ctx| {
                    ctx.eval::<(), _>(
                        "const start = Date.now(); while (Date.now() - start < 80) {}",
                    )
                })
                .unwrap();
        });
    }

    #[gpui::test]
    fn a_throwing_call_drains_jobs_under_its_own_policy(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(
            r#"
            import { View } from "gpui-kit";
            import { mark } from "probe";
            export function when() { Promise.resolve().then(() => mark("own-reaction")); throw new Error("thrown"); }
            export default class App extends View {}
        "#,
        );
        let (_tx, rx) = async_channel::unbounded();
        let seen = Rc::new(RefCell::new(Vec::new()));
        probe_policy(&source, rx, seen.clone());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            assert!(prepared.call("when", HostValue::Null, window, cx).is_err());
            assert_eq!(*seen.borrow(), vec!["own-reaction"]);
        });
        crate::policy::set_default(Policy::new());
    }

    #[gpui::test]
    fn dropping_unmounted_app_cancels_real_work_and_dropped_call_ignores_late_result(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let source = Source::new(
            r#"
            import { View } from "gpui-kit";
            import { pending, mark } from "probe";
            pending().then(() => mark("unmounted-resumed"));
            export async function when() { await pending(); return true; }
            export default class App extends View {}
        "#,
        );
        let (tx, rx) = async_channel::unbounded();
        let seen = Rc::new(RefCell::new(Vec::new()));
        probe_policy(&source, rx.clone(), seen.clone());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let prepared = context.update(|window, cx| {
            runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap()
        });
        context.run_until_parked();
        drop(prepared);
        context.run_until_parked();
        tx.try_send(()).unwrap();
        context.run_until_parked();
        assert_eq!(
            rx.try_recv(),
            Ok(()),
            "cancelled top-level work must not consume a message"
        );
        assert!(seen.borrow().is_empty());

        source.write(
            r#"
            import { View } from "gpui-kit";
            import { pending } from "probe";
            export async function when() { await pending(); return true; }
            export default class App extends View {}
        "#,
        );
        let (prepared, call) = context.update(|window, cx| {
            let prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let call = prepared.call("when", HostValue::Null, window, cx).unwrap();
            (prepared, call)
        });
        let receiver = call.receiver.clone();
        drop(call);
        context.run_until_parked();
        tx.try_send(()).unwrap();
        context.run_until_parked();
        assert!(
            receiver.try_recv().is_err(),
            "a dropped invocation must not deliver a late answer"
        );
        drop(prepared);
        context.run_until_parked();
        crate::policy::set_default(Policy::new());
    }

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
            assert!(
                call.receiver
                    .try_recv()
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("probe failed")
            );
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
    fn a_failed_first_mount_can_recover_after_a_source_fix(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(
            r#"
            import { View } from "gpui-kit";
            export function when() { return true; }
            export default class App extends View { init() { throw new Error("broken constructor"); } }
        "#,
        );
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let mut prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            assert!(prepared.mount(window, cx).is_err());
            assert!(
                prepared.mount(window, cx).is_err(),
                "one mount attempt per generation"
            );
            source.write(SOURCE);
            prepared.reload(window, cx).unwrap();
            let root = prepared.mount(window, cx).unwrap();
            assert!(
                prepared.reload(window, cx).is_err(),
                "active mounted roots still own reload"
            );
            assert!(!prepared.sync_mounted(&root, window, cx).unwrap());
        });
    }

    #[gpui::test]
    fn prepared_follows_the_existing_mounted_reload_path(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let source = Source::new(SOURCE);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        context.update(|window, cx| {
            let mut prepared = runtime
                .prepare_application(&source.0, "main.js", window, cx)
                .unwrap();
            let root = prepared.mount(window, cx).unwrap();
            assert!(!prepared.sync_mounted(&root, window, cx).unwrap());
            let old = prepared.call("later", HostValue::Null, window, cx).unwrap();
            source.write(&SOURCE.replace("return constructed === 0", "return false"));
            runtime.reload_now(&root, window, cx).unwrap();
            assert!(prepared.sync_mounted(&root, window, cx).unwrap());
            assert!(!prepared.sync_mounted(&root, window, cx).unwrap());
            let call = prepared.call("when", HostValue::Null, window, cx).unwrap();
            assert_eq!(
                call.receiver.try_recv().unwrap().unwrap(),
                HostValue::Bool(false)
            );
            assert!(smol::block_on(old.resolve(Duration::ZERO)).is_err());
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

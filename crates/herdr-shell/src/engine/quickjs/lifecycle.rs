//! Bounded final calls for an application root before its owner removes it.
use super::*;
use crate::ShellRoot;
use rquickjs::Promise;
use std::time::{Duration, Instant};

struct Closing {
    root: Entity<ShellRoot>,
    view: Entity<ScriptView>,
    policy: Rc<Policy>,
    application: Rc<ApplicationGeneration>,
    promise: Option<Persistent<Promise<'static>>>,
}

impl Closing {
    fn pending(&self, runtime: &Rc<ShellRuntime>, window: &mut Window, cx: &mut App) -> bool {
        let (_scope, _) = scope::enter_with_application(
            runtime, window, cx, ScopePhase::Task, Some(self.view.clone()),
            self.policy.clone(), Some(self.application.clone()),
        );
        scheduler::poll_synchronous_close(runtime);
        scheduler::drain_runtime_jobs(runtime, window, cx);
        let Some(promise) = &self.promise else { return false };
        match runtime.with_js(|ctx| {
            let promise = promise.clone().restore(ctx)?;
            match promise.state() {
                rquickjs::promise::PromiseState::Pending => Ok(true),
                _ => {
                    if let Some(result) = promise.result::<Value>() { result?; }
                    Ok(false)
                }
            }
        }) {
            Ok(pending) => pending,
            Err(error) => {
                tracing::warn!("error in View.close: {error}");
                false
            }
        }
    }
}

impl ShellRuntime {
    fn start_close(
        self: &Rc<Self>, root: &Entity<ShellRoot>, deadline: Instant,
        window: &mut Window, cx: &mut App,
    ) -> Option<Closing> {
        let view = root.read(cx).application()?.view.clone();
        let object = view.read(cx).object().clone();
        let policy = view.read(cx).policy();
        let application = object.application_generation()?;
        if application.close_deadline.get().is_some() || !application.is_active() {
            return None;
        }
        application.close_deadline.set(Some(deadline));
        let (_scope, _) = scope::enter_with_application(
            self, window, cx, ScopePhase::Task, Some(view.clone()),
            policy.clone(), Some(application.clone()),
        );
        let promise = if Instant::now() < deadline {
            self.with_js(|ctx| {
                let instance = object.restore(ctx)?;
                let close: Value = instance.get("close")?;
                let Some(close) = close.into_function() else { return Ok(None) };
                let binding = ContextBinding::Task(Rc::new(TaskBinding {
                    owner: Some(view.downgrade()),
                    application: Some(Rc::downgrade(&application)),
                }));
                let result: Value = close.call((This(instance), context_object(ctx, binding)?))?;
                Ok(result.into_promise().map(|promise| Persistent::save(ctx, promise)))
            })
        } else { Ok(None) };
        if let Err(error) = &promise {
            tracing::warn!("error in View.close: {error}");
        }
        scheduler::drain_runtime_jobs(self, window, cx);
        Some(Closing { root: root.clone(), view, policy, application, promise: promise.ok().flatten() })
    }

    /// Gives this copy its optional final call, once. Detach or await the task.
    /// The copy is retained until its returned promise settles or time expires.
    pub fn close(
        self: &Rc<Self>, root: &Entity<ShellRoot>, deadline: Instant,
        window: &mut Window, cx: &mut App,
    ) -> gpui::Task<()> {
        let Some(closing) = self.start_close(root, deadline, window, cx) else {
            return gpui::Task::ready(());
        };
        let runtime = self.clone();
        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            while Instant::now() < deadline {
                let pending = cx.update_window(handle, |_, window, cx| closing.pending(&runtime, window, cx));
                if !matches!(pending, Ok(true)) { break; }
                cx.background_executor().timer(Duration::from_millis(5)).await;
            }
            let _ = cx.update(|cx| runtime.release_application_generation(&closing.application, cx));
            drop(closing.root);
        })
    }

    /// Final window/app teardown: pump host-module cleanup before GPUI destroys
    /// the window. All copies must receive the same deadline.
    pub fn close_now(
        self: &Rc<Self>, root: &Entity<ShellRoot>, deadline: Instant,
        window: &mut Window, cx: &mut App,
    ) {
        let _pump = scheduler::SynchronousClose::enter();
        let Some(closing) = self.start_close(root, deadline, window, cx) else { return };
        while Instant::now() < deadline && closing.pending(self, window, cx) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.release_application_generation(&closing.application, cx);
    }
}

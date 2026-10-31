//! Change anything: a one-line box that asks an agent to change Herdr. The
//! agent it starts is this app session's conversation: asking again opens it
//! (as a sub-agent opens) instead of another box. A restart forgets it.

use super::*;

/// Shared so the sidebar can show the conversation working; cleared on start.
const KEY: &str = "ask";
/// The turn the person last opened it at: a finish since then is news.
const SEEN: &str = "ask.seen";
/// On its terminal, with the home it changes: one window's never ends another's.
const LABEL: &str = "herdr-app.ask";

impl AppWindow {
    pub(super) fn forget_ask() {
        mods::set_shared(KEY.into(), Value::Null);
        mods::set_shared(SEEN.into(), Value::Null);
    }

    /// `argv`: start the conversation with it (the box's agent and message).
    pub(super) fn ask(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(argv) = Self::argv(args) {
            let r = self.start_ask(argv, cx);
            return self.answer(r);
        }
        match mods::shared(KEY).and_then(|v| v.as_str().map(str::to_owned)).filter(|t| self.live.get(t).is_some_and(|r| r["state"] == "live")) {
            Some(t) => {
                mods::set_shared(SEEN.into(), self.live[&t].get("turn").cloned().unwrap_or(json!(0)));
                self.peek_terminal(&t, window, cx);
            }
            None if self.overlays.iter().any(|o| o.id == "ask") => {}
            // The whole window, so the box can fly into the sidebar once it's sent.
            None => self.open_overlay("ask".into(), json!({ "mod": "frame-ask", "kind": "window", "frame": "none", "width": 1.0, "height": 1.0 }), window, cx),
        }
    }

    /// A finish the person watched (the conversation open, or kept as a shown
    /// tab) isn't news.
    pub(super) fn watch_ask(&self) {
        let Some(t) = mods::shared(KEY).and_then(|v| v.as_str().map(str::to_owned)) else { return };
        let open = self.visible_addresses().contains(&t)
            || self.overlays.iter().filter_map(|o| o.layout.as_ref()).any(|l| l.terminals().iter().any(|n| n.terminal() == Some(t.as_str())));
        let turn = self.live.get(&t).and_then(|r| r["turn"].as_u64());
        if open && self.window_active && turn > mods::shared(SEEN).and_then(|v| v.as_u64()) {
            mods::set_shared(SEEN.into(), json!(turn));
        }
    }

    fn start_ask(&mut self, argv: Vec<String>, cx: &mut Context<Self>) -> Result<Value, String> {
        let home = mods::app_dir().to_string_lossy().into_owned();
        // One at a time: one left from an earlier session ends, unless it was
        // kept as a tab.
        let old: Vec<String> = self.live.iter()
            .filter(|(_, r)| r.pointer(&format!("/labels/{LABEL}")).and_then(Value::as_str) == Some(home.as_str()))
            .filter(|(_, r)| r.pointer("/labels/herdr.space").and_then(Value::as_str).is_none_or(str::is_empty))
            .map(|(a, _)| a.clone())
            .collect();
        self.kill(&old, cx);
        Self::forget_ask();
        // In the person's shell, as tabs start agents (so their functions and
        // npm's shims run, and on macOS the login profile), but ending with it:
        // the conversation is the agent.
        let sh = shell();
        let argv = if cfg!(windows) {
            vec![sh, "-NoLogo".into(), "-Command".into(), crate::programs::powershell_line(&argv)]
        } else {
            let script = if std::path::Path::new(&sh).file_name().and_then(|s| s.to_str()) == Some("fish") { "command $argv[2..]" } else { "\"$@\"" };
            [sh.clone(), if cfg!(target_os = "macos") { "-lic" } else { "-ic" }.into(), script.into(), sh].into_iter().chain(argv).collect()
        };
        // As `Core::run`, plus this window's home: the core's own environment
        // may name another (its `herdr-app` calls would change that one), and
        // without keep: a lost agent isn't replaced by a shell.
        let env = json!({ "TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "herdr", "HERDR_APP_HOME": home });
        let r = self.core.call("terminal.run", json!({ "argv": argv, "cwd": home, "env": env, "labels": { LABEL: home }, "keep": false }))
            .map_err(|e| format!("cannot start a terminal: {e:#}"))?;
        let t = r["address"].as_str().map(str::to_owned).ok_or_else(|| format!("terminal.run returned no address: {r}"))?;
        mods::set_shared(KEY.into(), json!(t));
        Ok(json!({ "terminal": t }))
    }
}

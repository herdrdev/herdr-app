// Kept local to each mod so a cloned mod remains self-contained. The copies
// are checked by the title tests; title policy belongs to mods, not the model.
export function cleanTitle(value) {
  return String(value ?? "")
    .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .replace(/[\x00-\x1f\x7f-\x9f]/g, " ")
    .replace(/^[\s\u2800-\u28ff✳✻✽✶✢✦●○◐◓◑◒]+/u, "")
    .replace(/\s+/g, " ").trim();
}

function folder(cwd) {
  const path = (cwd ?? "").replace(/[\\/]+$/, "");
  return path.split(/[\\/]/).pop() || (cwd ? "/" : "");
}

export function taskTitle(p) {
  const title = cleanTitle(p?.title);
  const lower = title.toLowerCase();
  if (!title || [p?.program?.name, p?.program?.key, p?.app].some(s => s && s.toLowerCase() === lower)) return "";
  // A prompt's path/host and transient status are not a task description.
  if (/^(?:[~\/\\]|[a-z]:[\\/]|\S+@\S+:)/i.test(title) || title === p?.cwd) return "";
  if (/^(?:working|thinking|loading|connecting|idle|done|ready|busy|waiting)(?:[.\u2026! ]*)$/i.test(title)) return "";
  return title;
}

export function paneTitle(p) {
  if (!p) return "Terminal";
  if (p.mod) return p.title || p.mod;
  const task = taskTitle(p);
  if (task) return task;
  const program = p.program?.name || "Terminal";
  const dir = folder(p.cwd);
  return dir ? `${program} · ${dir}` : program;
}

export function terminalTitle(t) {
  // A core terminal's explicit name is separate from its live OSC title.
  return t?.name || paneTitle(t);
}

export function tabTitle(tab, all) {
  if (tab.named !== false) return tab.name; // Includes older snapshots without `named`.
  const mine = all.filter(p => p.tab === tab.id);
  // Stable pane order: moving focus to a split must not rename its tab.
  const ordered = [...mine.filter(p => p.program?.agent), ...mine.filter(p => !p.program?.agent)];
  const task = ordered.map(taskTitle).find(Boolean);
  return task || (ordered.length ? paneTitle(ordered[0]) : tab.name || "Terminal");
}

export function titleSnapshot(spaces, panes, terminals) {
  return {
    spaces: spaces.map(s => ({
      id: s.id, title: s.name, mode: s.named === false ? "automatic" : "manual", machine: s.machine,
      tabs: s.tabs.map(t => ({ id: t.id, title: tabTitle(t, panes), mode: t.named === false ? "automatic" : "manual" })),
    })),
    panes: panes.map(p => ({ id: p.pane, tab: p.tab, terminal: p.terminal, title: paneTitle(p) })),
    terminals: terminals.map(t => ({ address: t.address, title: terminalTitle(t) })),
  };
}

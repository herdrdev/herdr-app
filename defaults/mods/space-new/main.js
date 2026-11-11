// Open repository on this computer. The person picks a folder; it has to be a
// git repository (a directory inside one uses that repository). A local space
// already in it is shown. Otherwise one space is created there, with its
// first tab. New space stays space.new: this picker is local.

import { View, div } from "gpui-kit";
import { pickFolder, exec, spaces, panes, run, notify, closeOverlay, props } from "herdr";

const norm = (path) => (path ?? "").trim().replace(/\\/g, "/").replace(/\/+$/, "");

// Where a space is now: its first terminal's live folder, else where it was made.
function place(s, all) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return all.find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder || "";
}

async function repositoryRoot(runGit, folder) {
  if (!folder) return "";
  const result = await runGit("git", ["rev-parse", "--show-toplevel"], { cwd: folder });
  if (!result || result.code !== 0) return "";
  return norm(result.stdout);
}

// The local space whose terminal is in this repository, else one that was
// made there and has since moved. A worktree has its own toplevel, and a
// space on another machine is not this checkout.
async function spaceForRepository(host, root) {
  const paneList = host.panes();
  const local = host.spaces().filter((s) => (s.machine ?? null) === null);
  for (const s of local) {
    if (await repositoryRoot(host.exec, place(s, paneList)) === root) return s.id;
  }
  for (const s of local) {
    const made = s.folder || "";
    if (!made || norm(made) === norm(place(s, paneList))) continue;
    if (await repositoryRoot(host.exec, made) === root) return s.id;
  }
  return "";
}

export async function openRepository(host) {
  const picked = await host.pickFolder({ prompt: "Choose a git repository" });
  if (picked == null || picked === "") return "";
  const root = await repositoryRoot(host.exec, picked);
  if (!root) return "That folder is not a git repository.";
  const existing = await spaceForRepository(host, root);
  if (existing) {
    host.run("space.select", { id: existing });
    return "";
  }
  host.run("space.new", { folder: root, machine: "" });
  return "";
}

export default class SpaceNew extends View {
  init(_props, cx) {
    cx.spawn(async () => {
      try {
        const message = await openRepository({ pickFolder, exec, spaces, panes, run });
        if (message) notify({ title: "Open repository", body: message, level: "error" });
      } catch (error) {
        notify({ title: "Open repository", body: error?.message || String(error), level: "error" });
      } finally {
        closeOverlay(props()?.overlay);
      }
    });
  }

  render() {
    return div();
  }
}

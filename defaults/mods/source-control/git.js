import { exec, readTextFile } from "herdr";

const BASE = ["--no-pager", "--literal-pathspecs", "-c", "core.quotepath=false"];
const READ = ["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "diff.autoRefreshIndex=false"];
const DIFF = ["--no-ext-diff", "--no-textconv", "--no-color", "--find-renames", "--unified=3", "--src-prefix=a/", "--dst-prefix=b/", "--no-relative"];
const NETWORK = ["-c", "credential.interactive=false", "-c", "core.askPass=", "-c", "push.followTags=false"];
const EDITOR = ["-c", "core.editor=true", "-c", "sequence.editor=true"];

function required(value, label) {
  if (typeof value !== "string" || !value.trim() || value.includes("\0")) throw new Error(`${label} is required.`);
  return value;
}

function operand(value, label = "Target") {
  required(value, label);
  if (value.startsWith("-")) throw new Error(`${label} must not start with '-'.`);
  return value;
}

function relativePath(value) {
  required(value, "File path");
  if (/^(?:[a-z]:|[\\/])/i.test(value) || value.split(/[\\/]/).includes("..")) throw new Error("Use a relative file path inside the repository.");
  return value;
}

export function parseStatus(text) {
  const groups = { staged: [], changes: [], conflicts: [] }, entries = text.split("\0");
  for (let i = 0; i < entries.length; i++) {
    const entry = entries[i];
    if (!entry) continue;
    const xy = entry.slice(0, 2), file = { path: entry.slice(3), xy };
    if (/[RC]/.test(xy)) file.oldPath = entries[++i];
    if (xy === "??") groups.changes.push({ ...file, status: "?", untracked: true });
    else if (["DD", "AU", "UD", "UA", "DU", "AA", "UU"].includes(xy)) groups.conflicts.push({ ...file, status: "U" });
    else {
      if (xy[0] !== " " && xy[0] !== "!") groups.staged.push({ ...file, status: xy[0] });
      if (xy[1] !== " " && xy[1] !== "!") groups.changes.push({ ...file, oldPath: /[RC]/.test(xy[1]) ? file.oldPath : undefined, status: xy[1] });
    }
  }
  return groups;
}

export function parseNameStatus(text) {
  const entries = text.split("\0"), files = [];
  for (let i = 0; i < entries.length && entries[i];) {
    const status = entries[i++].trim(), first = entries[i++];
    if (/^[RC]/.test(status)) files.push({ status: status[0], oldPath: first, path: entries[i++] });
    else files.push({ status: status[0], path: first });
  }
  return files;
}

export function createGit(cwd, on = null, runner = exec, reader = readTextFile) {
  let root = cwd, snapshot = null;
  async function command(args, write = false, allowed = [0]) {
    const result = await runner("git", [...BASE, ...(write ? EDITOR : READ), ...args], { cwd: root, on });
    if (!allowed.includes(result.code)) throw new Error((result.stderr || result.stdout || `Git exited with code ${result.code}`).trim());
    return result;
  }
  const output = async (args, write = false) => (await command(args, write)).stdout;
  const hasHead = async () => (await command(["rev-parse", "--verify", "--quiet", "HEAD"], false, [0, 1])).code === 0;
  const commitId = async (ref) => (await output(["rev-parse", "--verify", "--end-of-options", `${operand(ref, "Commit")}^{commit}`])).trim();
  async function branchName(name) {
    operand(name, "Branch name");
    await output(["check-ref-format", "--branch", name]);
    return name;
  }
  function paths(args) {
    if (args.all) return ["."];
    const file = args.file, list = args.paths || (typeof file === "string" ? [file] : file ? [file.oldPath, file.path].filter(Boolean) : args.path ? [args.path] : []);
    if (!list.length) throw new Error("Select a file.");
    return [...new Set(list.map(relativePath))];
  }
  const currentGroups = async () => parseStatus(await output(["status", "--porcelain=v1", "-z", "--untracked-files=all"]));
  async function operation() {
    const markers = [
      ["rebase", "rebase-merge/head-name"], ["rebase", "rebase-apply/head-name"],
      ["merge", "MERGE_HEAD"], ["cherry-pick", "CHERRY_PICK_HEAD"], ["revert", "REVERT_HEAD"],
    ];
    const markerPaths = (await output(["rev-parse", "--path-format=absolute", ...markers.flatMap(([, marker]) => ["--git-path", marker])])).trimEnd().split(/\r?\n/);
    const states = await Promise.all(markers.map(async ([kind], index) => await reader(markerPaths[index], { on }) !== null ? kind : null));
    return states.find(Boolean) || null;
  }

  async function read(limit = 200, graphOptions = {}) {
    root = (await output(["rev-parse", "--show-toplevel"])).trim();
    const [status, headResult, nameResult, refText, remoteText, stashText, worktreeText, currentOperation] = await Promise.all([
      output(["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
      command(["rev-parse", "--verify", "--quiet", "HEAD"], false, [0, 1]),
      command(["symbolic-ref", "--quiet", "HEAD"], false, [0, 1]),
      output(["for-each-ref", "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream:track)%00%(objectname)%00%(upstream)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(symref)", "refs/heads", "refs/remotes", "refs/tags"]),
      output(["remote"]), output(["stash", "list", "--encoding=UTF-8", "--format=%gd%x00%H%x00%gs"]),
      output(["worktree", "list", "--porcelain", "-z"]), operation(),
    ]);
    const head = headResult.code === 0 ? headResult.stdout.trim() : null;
    const name = nameResult.code === 0 ? nameResult.stdout.trim().replace(/^refs\/heads\//, "") : null;
    let branch = name ? { name, upstream: "", remote: "", target: "", ahead: 0, behind: 0, gone: false } : null;
    const refs = await Promise.all(refText.split(/\r?\n/).filter(Boolean).map(async line => {
      const [ref, remote, target, track, objectHash, upstreamRef, objectType, peeledHash, peeledType, symbolicTarget] = line.split("\0");
      if (branch && ref === `refs/heads/${name}`) branch = { name, upstream: upstreamRef.replace(/^refs\/(?:heads|remotes)\//, ""), upstreamRef, remote, target, ahead: Number(/ahead (\d+)/.exec(track)?.[1] || 0), behind: Number(/behind (\d+)/.exec(track)?.[1] || 0), gone: track.includes("gone") };
      let hash = objectType === "commit" ? objectHash : peeledType === "commit" ? peeledHash : null;
      // A tag can itself name another annotated tag. Resolve that uncommon case to a commit.
      if (objectType === "tag" && peeledType === "tag") {
        const peeled = await command(["rev-parse", "--verify", "--quiet", "--end-of-options", `${ref}^{commit}`], false, [0, 1]);
        hash = peeled.code === 0 ? peeled.stdout.trim() : null;
      }
      return { ref, name: ref.replace(/^refs\/(?:heads|remotes|tags)\//, ""), hash, objectHash, type: ref.startsWith("refs/heads/") ? "branch" : ref.startsWith("refs/remotes/") ? "remote" : "tag", ...(symbolicTarget ? { symbolicTarget } : {}) };
    }));
    if (branch) branch.remoteHead = refs.find(ref => ref.ref === branch.upstreamRef)?.hash || null;
    const remotes = remoteText.split(/\r?\n/).filter(Boolean), byRef = new Map(refs.map(ref => [ref.ref, ref]));
    const canonical = value => {
      const seen = new Set();
      let ref = byRef.get(value);
      while (ref?.symbolicTarget && !seen.has(ref.ref)) { seen.add(ref.ref); ref = byRef.get(ref.symbolicTarget); }
      return ref?.hash && !ref.symbolicTarget ? ref : null;
    };
    let baseRef = null;
    if (branch) {
      // Read VS Code's branch-base preference without writing config or contacting a remote.
      const configured = (await command(["config", "--get", `branch.${branch.name}.vscode-merge-base`], false, [0, 1])).stdout.trim();
      const candidates = configured.startsWith("refs/") ? [configured] : [`refs/remotes/${configured}`, `refs/heads/${configured}`];
      const remoteOrder = [...new Set([branch.remote, "origin", ...remotes].filter(remote => remotes.includes(remote)))];
      candidates.push(...remoteOrder.map(remote => `refs/remotes/${remote}/HEAD`));
      candidates.push(...remoteOrder.flatMap(remote => ["main", "master"].map(name => `refs/remotes/${remote}/${name}`)), "refs/heads/main", "refs/heads/master");
      baseRef = candidates.map(canonical).find(ref => ref && ref.type !== "tag")?.ref || null;
    }
    const worktrees = []; let worktree;
    for (const entry of worktreeText.split("\0")) {
      if (entry.startsWith("worktree ")) { worktree = { path: entry.slice(9), branch: "" }; worktrees.push(worktree); }
      else if (entry.startsWith("branch ") && worktree) worktree.branch = entry.slice(7).replace(/^refs\/heads\//, "");
    }
    const mode = graphOptions?.mode || "auto", graphRefs = [];
    const include = value => { const ref = canonical(value); if (ref && !graphRefs.includes(ref.ref)) graphRefs.push(ref.ref); };
    if (mode === "all") refs.forEach(ref => include(ref.ref));
    else if (mode === "refs") { for (const ref of Array.isArray(graphOptions.refs) ? graphOptions.refs : []) include(ref); }
    else if (mode === "auto") { if (branch) include(`refs/heads/${branch.name}`); include(branch?.upstreamRef); include(baseRef); }
    else throw new Error("Unknown history filter.");
    const graphIncludesHead = Boolean(head && (mode !== "refs" || graphOptions.includeHead || (branch && graphRefs.includes(`refs/heads/${branch.name}`))));
    if (graphIncludesHead && branch) include(`refs/heads/${branch.name}`);
    // Use the captured commit OIDs so a deleted/moved selection cannot become an option or broaden the query.
    const revisions = [...new Set([...graphRefs.map(ref => byRef.get(ref).hash), ...(graphIncludesHead ? [head] : [])])];
    const graph = revisions.length || mode === "all" ? await output(["log", ...(mode === "all" ? ["--all"] : []), "--graph", "--topo-order", "--no-color", "--no-show-signature", "--encoding=UTF-8", "--decorate=short", "--date=short", `-n${Math.max(1, Math.floor(limit))}`, "--format=%x1f%H%x1f%D%x1f%s%x1f%an%x1f%ad", ...revisions, "--"]) : "";
    snapshot = { root, head, branch, groups: parseStatus(status), refs, remotes, stashes: stashText.split(/\r?\n/).filter(Boolean).map(line => { const [ref, hash, subject] = line.split("\0"); return { ref, hash, subject }; }), worktrees, operation: currentOperation, graph, graphRefs, graphIncludesHead, baseRef };
    return snapshot;
  }

  async function diff(file, group = "changes") {
    if (typeof file === "string") file = { path: file };
    const selected = [file.oldPath, file.path].filter(Boolean).map(relativePath);
    if (file.untracked) return (await command(["diff", "--no-index", ...DIFF, "--", "/dev/null", relativePath(file.path)], false, [0, 1])).stdout;
    return output(["diff", ...(group === "staged" ? ["--cached"] : []), ...DIFF, "--", ...selected]);
  }
  async function showCommit(hash) {
    const id = await commitId(hash);
    const [text, names] = await Promise.all([
      output(["show", "--no-patch", "--format=fuller", "--no-show-signature", "--no-color", "--encoding=UTF-8", id, "--"]),
      output(["show", "--format=", "--name-status", "-z", "--first-parent", "--diff-merges=first-parent", "--no-show-signature", ...DIFF, id, "--"]),
    ]);
    return { text, files: parseNameStatus(names) };
  }
  async function commitDiff(hash, file) {
    const id = await commitId(hash), selected = typeof file === "string" ? [file] : [file.oldPath, file.path].filter(Boolean);
    return output(["show", "--format=", "--first-parent", "--diff-merges=first-parent", "--no-show-signature", ...DIFF, id, "--", ...selected.map(relativePath)]);
  }
  async function remoteTarget(args, publish = false) {
    const branch = args.branch;
    if (!branch?.name) throw new Error("Select a branch and try again.");
    await branchName(branch.name);
    const current = await command(["symbolic-ref", "--quiet", "HEAD"], false, [0, 1]);
    if (current.stdout.trim() !== `refs/heads/${branch.name}`) throw new Error("The current branch changed. Refresh and try again.");
    const remote = operand(args.remote || branch.remote, "Remote");
    const target = branch.target || (publish ? `refs/heads/${branch.name}` : "");
    if (!target) throw new Error("No upstream branch is configured. Publish the branch first.");
    await output(["check-ref-format", target]);
    return { branch, remote, target };
  }
  async function network(action, args) {
    if (action === "fetch") {
      const remote = args.remote || snapshot?.branch?.remote || (snapshot?.remotes.includes("origin") ? "origin" : snapshot?.remotes.length === 1 ? snapshot.remotes[0] : null);
      if (!args.all && !remote) throw new Error("Select a remote to fetch.");
      return output([...NETWORK, "fetch", ...(args.all ? ["--all"] : []), ...(args.prune ? ["--prune"] : []), ...(args.all ? [] : ["--", operand(remote, "Remote")])], true);
    }
    const { branch, remote, target } = await remoteTarget(args, action === "publish");
    let result = "";
    if (action === "pull" || action === "sync") result += await output([...NETWORK, "pull", args.rebase ? "--rebase" : "--no-rebase", "--no-edit", "--", remote, target], true);
    const force = [];
    if (action === "forcePush") {
      const expected = args.expected || branch.remoteHead;
      if (!/^[a-f0-9]{40,64}$/i.test(expected || "")) throw new Error("The remote branch has no known commit. Fetch and try again.");
      force.push(`--force-with-lease=${target}:${expected}`);
    }
    if (action !== "pull") result += await output([...NETWORK, "push", "--porcelain", ...force, ...(action === "publish" ? ["--set-upstream"] : []), "--", remote, `refs/heads/${branch.name}:${target}`], true);
    return result;
  }

  async function run(action, args = {}) {
    if (["fetch", "pull", "sync", "push", "publish", "forcePush"].includes(action)) return network(action, args);
    switch (action) {
      case "init": return output(["init"], true);
      case "clone": return output([...NETWORK, "clone", "--", required(args.url, "Remote URL"), required(args.path, "Clone path")], true);
      case "pushTags": return output([...NETWORK, "push", "--porcelain", "--tags", "--", operand(args.remote, "Remote")], true);
      case "stage": {
        const selected = args.all ? [...new Set((await currentGroups()).changes.flatMap(file => [file.oldPath, file.path].filter(Boolean)))] : paths(args);
        return selected.length ? output(["add", "-A", "--", ...selected], true) : "";
      }
      case "unstage": {
        const selected = args.all ? [...new Set((await currentGroups()).staged.flatMap(file => [file.oldPath, file.path].filter(Boolean)))] : paths(args);
        if (!selected.length) return "";
        return await hasHead()
          ? output(["reset", "--quiet", "HEAD", "--", ...selected], true)
          : output(["rm", "--cached", "-r", "-f", "--ignore-unmatch", "--", ...selected], true);
      }
      case "resolve": {
        if (!["ours", "theirs"].includes(args.side)) throw new Error("Choose ours or theirs.");
        return output(["checkout", `--${args.side}`, "--", ...paths(args)], true);
      }
      case "applyPatch": {
        const file = required(args.patchFile, "Patch file path");
        const cached = args.cached === false ? [] : ["--cached"];
        await output(["apply", ...cached, "--check", "--whitespace=nowarn", "--", file], true);
        return output(["apply", ...cached, "--whitespace=nowarn", "--", file], true);
      }
      case "discard": {
        const groups = await currentGroups();
        const chosen = args.all ? groups.changes : paths(args).map(path => groups.changes.find(file => file.path === path)).filter(Boolean);
        if (!chosen.length) throw new Error("There are no working tree changes to discard.");
        const tracked = chosen.filter(file => !file.untracked).map(file => file.path), untracked = chosen.filter(file => file.untracked).map(file => file.path);
        let result = "";
        if (tracked.length) result += await output(["restore", "--worktree", "--", ...tracked], true);
        if (untracked.length) result += await output(["clean", "-f", "--", ...untracked], true);
        return result;
      }
      case "commit": case "commitAll": {
        if (!args.amend && !args.message?.trim()) throw new Error("Enter a commit message.");
        const flags = ["commit", ...(args.amend ? ["--amend"] : []), ...(args.message?.trim() ? ["-m", args.message] : ["--no-edit"]), ...(args.signoff ? ["--signoff"] : []), ...(args.signed === true ? ["-S"] : args.signed === false ? ["--no-gpg-sign"] : []), ...(args.allowEmpty ? ["--allow-empty"] : [])];
        if (action === "commitAll") {
          if ((await currentGroups()).conflicts.length) throw new Error("Resolve and stage conflicted files before committing.");
          await output(["add", "-A", "--", "."], true);
        }
        return output(flags, true);
      }
      case "undoCommit": return output(["reset", "--soft", "HEAD^"], true);
      case "reset": {
        const mode = args.mode || "mixed";
        if (!["soft", "mixed", "hard"].includes(mode)) throw new Error("Unsupported reset mode.");
        return output(["reset", `--${mode}`, await commitId(args.hash)], true);
      }
      case "branchCreate": {
        const name = await branchName(args.name), start = args.start ? [await commitId(args.start)] : [];
        return output(args.checkout === false ? ["branch", "--", name, ...start] : ["checkout", "-b", name, ...start, "--"], true);
      }
      case "checkout": {
        const ref = operand(args.ref, "Branch or commit");
        if (args.detach) return output(["checkout", "--detach", await commitId(ref), "--"], true);
        const remote = ref.startsWith("refs/remotes/"), name = ref.replace(/^refs\/(?:heads|remotes)\//, "");
        return output(["checkout", ...(args.track || remote ? ["--track"] : []), name, "--"], true);
      }
      case "branchDelete": return output(["branch", args.force === true ? "-D" : "-d", "--", await branchName(args.name)], true);
      case "branchRename": return output(["branch", "-m", "--", await branchName(args.name), await branchName(args.newName)], true);
      case "merge": return output(["merge", "--no-edit", "--", await commitId(args.ref)], true);
      case "rebase": return output(["rebase", "--", await commitId(args.ref)], true);
      case "cherryPick": return output(["cherry-pick", await commitId(args.hash)], true);
      case "revert": return output(["revert", "--no-edit", await commitId(args.hash)], true);
      case "continue": case "abort": {
        const active = await operation();
        if (!["merge", "rebase", "cherry-pick", "revert"].includes(args.operation) || active !== args.operation) throw new Error("The Git operation changed. Refresh and try again.");
        return output([args.operation, `--${action}`], true);
      }
      case "stashSave": return output(["stash", "push", ...(args.includeUntracked ? ["--include-untracked"] : []), ...(args.message?.trim() ? ["-m", args.message] : [])], true);
      case "stashApply": case "stashPop": case "stashDrop": {
        const ref = operand(args.ref, "stash"), expected = args.hash || snapshot?.stashes.find(item => item.ref === ref)?.hash;
        if (expected && await commitId(ref) !== expected) throw new Error("The selected stash changed. Refresh and select it again.");
        return output(["stash", action.slice(5).toLowerCase(), "--", ref], true);
      }
      case "tagCreate": {
        const name = operand(args.name, "Tag name");
        await output(["check-ref-format", `refs/tags/${name}`]);
        return output(["tag", ...(args.signed ? ["-s", "-m", args.message || name] : args.message ? ["-a", "-m", args.message] : []), "--", name, ...(args.ref ? [await commitId(args.ref)] : [])], true);
      }
      case "tagDelete": return output(["tag", "-d", "--", operand(args.name, "Tag name")], true);
      case "worktreeAdd": return output(["worktree", "add", ...(args.name ? ["-b", await branchName(args.name)] : []), "--", required(args.path, "Worktree path"), ...(args.ref ? [operand(args.ref, "Branch")] : [])], true);
      case "worktreeRemove": return output(["worktree", "remove", "--", required(args.path, "Worktree path")], true);
      case "remoteAdd": return output(["remote", "add", "--", operand(args.name, "Remote name"), required(args.url, "Remote URL")], true);
      case "remoteRemove": return output(["remote", "remove", "--", operand(args.name, "Remote name")], true);
      default: throw new Error(`Unsupported Git action: ${action}`);
    }
  }
  return { read, run, diff, showCommit, commitDiff };
}

// Linear resolves project UUIDs to canonical project URLs after sign-in.
export function linearTarget(content) {
  const fallback = { url: "https://linear.app/?noRedirect=1", linked: false };
  try {
    const { workspace, project } = JSON.parse(content);
    if (typeof workspace !== "string" || !/^[a-z0-9][a-z0-9-]*$/i.test(workspace) ||
        typeof project !== "string" || !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(project)) return fallback;
    return { url: `https://linear.app/${workspace}/project/${project}?noRedirect=1`, linked: true };
  } catch { return fallback; }
}

export async function resolveLinearTarget(cwd, on, read, exec) {
  if (!cwd) return linearTarget(null);
  try {
    let content = await read(`${cwd}/.linear-project`, { on });
    if (content === null) {
      const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd, on });
      if (top.code === 0 && top.stdout.trim() && top.stdout.trim() !== cwd)
        content = await read(`${top.stdout.trim()}/.linear-project`, { on });
    }
    return linearTarget(content);
  } catch { return linearTarget(null); }
}
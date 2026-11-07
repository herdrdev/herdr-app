const HOME = "https://github.com/";

// Accept only GitHub's normal clone URLs; never send credentials or a remote
// hostname supplied by a repository to the embedded browser.
export function githubTarget(remote) {
  const match = /^(?:https:\/\/github\.com\/|ssh:\/\/git@github\.com\/|git@github\.com:)([a-z0-9](?:[a-z0-9-]{0,37}[a-z0-9])?)\/([a-z0-9_.-]+?)(?:\.git)?\/?$/i.exec((remote || "").trim());
  if (!match || match[2] === "." || match[2] === "..") return HOME;
  return `https://github.com/${match[1]}/${match[2]}`;
}

export async function resolveGithubTarget(cwd, on, exec) {
  if (!cwd) return HOME;
  try {
    const remote = await exec("git", ["remote", "get-url", "origin"], { cwd, on });
    return remote.code === 0 ? githubTarget(remote.stdout) : HOME;
  } catch { return HOME; }
}

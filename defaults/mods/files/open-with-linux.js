import { exec, readTextFile } from "herdr";

export async function applications(path) {
  // GIO consults MIME associations and desktop entries, rather than a catalog
  // of editor brands. Use a stable locale for its human-readable CLI output.
  const result = await exec("sh", ["-c", 'printf "%s\\0%s\\0" "${XDG_DATA_HOME:-$HOME/.local/share}" "${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"; mime=$(xdg-mime query filetype "$1") || exit; LC_ALL=C gio mime "$mime"', "files-apps", path]);
  if (result.code !== 0) throw new Error(result.stderr.trim() || "Could not read system application associations");
  const [personal, directories, output] = result.stdout.split("\0");
  const roots = [personal, ...directories.split(":")];
  const defaultId = output.match(/^Default application for .*?: (.+)$/m)?.[1];
  const ids = [...new Set([...(defaultId ? [defaultId] : []), ...[...output.matchAll(/^\s+(.+\.desktop)\s*$/gm)].map(m => m[1].trim())])];
  const apps = [];
  let defaultName = null, defaultPath = null;
  for (const id of ids) for (const root of roots) {
    const file = `${root}/applications/${id}`, text = await readTextFile(file);
    if (text === null) continue;
    const entry = text.split(/^\[Desktop Entry\]\s*$/m)[1]?.split(/^\[/m)[0] ?? "";
    if (/^Hidden=true\s*$/m.test(entry)) break;
    const name = entry.match(/^Name=(.+)$/m)?.[1]?.trim();
    if (name) {
      apps.push({ id: file, name, kind: "desktop" });
      if (id === defaultId) { defaultName = name; defaultPath = file; }
    }
    break;
  }
  return { defaultId: defaultPath, defaultName, apps };
}

export const picker = `
if command -v zenity >/dev/null; then
  zenity --file-selection --title="Choose an application"
elif command -v kdialog >/dev/null; then
  kdialog --getopenfilename /usr/bin --title "Choose an application"
else
  printf "No application picker is available (zenity or kdialog is required).\\n" >&2
  exit 127
fi
code=$?
[ "$code" -eq 1 ] && exit 0
exit "$code"
`;

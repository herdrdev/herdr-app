import { exec, window, appHome } from "herdr";
import { mkdir, exists, readFile, unlink } from "fs/promises";
import { randomBytes } from "crypto";
import { Buffer } from "buffer";
import * as macos from "./open-with-macos.js";
import * as windows from "./open-with-windows.js";
import * as linux from "./open-with-linux.js";

const quote = s => "'" + s.replace(/['‘’‚‛]/g, "$&$&") + "'";
const powershell = script => exec("powershell", ["-NoProfile", "-NonInteractive", "-STA", "-Command", "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); " + script]);
function checked(result) {
  if (result.code !== 0) throw new Error(result.stderr.trim() || "Could not open this file");
  return result.stdout;
}

export async function applications(path) {
  const platform = window()?.platform;
  if (platform === "macos") return JSON.parse(checked(await exec("osascript", ["-l", "JavaScript", "-e", macos.applications, path])));
  if (platform === "windows") return JSON.parse(checked(await powershell(windows.associations + `[FileApps]::List(${quote(path.replace(/\//g, "\\"))}) | ConvertTo-Json -Depth 4 -Compress`)));
  return linux.applications(path);
}

export async function openFile(path, app = null) {
  const platform = window()?.platform;
  if (platform === "macos") {
    checked(await exec("open", [...(app ? ["-a", app.id] : []), "--", path]));
  } else if (platform === "windows") {
    const file = path.replace(/\//g, "\\");
    if (app?.kind === "handler") checked(await powershell(windows.associations + `[FileApps]::Open(${quote(file)}, ${quote(app.id)})`));
    else checked(await powershell(`Start-Process -FilePath ${quote(app?.id || file)}` + (app ? ` -ArgumentList ${quote('"' + file + '"')}` : "")));
  } else if (app?.kind === "desktop") {
    // The desktop launcher handles Exec field codes and packaged apps. Redirect
    // child output so an editor cannot keep exec's capture pipes alive.
    checked(await exec("sh", ["-c", 'gio launch "$1" "$2" >/dev/null 2>&1', "files-open", app.id, path]));
  } else {
    // Editors and xdg-open can wait for a window to close. Detach them from
    // exec's 30 s limit so opening a file never kills a long-running editor.
    checked(await exec("sh", ["-c", 'command -v "$1" >/dev/null || { printf "Cannot find executable: %s\\n" "$1" >&2; exit 127; }; nohup "$1" "$2" </dev/null >/dev/null 2>&1 &', "files-open", app?.id || "xdg-open", path]));
  }
}

export async function pickApplication(cx) {
  const platform = window()?.platform, dir = `${appHome()}/data/files`;
  await mkdir(dir, { recursive: true });
  const output = `${dir}/pick-${randomBytes(12).toString("hex")}`;
  // A person can leave the system picker open indefinitely. Start it separately
  // from exec's bounded commands and read its atomically completed response.
  if (platform === "windows") {
    const script = `$ErrorActionPreference = 'Stop'; try { $picked = & { ${windows.picker} }; [IO.File]::WriteAllText(${quote(output + ".tmp")}, [string]$picked); [IO.File]::Move(${quote(output + ".tmp")}, ${quote(output)}); } catch { [IO.File]::WriteAllText(${quote(output + ".err.tmp")}, $_.Exception.Message); [IO.File]::Move(${quote(output + ".err.tmp")}, ${quote(output + ".err")}); }`;
    const encoded = Buffer.from(script, "utf16le").toString("base64");
    checked(await powershell(`Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile -STA -EncodedCommand ${encoded}'`));
  } else {
    const command = platform === "macos" ? ["osascript", "-e", macos.picker] : ["sh", "-c", linux.picker];
    checked(await exec("sh", ["-c", 'out=$1; shift; ( if "$@" >"$out.tmp" 2>"$out.err.tmp"; then mv "$out.tmp" "$out"; rm -f "$out.err.tmp"; else mv "$out.err.tmp" "$out.err"; rm -f "$out.tmp"; fi ) </dev/null >/dev/null 2>&1 &', "files-picker", output, ...command]));
  }
  try {
    for (;;) {
      if (await exists(output + ".err")) throw new Error(await readFile(output + ".err", "utf8"));
      if (await exists(output)) {
        let path = await readFile(output, "utf8");
        if (platform !== "windows") path = path.replace(/\r?\n$/, "");
        if (!path) return null;
        return { id: path, name: path.split(/[\\/]/).pop().replace(/\.(app|exe|desktop)$/i, ""),
          kind: platform === "macos" ? "app" : path.endsWith(".desktop") ? "desktop" : "executable" };
      }
      await cx.sleep(250);
    }
  } finally {
    for (const path of [output, output + ".err"]) if (await exists(path)) await unlink(path);
  }
}

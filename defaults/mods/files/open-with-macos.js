// Launch Services supplies the same file-specific application choices as Finder.
export const applications = `
ObjC.import("AppKit");
function run(args) {
  const workspace = $.NSWorkspace.sharedWorkspace;
  const file = $.NSURL.fileURLWithPath(args[0]);
  const name = url => ObjC.unwrap($.NSFileManager.defaultManager.displayNameAtPath(url.path)).replace(/\\.app$/, "");
  const preferred = workspace.URLForApplicationToOpenURL(file);
  const urls = workspace.URLsForApplicationsToOpenURL(file), apps = [];
  for (let i = 0; i < urls.count; i++) {
    const url = urls.objectAtIndex(i);
    apps.push({ id: ObjC.unwrap(url.path), name: name(url), kind: "app" });
  }
  return JSON.stringify({ defaultId: preferred ? ObjC.unwrap(preferred.path) : null,
    defaultName: preferred ? name(preferred) : null, apps });
}`;

export const picker = `
try
  set chosen to choose application with prompt "Choose an application to open this file" as alias
  return POSIX path of chosen
on error number -128
  return ""
end try`;
